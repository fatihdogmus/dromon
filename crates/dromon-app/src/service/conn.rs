//! One host's connection task: owns its ssh process, SFTP session, askpass prompts and the
//! [`Lifecycle`] machine, and carries out the machine's effects. Everything about the host
//! happens here, in order, on one task.
//!
//! A [`Session`] is one connection attempt: a fresh askpass token, a fresh `AuthAttempt`, the
//! ssh child, the SFTP handshake and then the session, and the folder listing in flight.
//! Dropping it unregisters the token, cancels its open prompts and kills ssh.

use std::collections::VecDeque;
use std::future::pending;
use std::io;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dromon_core::{Entry, Error, HostId, HostSpec, PromptAnswer, PromptId, RemotePath};
use dromon_sftp::Sftp;
use dromon_ssh::{AskpassEnv, AskpassRequest, AskpassServer, PasswordStore, SshCommand, spawn};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinError, JoinHandle};
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::auth::{self, AttemptPrompts, PromptIds, Routed};
use super::lifecycle::{Effect, Event, Lifecycle, Phase, step};
use super::{Config, Sink};
use crate::state::ServiceEvent;

/// Entries per listing page (§4.4).
pub const PAGE: usize = 512;
/// stderr lines kept per attempt for reading why ssh exited.
const TAIL: usize = 64;
/// How long to wait for ssh's last stderr lines once it exited.
const DRAIN: Duration = Duration::from_millis(300);

/// What the manager sends a connection.
pub enum ConnCmd {
    Connect(HostSpec),
    List(RemotePath),
    Answer(PromptId, PromptAnswer),
    Disconnect,
}

/// What the connection tasks share: configuration, the askpass endpoint, the password store
/// and prompt ids.
#[derive(Clone)]
pub struct Shared {
    pub cfg: Arc<Config>,
    pub askpass: Option<Arc<AskpassServer>>,
    pub store: Arc<Mutex<PasswordStore>>,
    pub ids: PromptIds,
}

/// The manager's end of a connection task.
pub struct ConnHandle {
    pub cmds: mpsc::UnboundedSender<ConnCmd>,
    pub asks: mpsc::UnboundedSender<AskpassRequest>,
}

/// Starts the task for `spec`'s host.
pub fn start<S: Sink>(spec: HostSpec, shared: Shared, sink: S) -> ConnHandle {
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    let (asks, ask_rx) = mpsc::unbounded_channel();
    let task = Task { host: spec.id.clone(), spec, shared, sink, life: Lifecycle::default(), session: None, retry: None, path: None };
    tokio::spawn(task.run(cmd_rx, ask_rx));
    ConnHandle { cmds, asks }
}

/// A folder being listed.
struct ListRun {
    path: RemotePath,
    pages: mpsc::Receiver<Result<Vec<Entry>, Error>>,
    first: bool,
}

/// One connection attempt.
struct Session {
    askpass: Option<Arc<AskpassServer>>,
    token: Zeroizing<String>,
    prompts: AttemptPrompts,
    stderr: Option<mpsc::Receiver<String>>,
    tail: VecDeque<String>,
    /// Resolves when ssh exits; dropping `kill` kills it.
    exit: Option<JoinHandle<io::Result<ExitStatus>>>,
    _kill: oneshot::Sender<()>,
    handshake: Option<JoinHandle<Result<(Sftp, RemotePath), Error>>>,
    sftp: Option<Sftp>,
    home: RemotePath,
    listing: Option<ListRun>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(server) = &self.askpass {
            server.unregister(&self.token);
        }
        if let Some(h) = &self.handshake {
            h.abort();
        }
        if let Some(sftp) = &self.sftp {
            sftp.close();
        }
    }
}

enum SessionEvent {
    Stderr(String),
    StderrClosed,
    Exited(Result<io::Result<ExitStatus>, JoinError>),
    Up(Result<Result<(Sftp, RemotePath), Error>, JoinError>),
    HungUp(PromptId),
    Page(Option<Result<Vec<Entry>, Error>>),
}

async fn recv_opt<T>(rx: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}

async fn join_opt<T>(h: &mut Option<JoinHandle<T>>) -> Result<T, JoinError> {
    match h {
        Some(h) => h.await,
        None => pending().await,
    }
}

async fn page_opt(run: &mut Option<ListRun>) -> Option<Result<Vec<Entry>, Error>> {
    match run {
        Some(run) => run.pages.recv().await,
        None => pending().await,
    }
}

impl Session {
    /// Starts ssh for `spec` with a fresh token, and the SFTP handshake on its stdio.
    async fn start(spec: &HostSpec, shared: &Shared) -> Result<(Session, String), String> {
        let cfg = &shared.cfg;
        let (env, askpass) = match &shared.askpass {
            Some(server) => {
                let env = server.register(spec.id.clone(), &cfg.helper_exe).map_err(|e| format!("Couldn't set up sign-in prompts: {e}"))?;
                (env, Some(server.clone()))
            }
            // No askpass endpoint: ssh's prompts are cancelled, keys and agents still work.
            None => (AskpassEnv { exe: cfg.helper_exe.clone(), endpoint: String::new(), token: String::new() }, None),
        };
        let token = Zeroizing::new(env.token.clone());
        let unregister = |askpass: &Option<Arc<AskpassServer>>| {
            if let Some(server) = askpass {
                server.unregister(&token);
            }
        };
        let built = SshCommand::sftp(spec, &env, cfg.verbose).and_then(|cmd| match &cfg.config_file {
            Some(file) => cmd.with_config_file(file),
            None => Ok(cmd),
        });
        let mut cmd = match built {
            Ok(cmd) => cmd,
            Err(e) => {
                unregister(&askpass);
                return Err(e.to_string());
            }
        };
        cmd.program = cfg.ssh.clone();
        let line = std::iter::once(cmd.program.to_string_lossy().into_owned())
            .chain(cmd.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" ");
        let mut child = match spawn(&cmd).await {
            Ok(child) => child,
            Err(e) => {
                unregister(&askpass);
                return Err(format!("Couldn't run {}: {e}", cmd.program.display()));
            }
        };
        let stream = child.take_stream();
        let stderr = child.take_stderr();
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
        let handshake = stream.map(|stream| {
            tokio::spawn(async move {
                let sftp = Sftp::new(stream).await?;
                let home = sftp.home().await?;
                Ok((sftp, home))
            })
        });
        let session = Session {
            askpass,
            token,
            prompts: AttemptPrompts::new(spec.id.clone(), shared.ids.clone()),
            stderr,
            tail: VecDeque::new(),
            exit: Some(exit),
            _kill: kill,
            handshake,
            sftp: None,
            home: RemotePath::root(),
            listing: None,
        };
        Ok((session, format!("Running {line}")))
    }

    async fn next(&mut self) -> SessionEvent {
        tokio::select! {
            line = recv_opt(&mut self.stderr) => match line {
                Some(line) => SessionEvent::Stderr(line),
                None => SessionEvent::StderrClosed,
            },
            status = join_opt(&mut self.exit) => SessionEvent::Exited(status),
            up = join_opt(&mut self.handshake) => SessionEvent::Up(up),
            id = self.prompts.hung_up() => SessionEvent::HungUp(id),
            page = page_opt(&mut self.listing) => SessionEvent::Page(page),
        }
    }

    fn keep(&mut self, line: &str) {
        if self.tail.len() == TAIL {
            self.tail.pop_front();
        }
        self.tail.push_back(line.to_owned());
    }
}

async fn session_next(s: &mut Option<Session>) -> SessionEvent {
    match s {
        Some(s) => s.next().await,
        None => pending().await,
    }
}

async fn retry_due(retry: &Option<(u32, Instant)>) -> u32 {
    match retry {
        Some((attempt, at)) => {
            tokio::time::sleep_until(*at).await;
            *attempt
        }
        None => pending().await,
    }
}

enum Input {
    Cmd(ConnCmd),
    Ask(AskpassRequest),
    Session(SessionEvent),
    Retry(u32),
}

struct Task<S: Sink> {
    host: HostId,
    spec: HostSpec,
    shared: Shared,
    sink: S,
    life: Lifecycle,
    session: Option<Session>,
    retry: Option<(u32, Instant)>,
    /// The folder on screen, listed again after a reconnect.
    path: Option<RemotePath>,
}

impl<S: Sink> Task<S> {
    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<ConnCmd>, mut asks: mpsc::UnboundedReceiver<AskpassRequest>) {
        loop {
            let input = tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => Input::Cmd(cmd),
                    None => break,
                },
                Some(req) = asks.recv() => Input::Ask(req),
                ev = session_next(&mut self.session) => Input::Session(ev),
                attempt = retry_due(&self.retry) => Input::Retry(attempt),
            };
            match input {
                Input::Cmd(cmd) => self.on_command(cmd).await,
                Input::Ask(req) => self.on_ask(req).await,
                Input::Session(ev) => self.on_session(ev).await,
                Input::Retry(attempt) => {
                    self.retry = None;
                    self.drive(Event::RetryDue { attempt }, None).await;
                }
            }
        }
        // The app is quitting: the session drops, killing ssh.
    }

    fn emit(&self, ev: ServiceEvent) {
        self.sink.emit(ev);
    }

    fn log(&self, line: impl Into<String>) {
        self.emit(ServiceEvent::Log { host: self.host.clone(), line: line.into() });
    }

    async fn on_command(&mut self, cmd: ConnCmd) {
        match cmd {
            ConnCmd::Connect(spec) => {
                if matches!(self.life.phase, Phase::Idle | Phase::Failed(_) | Phase::Disconnected) {
                    self.spec = spec;
                }
                self.drive(Event::Connect, None).await;
            }
            ConnCmd::List(path) => {
                self.path = Some(path.clone());
                self.list(path);
            }
            ConnCmd::Answer(id, answer) => {
                let store = self.shared.store.clone();
                let answered = match &mut self.session {
                    Some(s) => s.prompts.on_answer(id, answer, &store),
                    None => {
                        auth::wipe(answer);
                        false
                    }
                };
                if answered {
                    self.log("Answered ssh's prompt.");
                    self.drive(Event::PromptClosed, None).await;
                }
            }
            ConnCmd::Disconnect => self.drive(Event::Disconnect, None).await,
        }
    }

    async fn on_ask(&mut self, req: AskpassRequest) {
        let Some(s) = &mut self.session else { return };
        let first_line = req.prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_owned();
        match s.prompts.on_request(req, &self.shared.store) {
            Routed::FromMemory => self.log(format!("ssh asked “{first_line}”: answered from memory.")),
            Routed::Asked { prompt, needs_user } => {
                self.log(format!("ssh asks “{first_line}”"));
                self.emit(ServiceEvent::Prompt(prompt));
                if needs_user {
                    self.drive(Event::PromptOpened, None).await;
                }
            }
            Routed::Dropped => {}
        }
    }

    async fn on_session(&mut self, ev: SessionEvent) {
        let Some(s) = &mut self.session else { return };
        match ev {
            SessionEvent::Stderr(line) => {
                s.keep(&line);
                self.log(line);
            }
            SessionEvent::StderrClosed => s.stderr = None,
            SessionEvent::HungUp(id) => {
                let needs_user = s.prompts.remove(id);
                self.emit(ServiceEvent::PromptClosed(id));
                if needs_user == Some(true) {
                    self.drive(Event::PromptClosed, None).await;
                }
            }
            SessionEvent::Up(Ok(Ok((sftp, home)))) => {
                s.handshake = None;
                s.sftp = Some(sftp);
                s.home = home;
                self.log("SFTP session started.");
                self.drive(Event::Up, None).await;
            }
            SessionEvent::Up(Ok(Err(e))) => {
                s.handshake = None;
                self.finish(Some(e.to_string())).await;
            }
            SessionEvent::Up(Err(_)) => s.handshake = None,
            SessionEvent::Exited(status) => {
                s.exit = None;
                let note = match status {
                    Ok(Ok(st)) => format!("ssh exited ({st})."),
                    Ok(Err(e)) => format!("ssh exited: {e}."),
                    Err(_) => "ssh's watcher stopped.".to_owned(),
                };
                self.log(note);
                self.finish(None).await;
            }
            SessionEvent::Page(page) => self.on_page(page).await,
        }
    }

    async fn on_page(&mut self, page: Option<Result<Vec<Entry>, Error>>) {
        let Some(s) = &mut self.session else { return };
        let Some(run) = &mut s.listing else { return };
        let (host, path) = (self.host.clone(), run.path.clone());
        match page {
            Some(Ok(entries)) => {
                let first = std::mem::replace(&mut run.first, false);
                self.emit(ServiceEvent::ListingPage { host, path, entries, first, done: false });
            }
            None => {
                let first = run.first;
                s.listing = None;
                self.emit(ServiceEvent::ListingPage { host, path, entries: Vec::new(), first, done: true });
            }
            Some(Err(Error::ConnectionLost)) => {
                s.listing = None;
                self.drive(Event::Lost, None).await;
            }
            Some(Err(error)) => {
                s.listing = None;
                self.emit(ServiceEvent::ListFailed { host, path, error });
            }
        }
    }

    /// Lists `path` if the session is up (a newer listing replaces one in flight).
    fn list(&mut self, path: RemotePath) {
        let Some(s) = &mut self.session else { return };
        let Some(sftp) = &s.sftp else { return };
        s.listing = Some(ListRun { pages: sftp.list_paged(&path, PAGE), path, first: true });
    }

    /// ssh is gone (or SFTP couldn't start): collects its last words and tells the machine.
    async fn finish(&mut self, error: Option<String>) {
        let Some(mut s) = self.session.take() else { return };
        for id in s.prompts.close_all() {
            self.emit(ServiceEvent::PromptClosed(id));
        }
        if let Some(mut rx) = s.stderr.take() {
            while let Ok(Some(line)) = tokio::time::timeout(DRAIN, rx.recv()).await {
                s.keep(&line);
                self.log(line);
            }
        }
        if let Some(e) = &error {
            self.log(e.clone());
        }
        let stderr: Vec<String> = s.tail.iter().cloned().collect();
        self.drive(Event::Exited { stderr, error }, Some(s)).await;
    }

    /// Steps the machine and carries out its effects, feeding follow-up events back in.
    /// `ended` is the session that just finished, for `AuthFailed`.
    async fn drive(&mut self, event: Event, mut ended: Option<Session>) {
        let mut events = VecDeque::from([event]);
        while let Some(event) = events.pop_front() {
            for fx in step(&mut self.life, event) {
                match fx {
                    Effect::Spawn => {
                        self.session = None;
                        match Session::start(&self.spec, &self.shared).await {
                            Ok((session, note)) => {
                                self.session = Some(session);
                                self.log(note);
                            }
                            Err(e) => events.push_back(Event::Exited { stderr: Vec::new(), error: Some(e) }),
                        }
                    }
                    Effect::Kill => {
                        if let Some(mut s) = self.session.take() {
                            for id in s.prompts.close_all() {
                                self.emit(ServiceEvent::PromptClosed(id));
                            }
                        }
                    }
                    Effect::Status(status) => self.emit(ServiceEvent::Status { host: self.host.clone(), status }),
                    Effect::Opened => {
                        let start = self.session.as_ref().map_or_else(RemotePath::root, |s| s.home.clone());
                        self.path = None;
                        self.emit(ServiceEvent::Opened { host: self.host.clone(), start });
                    }
                    Effect::Reopened => {
                        if let Some(path) = self.path.clone() {
                            self.list(path);
                        }
                    }
                    Effect::Retry { attempt, after } => {
                        self.retry = Some((attempt, Instant::now() + after));
                        self.log(format!("Reconnecting in {} s.", after.as_secs()));
                    }
                    Effect::AuthFailed => {
                        if let Some(s) = &mut ended {
                            s.prompts.auth_failed(&self.shared.store);
                        }
                    }
                    Effect::ForgetSecrets => auth::lock(&self.shared.store).clear(&self.host),
                    Effect::Log(line) => self.log(line),
                }
            }
        }
    }
}
