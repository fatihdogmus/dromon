//! Live updates (§11 "Live mode"): a second ssh session per host runs `inotifywait -m` on the
//! folder on screen (that folder alone, not its subfolders), and each change it reports starts
//! the same poll the poller would (`conn_live.rs`), so the view shows it within a fraction of a
//! second. Polls go on, stretched (`poll.rs`'s `LIVE_STRETCH`), for what inotify misses.
//!
//! - The pure part is the [`Phase`] machine ([`step`]), the [`Debounce`] of changes and what a
//!   line means ([`event_of`]): no clock, no I/O. The host's connection task (`conn.rs`) owns the
//!   clock, the [`Watcher`] and the session it follows.
//! - What to watch ([`Event::Target`]) is the focused (else a visible) window's folder on this
//!   host, while the session is up and the setting is on; the watch restarts on another folder
//!   and stops when there's none.
//! - Whether the server can is learnt once per session: no `inotifywait`, a sign-in only the user
//!   could answer, or a watch that never started make the host [`Phase::Unavailable`] until its
//!   session comes up again ([`Event::Reset`]). A watch that ends later ([`Phase::Ended`]) leaves
//!   its folder to polls until another one is shown.
//! - Signing in: a [`Watcher`] has an askpass endpoint and an `AuthAttempt` of its own. A
//!   remembered password or passphrase answers, as on a reconnect; anything else is cancelled
//!   and the watch gives up. No prompt of it ever reaches the user.
//! - The command line is `dromon-ssh`'s (`SshCommand::watch`, the folder POSIX-quoted). The
//!   watch's lines name events only, never files; trace lines name the state alone
//!   (`ServiceEvent::Live`).

use std::future::pending;
use std::io;
use std::process::ExitStatus;
use std::sync::Mutex;

use dromon_core::{HostSpec, PromptAnswer, RemotePath};
use dromon_ssh::{
    AskpassEnv, AskpassRequest, AskpassServer, AuthAttempt, AuthDecision, PasswordStore, SshCommand, SshStream,
    WATCH_MISSING, WATCH_READY, classify_prompt, spawn,
};
use secrecy::ExposeSecret;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};

use super::Config;
use super::auth::{lock, wipe};
use crate::state::{Live, LiveUnavailable};

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;

/// How long changes gather before the poll they start: a burst (a file written in many pieces,
/// a folder unpacked) is one listing.
pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// The longest line read from the watch; a longer one counts as a change and is cut there.
const MAX_LINE: u64 = 4096;
/// Lines waiting for the task; a change while it's full is one already waiting.
const QUEUE: usize = 64;

/// Where one host's live updates stand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Nothing to watch: no folder on screen here, the session down, or the setting off.
    #[default]
    Idle,
    /// The second session is signing in and checking for `inotifywait`, for this folder.
    Starting(RemotePath),
    /// This folder is watched: each change starts a poll.
    Watching(RemotePath),
    /// The watch on this folder ended (killed, the folder gone): polled until another is shown.
    Ended(RemotePath),
    /// Not for this host until its session comes up again.
    Unavailable(LiveUnavailable),
}

impl Phase {
    /// What the windows are told.
    pub fn shown(&self) -> Live {
        match self {
            Phase::Watching(path) => Live::On(path.clone()),
            Phase::Unavailable(why) => Live::Unavailable(*why),
            Phase::Idle | Phase::Starting(_) | Phase::Ended(_) => Live::Off,
        }
    }

    /// The folder watched, while one is.
    pub fn watching(&self) -> Option<&RemotePath> {
        match self {
            Phase::Watching(path) => Some(path),
            _ => None,
        }
    }
}

/// What moves the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The folder to watch now (`None`: none).
    Target(Option<RemotePath>),
    /// The server has `inotifywait`; the watch is starting ([`WATCH_READY`]).
    Ready,
    /// The server has no `inotifywait` ([`WATCH_MISSING`]).
    Missing,
    /// The watch reported a change.
    Changed,
    /// Signing in asked something only the user could answer.
    CantSignIn,
    /// The second session's output ended: its ssh is exiting.
    Exited,
    /// The host's session came up (again): what the server can do is asked afresh.
    Reset,
}

/// What the owner does for a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Start a watch of this folder (a new [`Watcher`]).
    Spawn(RemotePath),
    /// Stop the watch (drop its [`Watcher`], which kills its ssh).
    Kill,
    /// A change: poll the folder watched soon ([`Debounce::changed`]).
    Changed,
    /// A line for the host's connection log.
    Log(String),
}

/// Steps `phase` by `event`; the owner carries the effects out, in order.
pub fn step(phase: &mut Phase, event: Event) -> Vec<Effect> {
    match event {
        Event::Target(target) => retarget(phase, target),
        Event::Reset => {
            let running = matches!(phase, Phase::Starting(_) | Phase::Watching(_));
            *phase = Phase::Idle;
            if running { vec![Effect::Kill] } else { Vec::new() }
        }
        Event::Ready => match phase {
            Phase::Starting(path) => {
                *phase = Phase::Watching(path.clone());
                // What changed between the folder's listing and the watch's start.
                vec![Effect::Changed]
            }
            _ => Vec::new(),
        },
        // Lines before the watch starts aren't its.
        Event::Changed if matches!(phase, Phase::Watching(_)) => vec![Effect::Changed],
        Event::Changed => Vec::new(),
        Event::Missing => give_up(phase, LiveUnavailable::NoInotify),
        Event::CantSignIn => give_up(phase, LiveUnavailable::SignIn),
        Event::Exited => match phase {
            Phase::Starting(_) => give_up(phase, LiveUnavailable::Failed),
            Phase::Watching(path) => {
                *phase = Phase::Ended(path.clone());
                vec![Effect::Kill, Effect::Log("Live updates stopped; changes show every few seconds.".into())]
            }
            _ => Vec::new(),
        },
    }
}

/// The folder to watch is `target` now.
fn retarget(phase: &mut Phase, target: Option<RemotePath>) -> Vec<Effect> {
    match (&*phase, target) {
        (Phase::Unavailable(_), _) => Vec::new(),
        (Phase::Starting(p) | Phase::Watching(p) | Phase::Ended(p), Some(to)) if *p == to => Vec::new(),
        (Phase::Starting(_) | Phase::Watching(_), None) => {
            *phase = Phase::Idle;
            vec![Effect::Kill]
        }
        (Phase::Starting(_) | Phase::Watching(_), Some(to)) => {
            *phase = Phase::Starting(to.clone());
            vec![Effect::Kill, Effect::Spawn(to)]
        }
        (Phase::Idle | Phase::Ended(_), None) => {
            *phase = Phase::Idle;
            Vec::new()
        }
        (Phase::Idle | Phase::Ended(_), Some(to)) => {
            *phase = Phase::Starting(to.clone());
            vec![Effect::Spawn(to)]
        }
    }
}

/// A watch still starting learnt the server can't: not again this session.
fn give_up(phase: &mut Phase, why: LiveUnavailable) -> Vec<Effect> {
    if !matches!(phase, Phase::Starting(_)) {
        return Vec::new();
    }
    *phase = Phase::Unavailable(why);
    vec![Effect::Kill, Effect::Log(format!("No live updates: {}.", why.reason()))]
}

/// What a line of the watch's output means.
pub fn event_of(line: &[u8]) -> Event {
    match line.trim_ascii() {
        l if l == WATCH_READY.as_bytes() => Event::Ready,
        l if l == WATCH_MISSING.as_bytes() => Event::Missing,
        _ => Event::Changed,
    }
}

/// When the poll changes start is due: [`DEBOUNCE`] after the first change it hasn't covered.
#[derive(Debug, Default)]
pub struct Debounce {
    due: Option<Instant>,
}

impl Debounce {
    /// A change at `now`.
    pub fn changed(&mut self, now: Instant) {
        self.due.get_or_insert(now + DEBOUNCE);
    }

    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// Whether the poll is due at `now`; it's taken if so (changes after this start another).
    pub fn take(&mut self, now: Instant) -> bool {
        let due = self.due.is_some_and(|d| d <= now);
        if due {
            self.due = None;
        }
        due
    }

    /// No folder is watched any more.
    pub fn clear(&mut self) {
        self.due = None;
    }
}

/// One host's live updates, as its connection task holds them.
pub struct Updates {
    pub phase: Phase,
    pub watcher: Option<Watcher>,
    pub relist: Debounce,
    /// What the windows were last told.
    pub shown: Live,
    /// The setting (`Command::LiveUpdates`).
    pub on: bool,
}

impl Updates {
    pub fn new(on: bool) -> Updates {
        Updates { phase: Phase::Idle, watcher: None, relist: Debounce::default(), shown: Live::Off, on }
    }
}

/// What a [`Watcher`] heard.
pub enum Heard {
    Event(Event),
    /// Its ssh asks something to sign in ([`Watcher::answer`]).
    Ask(AskpassRequest),
}

/// One run of the watch: its ssh, an askpass endpoint and a sign-in of its own (so a prompt of
/// a run being replaced never counts as this one's), and a task reading its lines. Dropping it
/// kills ssh and closes the endpoint.
pub struct Watcher {
    _askpass: Option<AskpassServer>,
    asks: Option<mpsc::Receiver<AskpassRequest>>,
    auth: AuthAttempt,
    events: mpsc::Receiver<Event>,
    reader: JoinHandle<()>,
    /// Resolves when ssh exits; dropping `kill` kills it.
    _exit: JoinHandle<io::Result<ExitStatus>>,
    _kill: oneshot::Sender<()>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl Watcher {
    /// Starts ssh watching `folder` on `spec`'s host, as `cfg` runs ssh.
    pub async fn start(spec: &HostSpec, cfg: &Config, folder: &RemotePath) -> Result<Watcher, String> {
        // No endpoint: ssh's questions are cancelled; keys and agents still work.
        let mut server = AskpassServer::bind().ok();
        let asks = server.as_mut().and_then(AskpassServer::take_requests);
        let env = match &server {
            Some(s) => s.register(spec.id.clone(), &cfg.helper_exe).map_err(|e| e.to_string())?,
            None => AskpassEnv { exe: cfg.helper_exe.clone(), endpoint: String::new(), token: String::new() },
        };
        let built = SshCommand::watch(spec, &env, folder.as_str()).and_then(|cmd| match &cfg.config_file {
            Some(file) => cmd.with_config_file(file),
            None => Ok(cmd),
        });
        let mut cmd = built.map_err(|e| e.to_string())?;
        cmd.program = cfg.ssh.clone();
        for name in &cfg.without_env {
            cmd = cmd.without_env(name.clone());
        }
        let mut child = spawn(&cmd).await.map_err(|e| format!("Couldn't run {}: {e}", cmd.program.display()))?;
        let out = child.take_stream().ok_or("ssh's output isn't piped")?;
        // Its stderr isn't read (the child keeps draining it): the log has the session's.
        drop(child.take_stderr());
        let (tx, events) = mpsc::channel(QUEUE);
        let reader = tokio::spawn(read_lines(out, tx));
        let (kill, killed) = oneshot::channel::<()>();
        let exit = tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => status,
                _ = killed => {
                    child.kill().await?;
                    Err(io::Error::other("stopped"))
                }
            }
        });
        let auth = AuthAttempt::new(spec.id.clone());
        Ok(Watcher { _askpass: server, asks, auth, events, reader, _exit: exit, _kill: kill })
    }

    async fn next(&mut self) -> Heard {
        tokio::select! {
            Some(req) = recv_opt(&mut self.asks) => Heard::Ask(req),
            ev = self.events.recv() => Heard::Event(ev.unwrap_or(Event::Exited)),
        }
    }

    /// Answers `req` from memory, as a reconnect would. `false`: only the user could answer it;
    /// it is cancelled (`req` dropped) and the watch should give up.
    pub fn answer(&mut self, req: AskpassRequest, store: &Mutex<PasswordStore>) -> bool {
        if req.reply.is_closed() {
            return true;
        }
        let kind = classify_prompt(&req.prompt, req.hint.as_deref());
        match self.auth.on_prompt(&kind, &req.prompt, &mut lock(store)) {
            AuthDecision::AnswerFromMemory(secret) => {
                if let Err(answer) = req.reply.send(PromptAnswer::Text(secret.expose_secret().to_owned())) {
                    wipe(answer);
                }
                true
            }
            AuthDecision::AskUser { .. } => false,
        }
    }
}

/// What the watch running (if any) heard next.
pub async fn next(watcher: &mut Option<Watcher>) -> Heard {
    match watcher {
        Some(w) => w.next().await,
        None => pending().await,
    }
}

async fn recv_opt<T>(rx: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}

/// Reads the watch's lines (each at most [`MAX_LINE`]) until its output ends, as events.
async fn read_lines(out: SshStream, tx: mpsc::Sender<Event>) {
    let mut reader = BufReader::new(out);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader).take(MAX_LINE).read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        match event_of(&line) {
            // A change waiting already covers this one.
            Event::Changed => {
                if let Err(TrySendError::Closed(_)) = tx.try_send(Event::Changed) {
                    return;
                }
            }
            marker => {
                if tx.send(marker).await.is_err() {
                    return;
                }
            }
        }
    }
}
