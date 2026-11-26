//! One host's connection task: owns its ssh process, SFTP session, askpass prompts and the
//! [`Lifecycle`] machine, and carries out the machine's effects. Everything about the host
//! happens here, in order, on one task.
//!
//! A [`Session`] is one connection attempt: a fresh askpass token, a fresh `AuthAttempt`, the
//! ssh child, the SFTP handshake and then the session, and the folder listing in flight.
//! Dropping it unregisters the token, cancels its open prompts and kills ssh.
//!
//! The host is one connection for every main window (`windows.rs`): each window's folder on
//! screen here is listed for it, polled while it shows, and listed again after a reconnect or
//! an operation that changed it. One listing runs at a time: one a window waits for finishes
//! before the next starts (a poll gives way at once).
//!
//! The checks after a wake or a window's focus, and verbose logging, are `conn_checks.rs`.

#[path = "conn_checks.rs"]
mod checks;

use std::collections::VecDeque;
use std::future::pending;
use std::io;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dromon_core::{ConnStatus, Entry, Error, HostId, HostSpec, PromptAnswer, PromptId, RemotePath};
use dromon_sftp::{Sftp, Visibility};
use dromon_ssh::{AskpassEnv, AskpassRequest, AskpassServer, PasswordStore, SshCommand, spawn};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinError, JoinHandle};
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::auth::{self, AttemptPrompts, PromptIds, Routed};
use super::fileops::{FileOps, Finished};
use super::lifecycle::{Effect, Event, Lifecycle, Phase, step};
use super::poll::{Collector, Outcome};
use super::transfers::EngineMsg;
use super::wake::{ProbeGate, Why};
use super::windows::{Route, Watch, status_of};
use super::{Config, Sink};
use crate::state::{FileOp, OpId, ServiceEvent, WindowId};

/// Entries per listing page (§4.4).
pub const PAGE: usize = 512;
/// stderr lines kept per attempt for reading why ssh exited.
const TAIL: usize = 64;
/// How long to wait for ssh's last stderr lines once it exited.
const DRAIN: Duration = Duration::from_millis(300);

/// What the manager sends a connection; the window is the one the command came from.
pub enum ConnCmd {
    Connect(WindowId, HostSpec),
    List(WindowId, RemotePath),
    Answer(WindowId, PromptId, PromptAnswer),
    Disconnect,
    FileOp(OpId, FileOp),
    /// How the window shows now (`windows.rs`).
    Window(WindowId, Visibility),
    /// Poll the window's folder on screen now, if it's polled at all (it gained focus).
    PollNow(WindowId),
    /// The window closed.
    Closed(WindowId),
    /// The computer woke from sleep (`wake.rs`).
    Wake,
    /// A window gained focus: check the session, at most every 30 s.
    Focus,
    /// Run ssh with `-v` or without from now on (`Command::Verbose`).
    Verbose(bool),
}

/// What the connection tasks share: configuration, the askpass endpoint, the password store,
/// prompt ids, and the transfer engine (told when a session comes and goes).
#[derive(Clone)]
pub struct Shared {
    pub cfg: Arc<Config>,
    pub askpass: Option<Arc<AskpassServer>>,
    pub store: Arc<Mutex<PasswordStore>>,
    pub ids: PromptIds,
    pub engine: mpsc::UnboundedSender<EngineMsg>,
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
    let task = Task {
        host: spec.id.clone(),
        spec,
        verbose: shared.cfg.verbose,
        probes: ProbeGate::default(),
        probe: None,
        shared,
        sink,
        life: Lifecycle::default(),
        session: None,
        retry: None,
        watch: Watch::default(),
        waiting: VecDeque::new(),
        route: Route::default(),
        ops: FileOps::default(),
        epoch: 0,
    };
    tokio::spawn(task.run(cmd_rx, ask_rx));
    ConnHandle { cmds, asks }
}

/// A folder being listed: for the UI (pages go on as they come), or a poll (pages collect
/// quietly and the whole is compared with what the UI has). One per host at a time.
struct ListRun {
    path: RemotePath,
    pages: mpsc::Receiver<Result<Vec<Entry>, Error>>,
    first: bool,
    poll: bool,
    started: Instant,
    got: Collector,
}

impl ListRun {
    fn new(sftp: &Sftp, path: RemotePath, poll: bool) -> ListRun {
        ListRun { pages: sftp.list_paged(&path, PAGE), path, first: true, poll, started: Instant::now(), got: Collector::default() }
    }
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
    /// Starts ssh for `spec` (with `-v` if `verbose`) with a fresh token, and the SFTP handshake
    /// on its stdio.
    async fn start(spec: &HostSpec, shared: &Shared, verbose: bool) -> Result<(Session, String), String> {
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
        let built = SshCommand::sftp(spec, &env, verbose).and_then(|cmd| match &cfg.config_file {
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
        for name in &cfg.without_env {
            cmd = cmd.without_env(name.clone());
        }
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

async fn sleep_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => pending().await,
    }
}

enum Input {
    Cmd(ConnCmd),
    Ask(AskpassRequest),
    Session(SessionEvent),
    Retry(u32),
    OpEnded(Finished),
    PollDue,
    Probed(checks::Probed),
}

struct Task<S: Sink> {
    host: HostId,
    spec: HostSpec,
    /// ssh runs with `-v` (the log's "Verbose logging").
    verbose: bool,
    /// When the session is checked, and the check running (`conn_checks.rs`).
    probes: ProbeGate,
    probe: Option<checks::Probe>,
    shared: Shared,
    sink: S,
    life: Lifecycle,
    session: Option<Session>,
    retry: Option<(u32, Instant)>,
    /// Each window's folder on screen, listed again after a reconnect and after a file
    /// operation changed it, and polled (`windows.rs`).
    watch: Watch,
    /// Folders windows asked for while another window's listing ran, in order.
    waiting: VecDeque<RemotePath>,
    /// Which window each prompt goes to.
    route: Route,
    /// New folder, rename and delete, running on the session (`fileops.rs`).
    ops: FileOps,
    /// Counts SFTP sessions that came up: an operation's lost connection only ends the
    /// session it ran on.
    epoch: u64,
}

impl<S: Sink> Task<S> {
    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<ConnCmd>, mut asks: mpsc::UnboundedReceiver<AskpassRequest>) {
        loop {
            let poll_at = self.poll_at();
            let input = tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => Input::Cmd(cmd),
                    None => break,
                },
                Some(req) = asks.recv() => Input::Ask(req),
                ev = session_next(&mut self.session) => Input::Session(ev),
                attempt = retry_due(&self.retry) => Input::Retry(attempt),
                ended = self.ops.next() => Input::OpEnded(ended),
                _ = sleep_opt(poll_at) => Input::PollDue,
                probed = checks::next(&mut self.probe) => Input::Probed(probed),
            };
            match input {
                Input::Cmd(cmd) => self.on_command(cmd).await,
                Input::Ask(req) => self.on_ask(req).await,
                Input::Session(ev) => self.on_session(ev).await,
                Input::Retry(attempt) => {
                    self.retry = None;
                    self.drive(Event::RetryDue { attempt }, None).await;
                }
                Input::OpEnded(ended) => self.on_op_ended(ended).await,
                Input::PollDue => {
                    if let Some(path) = self.watch.next_due() {
                        self.start_poll(path);
                    }
                }
                Input::Probed(probed) => self.on_probed(probed).await,
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

    /// Hands the transfer engine this host's live SFTP session.
    fn session_up(&self) {
        if let Some(sftp) = self.session.as_ref().and_then(|s| s.sftp.clone()) {
            let _ = self.shared.engine.send(EngineMsg::HostUp(self.host.clone(), sftp));
        }
    }

    /// Tells the transfer engine a session it had is gone (`s` is being dropped), and fails
    /// the deletes waiting for it.
    fn session_gone(&mut self, s: &Session) {
        if s.sftp.is_some() {
            let _ = self.shared.engine.send(EngineMsg::HostDown(self.host.clone()));
            self.ops.fail_queued(&self.host, &self.sink);
        }
    }

    /// The live SFTP session, if any.
    fn sftp(&self) -> Option<Sftp> {
        self.session.as_ref().and_then(|s| s.sftp.clone())
    }

    /// Runs a file operation on the live session; without one it fails at once.
    fn file_op(&mut self, id: OpId, op: FileOp) {
        match self.sftp() {
            Some(sftp) => self.ops.start(id, op, &sftp, self.epoch, &self.host, &self.sink),
            None => self.emit(ServiceEvent::FileOpFailed { id, host: self.host.clone(), error: Error::ConnectionLost }),
        }
    }

    /// A file operation ended: says so, lists the folder on screen again if the operation
    /// changed it (never a folder it removed), and starts the next delete waiting. A lost
    /// connection on the session still up ends that session, as a listing's does.
    async fn on_op_ended(&mut self, ended: Finished) {
        let Finished { id, op, epoch, result } = ended;
        let host = self.host.clone();
        let ok = result.is_ok();
        let lost = matches!(result, Err(Error::ConnectionLost));
        match result {
            Ok(()) => self.emit(ServiceEvent::FileOpDone { id, host: host.clone() }),
            Err(error) => self.emit(ServiceEvent::FileOpFailed { id, host: host.clone(), error }),
        }
        let drop_session = lost && epoch == self.epoch && self.sftp().is_some();
        if drop_session {
            self.drive(Event::Lost, None).await;
        }
        if matches!(op, FileOp::Delete { .. }) {
            let sftp = self.sftp();
            self.ops.delete_ended(sftp.as_ref(), self.epoch, &host, &self.sink);
        }
        if !drop_session {
            for path in self.watch.folders().into_iter().filter(|p| op.relists(p, ok)) {
                self.list(path);
            }
        }
    }

    async fn on_command(&mut self, cmd: ConnCmd) {
        match cmd {
            ConnCmd::Connect(window, spec) => self.connect(window, spec).await,
            ConnCmd::List(window, path) => {
                self.watch.show(window, path.clone(), Instant::now());
                self.list(path);
            }
            ConnCmd::Answer(window, id, answer) => {
                let store = self.shared.store.clone();
                let answered = match &mut self.session {
                    Some(s) => s.prompts.on_answer(id, answer, &store),
                    None => {
                        auth::wipe(answer);
                        false
                    }
                };
                if answered {
                    self.route.answered(id, window);
                    // Off every window's screen, whichever showed it.
                    self.emit(ServiceEvent::PromptClosed(id));
                    self.log("Answered ssh's prompt.");
                    self.drive(Event::PromptClosed, None).await;
                }
            }
            ConnCmd::Disconnect => self.drive(Event::Disconnect, None).await,
            ConnCmd::FileOp(id, op) => self.file_op(id, op),
            ConnCmd::Window(window, vis) => {
                self.watch.window(window, vis, Instant::now());
                self.drop_unseen_poll();
            }
            ConnCmd::PollNow(window) => {
                if let Some(path) = self.watch.folder(window).filter(|p| self.watch.may_poll(p)).cloned() {
                    self.start_poll(path);
                }
            }
            ConnCmd::Closed(window) => {
                self.watch.closed(window, Instant::now());
                self.drop_unseen_poll();
                for (prompt, to) in self.route.closed(window, &self.watch) {
                    self.emit_to(to, ServiceEvent::Prompt(prompt));
                }
            }
            ConnCmd::Wake => self.drive(Event::Wake, None).await,
            ConnCmd::Focus => self.probe(Why::Focus),
            ConnCmd::Verbose(on) => self.set_verbose(on).await,
        }
    }

    /// `window` asks to connect: a host that's down starts; one that's up opens in that window
    /// at once; one on its way (back) opens there when it's up.
    async fn connect(&mut self, window: WindowId, spec: HostSpec) {
        if self.verbose {
            self.sink.emit_to(window, ServiceEvent::Verbose { host: self.host.clone(), on: true });
        }
        match self.life.phase {
            Phase::Idle | Phase::Failed(_) | Phase::Disconnected => {
                self.spec = spec;
                self.route.started_by(window);
                self.drive(Event::Connect, None).await;
            }
            Phase::Ready if self.sftp().is_some() => {
                let start = self.session.as_ref().map_or_else(RemotePath::root, |s| s.home.clone());
                self.sink.emit_to(window, ServiceEvent::Opened { host: self.host.clone(), start });
            }
            _ => {
                self.route.joining.insert(window);
                if let Some(status) = status_of(&self.life.phase) {
                    self.sink.emit_to(window, ServiceEvent::Status { host: self.host.clone(), status });
                }
            }
        }
    }

    /// A poll under way of a folder no window on screen shows is dropped, so nothing arrives
    /// unseen.
    fn drop_unseen_poll(&mut self) {
        if let Some(s) = &mut self.session
            && s.listing.as_ref().is_some_and(|r| r.poll && !self.watch.visible(&r.path))
        {
            s.listing = None;
        }
    }

    /// `ev` to window `to`, or to every window.
    fn emit_to(&self, to: Option<WindowId>, ev: ServiceEvent) {
        match to {
            Some(window) => self.sink.emit_to(window, ev),
            None => self.emit(ev),
        }
    }

    /// When the next poll should start: never while the session is down or a listing is
    /// under way or waiting (exactly one per host).
    fn poll_at(&self) -> Option<Instant> {
        let s = self.session.as_ref()?;
        let idle = s.sftp.is_some() && s.listing.is_none() && self.waiting.is_empty() && self.life.phase == Phase::Ready;
        if idle { self.watch.due() } else { None }
    }

    /// Starts a poll of `path`, unless something is being listed already.
    fn start_poll(&mut self, path: RemotePath) {
        let Some(s) = &mut self.session else { return };
        if s.listing.is_some() || self.life.phase != Phase::Ready {
            return;
        }
        if let Some(sftp) = &s.sftp {
            s.listing = Some(ListRun::new(sftp, path, true));
        }
    }

    async fn on_ask(&mut self, req: AskpassRequest) {
        let Some(s) = &mut self.session else { return };
        let first_line = req.prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_owned();
        match s.prompts.on_request(req, &self.shared.store) {
            Routed::FromMemory => self.log(format!("ssh asked “{first_line}”: answered from memory.")),
            Routed::Asked { prompt, needs_user } => {
                self.log(format!("ssh asks “{first_line}”"));
                let to = self.route.target(&self.watch);
                self.route.asked(prompt.clone(), to);
                self.emit_to(to, ServiceEvent::Prompt(prompt));
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
                self.route.gone(id);
                self.emit(ServiceEvent::PromptClosed(id));
                if needs_user == Some(true) {
                    self.drive(Event::PromptClosed, None).await;
                }
            }
            SessionEvent::Up(Ok(Ok((sftp, home)))) => {
                s.handshake = None;
                s.sftp = Some(sftp);
                s.home = home;
                self.epoch += 1;
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
            // A poll's pages wait for the whole folder.
            Some(Ok(entries)) if run.poll => run.got.take(entries),
            Some(Ok(entries)) => {
                run.got.copy(&entries);
                let first = std::mem::replace(&mut run.first, false);
                self.emit(ServiceEvent::ListingPage { host, path, entries, first, done: false });
            }
            None => {
                let Some(run) = s.listing.take() else { return };
                let (now, first) = (Instant::now(), run.first);
                let took = now.saturating_duration_since(run.started);
                if run.poll {
                    match self.watch.polled(&path, Ok(run.got), took, now) {
                        Outcome::Changed(entries) => self.emit(ServiceEvent::Refreshed { host, path, entries }),
                        Outcome::Failed(error) => self.emit(ServiceEvent::ListFailed { host, path, error }),
                        Outcome::Quiet => {}
                    }
                } else {
                    self.watch.listed(&path, run.got, took, now);
                    self.emit(ServiceEvent::ListingPage { host, path, entries: Vec::new(), first, done: true });
                }
                self.list_next();
            }
            Some(Err(Error::ConnectionLost)) => {
                s.listing = None;
                self.drive(Event::Lost, None).await;
            }
            Some(Err(error)) => {
                let Some(run) = s.listing.take() else { return };
                if run.poll {
                    let took = run.started.elapsed();
                    if let Outcome::Failed(error) = self.watch.polled(&path, Err(error), took, Instant::now()) {
                        self.emit(ServiceEvent::ListFailed { host, path, error });
                    }
                } else {
                    self.watch.list_failed(&path, &error);
                    self.emit(ServiceEvent::ListFailed { host, path, error });
                }
                self.list_next();
            }
        }
    }

    /// Lists `path` for the windows if the session is up. It replaces a poll in flight, or a
    /// listing no window waits for any more (or of `path` itself); one another window waits for
    /// finishes first, and `path` waits its turn.
    fn list(&mut self, path: RemotePath) {
        let Some(s) = &mut self.session else { return };
        let Some(sftp) = &s.sftp else { return };
        if let Some(run) = &s.listing
            && !run.poll
            && run.path != path
            && self.watch.shows(&run.path)
        {
            if !self.waiting.contains(&path) {
                self.waiting.push_back(path);
            }
            return;
        }
        self.waiting.retain(|p| *p != path);
        self.watch.listing(&path);
        s.listing = Some(ListRun::new(sftp, path, false));
    }

    /// A listing ended: the next folder a window still waits for.
    fn list_next(&mut self) {
        while let Some(path) = self.waiting.pop_front() {
            if self.watch.shows(&path) {
                return self.list(path);
            }
        }
    }

    /// ssh is gone (or SFTP couldn't start): collects its last words and tells the machine.
    async fn finish(&mut self, error: Option<String>) {
        let Some(mut s) = self.session.take() else { return };
        self.session_gone(&s);
        for id in s.prompts.close_all() {
            self.route.gone(id);
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
                        // An attempt now: no wait for one is left.
                        self.retry = None;
                        if let Some(old) = self.session.take() {
                            self.session_gone(&old);
                        }
                        match Session::start(&self.spec, &self.shared, self.verbose).await {
                            Ok((session, note)) => {
                                self.session = Some(session);
                                self.log(note);
                            }
                            Err(e) => events.push_back(Event::Exited { stderr: Vec::new(), error: Some(e) }),
                        }
                    }
                    Effect::Kill => {
                        if let Some(mut s) = self.session.take() {
                            self.session_gone(&s);
                            for id in s.prompts.close_all() {
                                self.route.gone(id);
                                self.emit(ServiceEvent::PromptClosed(id));
                            }
                        }
                    }
                    Effect::Status(status) => {
                        if matches!(status, ConnStatus::Ready | ConnStatus::Failed(_) | ConnStatus::Disconnected) {
                            self.route.settled();
                        }
                        self.emit(ServiceEvent::Status { host: self.host.clone(), status });
                    }
                    Effect::Opened => {
                        // Every window with the host opens it afresh and lists its folder.
                        let start = self.session.as_ref().map_or_else(RemotePath::root, |s| s.home.clone());
                        self.watch.forget();
                        self.waiting.clear();
                        self.route.joining.clear();
                        self.emit(ServiceEvent::Opened { host: self.host.clone(), start });
                        self.session_up();
                    }
                    Effect::Reopened => {
                        // The windows' folders again; windows that asked meanwhile open now.
                        self.waiting.clear();
                        for path in self.watch.folders() {
                            self.list(path);
                        }
                        let start = self.session.as_ref().map_or_else(RemotePath::root, |s| s.home.clone());
                        for window in std::mem::take(&mut self.route.joining) {
                            self.sink.emit_to(window, ServiceEvent::Opened { host: self.host.clone(), start: start.clone() });
                        }
                        self.session_up();
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
                    Effect::Probe => self.probe(Why::Wake),
                }
            }
        }
    }
}
