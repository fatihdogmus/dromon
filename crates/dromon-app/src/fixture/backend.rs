//! [`FixtureBackend`]: a [`Backend`] that answers from a [`Fixture`] synchronously, from
//! [`Backend::send`], with the events the real service would send.
//!
//! Each host's connection runs the service's own lifecycle machine (`service::lifecycle`), so
//! status changes, "Connected." and "Couldn't connect: …" lines, Disconnect and a lost
//! connection behave exactly as they do for real. One connection attempt stands in for one ssh
//! process: its auth steps are asked in order through a real `AuthAttempt`, so remembered
//! passwords and retry errors (`WRONG_PASSWORD`, `WRONG_PASSPHRASE`, `WRONG_SIGN_IN`) are the
//! real ones. A prompt stays open until the UI answers it.
//!
//! There's no clock: a lost connection says when it would retry, then stays reconnecting.
//! Answers are compared with the fixture's and wiped; nothing logs them.
//!
//! Transfer commands go to [`Engine`] (`fixture/transfers.rs`), whose listings (the tree plus
//! finished uploads) are the ones this backend sends, with what file operations changed in them
//! on top ([`Changes`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use dromon_core::{ConnStatus, Error, HostId, HostSpec, Prompt, PromptAnswer, PromptId, PromptKind, RemotePath};
use dromon_ssh::{AskpassEnv, AuthAttempt, AuthDecision, PasswordStore, SshCommand};
use secrecy::ExposeSecret;
use zeroize::Zeroize;

use super::transfers::Engine;
use super::{Changes, Fixture, Step};
use crate::service::lifecycle::{self, Event, Lifecycle, Phase};
use crate::state::{Backend, Command, FileOp, OpId, ServiceEvent, TransferCommand};

/// Entries per listing page, as the service pages them (§4.4).
pub const PAGE: usize = 512;

/// Wrong answers one attempt takes before ssh gives up (ssh's `NumberOfPasswordPrompts`).
pub const TRIES: u32 = 3;

/// The backend on a fixture.
pub struct FixtureBackend {
    fixture: Arc<Fixture>,
    conns: BTreeMap<HostId, Conn>,
    /// Remembered passwords and passphrases, as the service keeps them.
    store: PasswordStore,
    last_prompt: u64,
    /// Downloads and uploads (`fixture::transfers`); it also owns what uploads add to listings.
    transfers: Engine,
    /// What new folders, renames and deletes changed, over the engine's listings.
    changes: Changes,
}

#[derive(Default)]
struct Conn {
    life: Lifecycle,
    /// The running attempt ("ssh"), if any.
    attempt: Option<Attempt>,
    /// The attempt that just ended, for `AuthFailed`.
    ended: Option<AuthAttempt>,
    /// The host key was trusted once: later attempts don't ask (known_hosts).
    trusted: bool,
    /// The folder on screen, listed again after a reconnect.
    path: Option<RemotePath>,
}

/// One connection attempt, at auth step `step`.
struct Attempt {
    auth: AuthAttempt,
    step: usize,
    /// Times the current step was asked.
    asked: u32,
    /// Wrong answers so far.
    wrong: u32,
    /// The prompt waiting for the user.
    waiting: Option<Open>,
    /// Notices on screen until the attempt ends.
    notices: Vec<PromptId>,
}

struct Open {
    id: PromptId,
    kind: PromptKind,
    text: String,
}

/// What a host's connection does next, in order.
enum Work {
    Life(Event),
    /// Ask the next step, or finish signing in.
    Advance,
    /// The user's answer to the open prompt.
    Answer(PromptAnswer),
    /// Check an answer against the current step.
    Judge(PromptAnswer),
}

fn wipe(answer: PromptAnswer) {
    if let PromptAnswer::Text(mut t) = answer {
        t.zeroize();
    }
}

/// The first line of ssh's prompt, for the log.
fn first_line(text: &str) -> &str {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

impl FixtureBackend {
    pub fn new(fixture: Arc<Fixture>) -> Self {
        let transfers = Engine::new(fixture.clone());
        FixtureBackend { fixture, conns: BTreeMap::new(), store: PasswordStore::new(), last_prompt: 0, transfers, changes: Changes::default() }
    }

    /// Whether `host`'s session is up (ready, its "ssh" running).
    fn up(&self, host: &HostId) -> bool {
        self.conns.get(host).is_some_and(|c| c.life.phase == Phase::Ready && c.attempt.is_some())
    }

    /// A file operation, carried out at once against the trees: a delete's one progress event,
    /// then Done or Failed, then the folder on screen again if the operation changed it (as
    /// the service does). Not connected: `ConnectionLost` at once. A folder in the way whose
    /// listing loses the connection fails it with `ConnectionLost` and drops the connection.
    fn file_op(&mut self, id: OpId, host: HostId, op: FileOp, out: &mut Vec<ServiceEvent>) {
        if !self.up(&host) {
            out.push(ServiceEvent::FileOpFailed { id, host, error: Error::ConnectionLost });
            return;
        }
        let (engine, changes) = (&self.transfers, &mut self.changes);
        let result = match &op {
            FileOp::Mkdir { path } => changes.mkdir(engine, &host, path, self.fixture.now),
            FileOp::Rename { from, to } => changes.rename(engine, &host, from, to),
            FileOp::Delete { paths } => changes.delete(engine, &host, paths).map(|n| {
                out.push(ServiceEvent::FileOpProgress { id, host: host.clone(), done: n, total: Some(n) });
            }),
        };
        let ok = result.is_ok();
        match result {
            Ok(()) => out.push(ServiceEvent::FileOpDone { id, host: host.clone() }),
            Err(Error::ConnectionLost) => {
                out.push(ServiceEvent::FileOpFailed { id, host: host.clone(), error: Error::ConnectionLost });
                return self.drive(&host, Work::Life(Event::Lost), out);
            }
            Err(error) => out.push(ServiceEvent::FileOpFailed { id, host: host.clone(), error }),
        }
        if let Some(path) = self.conns.get(&host).and_then(|c| c.path.clone()).filter(|p| op.relists(p, ok)) {
            self.list_now(&host, path, out);
        }
    }

    /// A transfer command: the engine runs it to rest, and a folder on screen that a finished
    /// upload changed is listed again (as the service's poll would notice).
    fn transfer(&mut self, cmd: TransferCommand, out: &mut Vec<ServiceEvent>) {
        let up: BTreeSet<HostId> = self.conns.keys().filter(|h| self.up(h)).cloned().collect();
        let (snapshot, touched) = self.transfers.command(cmd, &up);
        out.push(snapshot);
        for (host, folder) in touched {
            if up.contains(&host) && self.conns.get(&host).and_then(|c| c.path.as_ref()) == Some(&folder) {
                self.list_now(&host, folder, out);
            }
        }
    }

    fn next_id(&mut self) -> PromptId {
        self.last_prompt += 1;
        PromptId(self.last_prompt)
    }

    fn spec(&self, host: &HostId) -> Option<&HostSpec> {
        self.fixture.host(host.as_str())
    }

    /// `user@hostname` as ssh names the account.
    fn account(&self, host: &HostId) -> (String, String) {
        let spec = self.spec(host);
        let user = spec.and_then(|h| h.user.clone()).unwrap_or_else(|| "user".into());
        let hostname = spec.and_then(|h| h.hostname.clone()).unwrap_or_else(|| host.as_str().to_owned());
        (user, hostname)
    }

    fn log(host: &HostId, line: impl Into<String>, out: &mut Vec<ServiceEvent>) {
        out.push(ServiceEvent::Log { host: host.clone(), line: line.into() });
    }

    /// Runs `first` and everything it leads to for `host`.
    fn drive(&mut self, host: &HostId, first: Work, out: &mut Vec<ServiceEvent>) {
        let mut queue = VecDeque::from([first]);
        while let Some(work) = queue.pop_front() {
            match work {
                Work::Life(event) => {
                    let effects = lifecycle::step(&mut self.conns.entry(host.clone()).or_default().life, event);
                    for fx in effects {
                        self.effect(host, fx, &mut queue, out);
                    }
                }
                Work::Advance => self.advance(host, &mut queue, out),
                Work::Answer(answer) => self.answer(host, answer, &mut queue, out),
                Work::Judge(answer) => self.judge(host, answer, &mut queue, out),
            }
        }
    }

    fn effect(&mut self, host: &HostId, fx: lifecycle::Effect, queue: &mut VecDeque<Work>, out: &mut Vec<ServiceEvent>) {
        use lifecycle::Effect as E;
        match fx {
            E::Spawn => {
                let line = self.ssh_line(host);
                let conn = self.conns.entry(host.clone()).or_default();
                conn.ended = None;
                conn.attempt =
                    Some(Attempt { auth: AuthAttempt::new(host.clone()), step: 0, asked: 0, wrong: 0, waiting: None, notices: Vec::new() });
                Self::log(host, line, out);
                queue.push_back(Work::Advance);
            }
            E::Kill => {
                if let Some(a) = self.conns.get_mut(host).and_then(|c| c.attempt.take()) {
                    close_prompts(&a, out);
                }
            }
            E::Status(status) => out.push(ServiceEvent::Status { host: host.clone(), status }),
            E::Opened => {
                let start = self.fixture.server(host.as_str()).map_or_else(RemotePath::root, |s| s.home.clone());
                if let Some(c) = self.conns.get_mut(host) {
                    c.path = None;
                }
                out.push(ServiceEvent::Opened { host: host.clone(), start });
            }
            E::Reopened => {
                if let Some(path) = self.conns.get(host).and_then(|c| c.path.clone()) {
                    self.list_now(host, path, out);
                }
            }
            // No clock to wait on: say so, as the service would, and stay reconnecting.
            E::Retry { after, .. } => Self::log(host, format!("Reconnecting in {} s.", after.as_secs()), out),
            E::AuthFailed => {
                if let Some(auth) = self.conns.get_mut(host).and_then(|c| c.ended.as_mut()) {
                    auth.on_auth_failed(&mut self.store);
                }
            }
            E::ForgetSecrets => self.store.clear(host),
            E::Log(line) => Self::log(host, line, out),
        }
    }

    /// The command line the service would run for `host`, for the log.
    fn ssh_line(&self, host: &HostId) -> String {
        let spec = self.spec(host).cloned().unwrap_or_else(|| HostSpec::from_config_alias(host.as_str()));
        let env = AskpassEnv { exe: "dromon".into(), endpoint: String::new(), token: String::new() };
        let args = match SshCommand::sftp(&spec, &env, false) {
            Ok(cmd) => cmd.args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" "),
            Err(_) => format!("-s {host} sftp"),
        };
        format!("Running ssh {args}")
    }

    /// Asks the current step, or ends the attempt when none is left.
    fn advance(&mut self, host: &HostId, queue: &mut VecDeque<Work>, out: &mut Vec<ServiceEvent>) {
        let fixture = self.fixture.clone();
        let steps = fixture.server(host.as_str()).map_or(&[][..], |s| &s.auth);
        let (user, hostname) = self.account(host);
        let port = self.spec(host).and_then(|h| h.port);
        loop {
            let Some(conn) = self.conns.get_mut(host) else { return };
            let Some(a) = conn.attempt.as_mut() else { return };
            if a.waiting.is_some() {
                return;
            }
            let Some(step) = steps.get(a.step) else {
                let reason = match fixture.server(host.as_str()) {
                    Some(s) => s.fail.clone(),
                    None => Some(format!("ssh: Could not resolve hostname {hostname}: Name or service not known")),
                };
                match reason {
                    Some(reason) => self.exit(host, vec![reason], queue, out),
                    None => {
                        let a = conn.attempt.as_mut().expect("checked above");
                        for id in a.notices.drain(..) {
                            out.push(ServiceEvent::PromptClosed(id));
                        }
                        Self::log(host, "SFTP session started.", out);
                        queue.push_back(Work::Life(Event::Up));
                    }
                }
                return;
            };
            let (kind, text) = match step {
                Step::HostKey { .. } if conn.trusted => {
                    a.step += 1;
                    a.asked = 0;
                    continue;
                }
                Step::Notice { text } => {
                    let text = text.clone();
                    let id = self.next_id();
                    if let Some(a) = self.conns.get_mut(host).and_then(|c| c.attempt.as_mut()) {
                        a.notices.push(id);
                        a.step += 1;
                        a.asked = 0;
                    }
                    Self::log(host, format!("ssh asks “{}”", first_line(&text)), out);
                    out.push(ServiceEvent::Prompt(Prompt { id, host: host.clone(), kind: PromptKind::Notice, text, error: None }));
                    continue;
                }
                Step::HostKey { key_type, fingerprint } => {
                    let name = match port {
                        Some(p) if p != 22 => format!("[{hostname}]:{p}"),
                        _ => hostname.clone(),
                    };
                    let text = format!(
                        "The authenticity of host '{name}' can't be established.\n{key_type} key fingerprint is {fingerprint}.\n\
                         This key is not known by any other names.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? "
                    );
                    (PromptKind::HostKey { key_type: Some(key_type.clone()), fingerprint: Some(fingerprint.clone()) }, text)
                }
                Step::Password { prompt, .. } => (PromptKind::Password, prompt.clone().unwrap_or_else(|| format!("{user}@{hostname}'s password: "))),
                Step::Passphrase { key_path, .. } => {
                    let text = if a.asked == 0 {
                        format!("Enter passphrase for key '{key_path}': ")
                    } else {
                        format!("Bad passphrase, try again for {key_path}: ")
                    };
                    (PromptKind::Passphrase { key_path: Some(key_path.clone()) }, text)
                }
                Step::Code { prompt, .. } => (PromptKind::Other, prompt.clone().unwrap_or_else(|| format!("({user}@{hostname}) Verification code: "))),
                Step::Confirm { text } => (PromptKind::Confirm, text.clone()),
            };
            a.asked += 1;
            match a.auth.on_prompt(&kind, &text, &mut self.store) {
                AuthDecision::AnswerFromMemory(secret) => {
                    Self::log(host, format!("ssh asked “{}”: answered from memory.", first_line(&text)), out);
                    queue.push_back(Work::Judge(PromptAnswer::Text(secret.expose_secret().to_owned())));
                }
                AuthDecision::AskUser { error } => {
                    let id = self.next_id();
                    if let Some(a) = self.conns.get_mut(host).and_then(|c| c.attempt.as_mut()) {
                        a.waiting = Some(Open { id, kind: kind.clone(), text: text.clone() });
                    }
                    Self::log(host, format!("ssh asks “{}”", first_line(&text)), out);
                    out.push(ServiceEvent::Prompt(Prompt { id, host: host.clone(), kind, text, error }));
                    queue.push_back(Work::Life(Event::PromptOpened));
                }
            }
            return;
        }
    }

    /// The user answered the open prompt: remember it as the service does, then judge it.
    fn answer(&mut self, host: &HostId, answer: PromptAnswer, queue: &mut VecDeque<Work>, out: &mut Vec<ServiceEvent>) {
        let Some(a) = self.conns.get_mut(host).and_then(|c| c.attempt.as_mut()) else { return wipe(answer) };
        let Some(open) = a.waiting.take() else { return wipe(answer) };
        a.auth.on_user_answer(&open.kind, &open.text, &answer, &mut self.store);
        Self::log(host, "Answered ssh's prompt.", out);
        queue.push_back(Work::Life(Event::PromptClosed));
        queue.push_back(Work::Judge(answer));
    }

    /// What the server makes of `answer` to the current step.
    fn judge(&mut self, host: &HostId, answer: PromptAnswer, queue: &mut VecDeque<Work>, out: &mut Vec<ServiceEvent>) {
        let fixture = self.fixture.clone();
        let steps = fixture.server(host.as_str()).map_or(&[][..], |s| &s.auth);
        let (user, hostname) = self.account(host);
        let Some(conn) = self.conns.get_mut(host) else { return wipe(answer) };
        let Some(a) = conn.attempt.as_mut() else { return wipe(answer) };
        let Some(step) = steps.get(a.step) else { return wipe(answer) };
        let denied = |methods: &str| format!("{user}@{hostname}: Permission denied ({methods}).");
        let refusal = match step {
            Step::HostKey { .. } => "Host key verification failed.".to_owned(),
            Step::Password { .. } => denied("publickey,password"),
            Step::Code { .. } => denied("keyboard-interactive"),
            Step::Passphrase { .. } | Step::Confirm { .. } | Step::Notice { .. } => denied("publickey"),
        };
        let PromptAnswer::Text(mut given) = answer else {
            return self.exit(host, vec![refusal], queue, out);
        };
        let right = match step {
            Step::HostKey { fingerprint, .. } => given == "yes" || given == *fingerprint,
            Step::Confirm { .. } => given == "yes",
            Step::Password { accept, .. } | Step::Passphrase { accept, .. } | Step::Code { accept, .. } => given == *accept,
            Step::Notice { .. } => true,
        };
        given.zeroize();
        if right {
            if let Step::HostKey { key_type, .. } = step {
                conn.trusted = true;
                Self::log(host, format!("Warning: Permanently added '{hostname}' ({key_type}) to the list of known hosts."), out);
            }
            a.step += 1;
            a.asked = 0;
            queue.push_back(Work::Advance);
            return;
        }
        a.wrong += 1;
        let retries = matches!(step, Step::Password { .. } | Step::Passphrase { .. } | Step::Code { .. });
        if !retries || a.wrong >= TRIES {
            return self.exit(host, vec![refusal], queue, out);
        }
        match step {
            Step::Password { .. } => Self::log(host, "Permission denied, please try again.", out),
            // keyboard-interactive starts the round over: back to its password, if it has one.
            Step::Code { .. } => {
                if let Some(pw) = steps[..a.step].iter().rposition(|s| matches!(s, Step::Password { .. })) {
                    a.step = pw;
                    a.asked = 0;
                }
            }
            _ => {}
        }
        queue.push_back(Work::Advance);
    }

    /// ssh exits with `stderr`: its last words, then the lifecycle decides.
    fn exit(&mut self, host: &HostId, stderr: Vec<String>, queue: &mut VecDeque<Work>, out: &mut Vec<ServiceEvent>) {
        for line in &stderr {
            Self::log(host, line.clone(), out);
        }
        Self::log(host, "ssh exited (exit status: 255).", out);
        if let Some(conn) = self.conns.get_mut(host)
            && let Some(a) = conn.attempt.take()
        {
            close_prompts(&a, out);
            conn.ended = Some(a.auth);
        }
        queue.push_back(Work::Life(Event::Exited { stderr, error: None }));
    }

    fn list(&mut self, host: &HostId, path: RemotePath, out: &mut Vec<ServiceEvent>) {
        let Some(conn) = self.conns.get_mut(host) else { return };
        // As the service: no session up, nothing to list with.
        if conn.life.phase != Phase::Ready || conn.attempt.is_none() {
            return;
        }
        conn.path = Some(path.clone());
        self.list_now(host, path, out);
    }

    fn list_now(&mut self, host: &HostId, path: RemotePath, out: &mut Vec<ServiceEvent>) {
        match self.changes.list(&self.transfers, host, &path) {
            Ok(entries) => out.extend(pages(host, &path, entries)),
            Err(Error::ConnectionLost) => self.drive(host, Work::Life(Event::Lost), out),
            Err(error) => out.push(ServiceEvent::ListFailed { host: host.clone(), path, error }),
        }
    }

    /// The host whose open prompt is `id`.
    fn owner(&self, id: PromptId) -> Option<HostId> {
        self.conns.iter().find(|(_, c)| c.attempt.as_ref().and_then(|a| a.waiting.as_ref()).is_some_and(|o| o.id == id)).map(|(h, _)| h.clone())
    }
}

/// Every prompt of an attempt that's going away: the UI drops their sheets and toasts.
fn close_prompts(a: &Attempt, out: &mut Vec<ServiceEvent>) {
    for id in a.waiting.iter().map(|o| o.id).chain(a.notices.iter().copied()) {
        out.push(ServiceEvent::PromptClosed(id));
    }
}

/// A listing as the service streams it: pages of [`PAGE`] entries, then an empty last page.
pub fn pages(host: &HostId, path: &RemotePath, entries: Vec<dromon_core::Entry>) -> Vec<ServiceEvent> {
    let mut out = Vec::new();
    let mut rest = entries.into_iter().peekable();
    while rest.peek().is_some() {
        let page: Vec<_> = rest.by_ref().take(PAGE).collect();
        out.push(ServiceEvent::ListingPage { host: host.clone(), path: path.clone(), entries: page, first: out.is_empty(), done: false });
    }
    out.push(ServiceEvent::ListingPage { host: host.clone(), path: path.clone(), entries: Vec::new(), first: out.is_empty(), done: true });
    out
}

impl Backend for FixtureBackend {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        let mut out = Vec::new();
        match cmd {
            Command::Connect { host } => {
                if self.spec(&host).is_none() {
                    let status = ConnStatus::Failed("Dromon doesn't know this host any more.".into());
                    out.push(ServiceEvent::Status { host, status });
                } else {
                    self.drive(&host, Work::Life(Event::Connect), &mut out);
                }
            }
            Command::List { host, path } => self.list(&host, path, &mut out),
            Command::Disconnect { host } => {
                if self.conns.contains_key(&host) {
                    self.drive(&host, Work::Life(Event::Disconnect), &mut out);
                } else {
                    self.store.clear(&host);
                    out.push(ServiceEvent::Status { host, status: ConnStatus::Disconnected });
                }
            }
            Command::AnswerPrompt { id, answer } => match self.owner(id) {
                Some(host) => self.drive(&host, Work::Answer(answer), &mut out),
                None => wipe(answer),
            },
            Command::Transfer(cmd) => self.transfer(cmd, &mut out),
            Command::FileOp { id, host, op } => self.file_op(id, host, op, &mut out),
        }
        out
    }
}
