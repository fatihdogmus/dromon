//! The service thread (§4.1): a dedicated `dromon-service` thread running a multi-threaded tokio
//! runtime, which discovers hosts, runs one ssh per connected host, answers ssh's prompts
//! through the askpass helper and the UI's sheets, and serves SFTP listings.
//!
//! - UI → service: [`ChannelBackend`] puts each [`Command`] on an unbounded channel, with the
//!   main window it came from (each window has its own backend, [`ChannelBackend::another`]);
//!   sending never blocks the UI.
//! - Service → UI: every [`ServiceEvent`] goes to a [`Sink`], for every main window
//!   ([`Sink::emit`]) or one ([`Sink::emit_to`]: a prompt, a host opening in a window that asked
//!   for it); in the app that is [`RuneSink`], which applies it on the UI thread of each window
//!   with `AppHandle::update_if`.
//!
//! The service is one for all the windows (`windows.rs`): one connection per host, one transfer
//! engine. The manager knows how each window shows, and each host's task which folder each
//! window has on screen there.
//!
//! The manager ([`run`]) routes commands and askpass requests to one task per host
//! (`conn.rs`), whose lifecycle is the pure machine in `lifecycle.rs` and whose prompts go
//! through `auth.rs`; its file operations (new folder, rename, delete) run in `fileops.rs`
//! on its SFTP session, and the Properties sheet's folder size walks in `sizes.rs`. The password store lives here only: cleared per host on Disconnect, and
//! all of it when the UI goes away. `hostfiles.rs` owns the files: discovery and its watcher,
//! the saved hosts' `hosts.conf`, and "Add to SSH config".
//!
//! The manager also watches the clocks for a sleep (`wake.rs`; Rune reports no wake or network
//! change): after one, and when a window gains focus, every live connection checks its session
//! and a reconnect waiting out its backoff tries at once.

mod auth;
mod conn;
mod fileops;
mod hostfiles;
pub(crate) mod hosts;
/// Also drives the fixture backend's connections, so both change status alike.
pub(crate) mod lifecycle;
/// Adaptive polling of the folder on screen (§4.5); the fixture backend's polls use it too.
pub(crate) mod poll;
/// Folder size walks for the Properties sheet; the fixture backend counts with it too.
pub(crate) mod sizes;
mod transfers;
/// Sleep detection and connection checks (spike S6).
mod wake;
/// The windows as a host's task sees them; the fixture backend uses it too.
pub(crate) mod windows;

// A shell-script fake ssh and OpenSSH's sftp-server: Unix only.
#[cfg(all(test, unix))]
mod tests;
#[cfg(all(test, unix))]
mod checks_tests;
#[cfg(all(test, unix))]
mod windows_e2e_tests;

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::future::pending;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dromon_core::{ConnStatus, HostId, HostSpec};
use dromon_sftp::Visibility;
use dromon_ssh::{AskpassRequest, AskpassServer, ConfigSources, PasswordStore, ssh_program};
use tokio::sync::mpsc;

use self::auth::PromptIds;
use self::conn::{ConnCmd, ConnHandle, Shared};
use self::sizes::SizeCmd;
use crate::state::{App, Backend, Command, ServiceEvent, WindowId};

/// Where the service's events go.
pub trait Sink: Clone + Send + Sync + 'static {
    /// To every main window.
    fn emit(&self, ev: ServiceEvent);
    /// To main window `window` alone. A sink that doesn't tell windows apart hands it on like
    /// any other.
    fn emit_to(&self, window: WindowId, ev: ServiceEvent) {
        let _ = window;
        self.emit(ev);
    }
}

/// The app's sink: every main window's `AppHandle`, by window id. Each event is applied on
/// the UI thread in the windows it's for; a window's view re-runs once per batch of updates
/// drained together. Windows join as they open and leave as they close (`crate::windows`).
#[derive(Clone, Default)]
pub struct RuneSink(Arc<Mutex<Handles>>);

/// The main windows' handles, in the order they opened.
type Handles = Vec<(WindowId, rune::AppHandle<App>)>;

impl RuneSink {
    fn lock(&self) -> std::sync::MutexGuard<'_, Handles> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Window `id` hears events from now on.
    pub fn add(&self, id: WindowId, handle: rune::AppHandle<App>) {
        let mut windows = self.lock();
        windows.retain(|(w, _)| *w != id);
        windows.push((id, handle));
    }

    /// Window `id` closed.
    pub fn remove(&self, id: WindowId) {
        self.lock().retain(|(w, _)| *w != id);
    }

    /// The windows, in the order they opened.
    pub fn handles(&self) -> Handles {
        self.lock().clone()
    }

    fn deliver(handle: &rune::AppHandle<App>, ev: ServiceEvent) {
        handle.update_if(move |app, tx| {
            app.apply(ev);
            // A refresh that changed rows animates them (§4.5), though no input caused it.
            if app.take_animate() {
                tx.animated();
            }
            true
        });
    }
}

impl Sink for RuneSink {
    fn emit(&self, ev: ServiceEvent) {
        for (_, handle) in self.handles() {
            Self::deliver(&handle, ev.clone());
        }
    }

    /// A window that's gone has nothing to show it in (its prompts moved when it closed).
    fn emit_to(&self, window: WindowId, ev: ServiceEvent) {
        if let Some((_, handle)) = self.handles().into_iter().find(|(w, _)| *w == window) {
            Self::deliver(&handle, ev);
        }
    }
}

/// One main window's end of the command channel.
pub struct ChannelBackend {
    window: WindowId,
    tx: mpsc::UnboundedSender<(WindowId, Command)>,
}

impl ChannelBackend {
    /// The end for main window `window` of the same channel.
    pub fn for_window(&self, window: WindowId) -> ChannelBackend {
        ChannelBackend { window, tx: self.tx.clone() }
    }
}

impl Backend for ChannelBackend {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        // A service that's gone (it only stops when the app quits) has nothing to answer.
        if let Err(mpsc::error::SendError((_, Command::AnswerPrompt { answer, .. }))) = self.tx.send((self.window, cmd)) {
            auth::wipe(answer);
        }
        Vec::new()
    }

    fn another(&mut self, window: WindowId) -> Option<Box<dyn Backend>> {
        Some(Box::new(self.for_window(window)))
    }
}

/// The service's end of the command channel: each command with the window it came from.
pub type Commands = mpsc::UnboundedReceiver<(WindowId, Command)>;

/// A command channel: the first window's backend, the receiver for [`start`].
pub fn channel() -> (ChannelBackend, Commands) {
    let (tx, rx) = mpsc::unbounded_channel();
    (ChannelBackend { window: WindowId::FIRST, tx }, rx)
}

/// How the service runs ssh and finds hosts.
#[derive(Clone, Debug)]
pub struct Config {
    /// The ssh program.
    pub ssh: PathBuf,
    /// Our own executable, which ssh runs as `SSH_ASKPASS`.
    pub helper_exe: PathBuf,
    /// Pass `-v` to ssh (the log's "Verbose logging").
    pub verbose: bool,
    /// Read only this ssh config file (`-F`), for tests. Discovery resolves hosts (`ssh -G`)
    /// against it too.
    pub config_file: Option<PathBuf>,
    /// Discover hosts from these config files, and watch them for changes; "Add to SSH config"
    /// appends to its user config.
    pub discover: Option<ConfigSources>,
    /// The saved hosts' `hosts.conf` (in the app's config folder); `None`: no saved hosts.
    pub hosts_file: Option<PathBuf>,
    /// Hosts known from the start.
    pub hosts: Vec<HostSpec>,
    /// Inherited environment variables ssh must not see, for tests (`SSH_AUTH_SOCK`, so the
    /// developer's agent keys stay out). The app passes everything through.
    pub without_env: Vec<OsString>,
}

impl Config {
    /// The real app's: the system ssh, this executable as askpass helper, the user's config.
    pub fn for_app() -> Config {
        Config {
            ssh: ssh_program(),
            helper_exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dromon")),
            verbose: false,
            config_file: None,
            discover: Some(ConfigSources::default_for_platform()),
            hosts_file: rune::dirs::AppDirs::new(crate::persist::APP_ID).map(|d| d.config().join(dromon_ssh::HOSTS_CONF)),
            hosts: Vec::new(),
            without_env: Vec::new(),
        }
    }
}

/// Starts the `dromon-service` thread with its tokio runtime, serving `commands` until the
/// app drops its backend.
pub fn start<S: Sink>(commands: Commands, sink: S, cfg: Config) -> std::io::Result<()> {
    std::thread::Builder::new().name("dromon-service".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("dromon-io").enable_all().build();
        match rt {
            Ok(rt) => rt.block_on(run(commands, sink, cfg)),
            Err(e) => eprintln!("dromon: couldn't start the network runtime: {e}"),
        }
    })?;
    Ok(())
}

async fn recv_opt<T>(rx: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}

/// The manager: routes the UI's commands and ssh's askpass requests to the host tasks, and
/// tells them when the computer woke.
pub async fn run<S: Sink>(commands: Commands, sink: S, cfg: Config) {
    let (woke, wakes) = mpsc::unbounded_channel();
    tokio::spawn(wake::watch(woke));
    serve(commands, sink, cfg, wakes).await;
}

/// [`run`], hearing of wakes (how long the computer slept) from `wakes`: the clocks' watcher's,
/// or a test's.
async fn serve<S: Sink>(mut commands: Commands, sink: S, cfg: Config, mut wakes: mpsc::UnboundedReceiver<Duration>) {
    let (askpass, mut requests) = match AskpassServer::bind() {
        Ok(mut server) => {
            let requests = server.take_requests();
            (Some(Arc::new(server)), requests)
        }
        Err(e) => {
            eprintln!("dromon: sign-in prompts are unavailable: {e}");
            (None, None)
        }
    };
    let mut known: HashMap<HostId, HostSpec> = cfg.hosts.iter().map(|h| (h.id.clone(), h.clone())).collect();
    // Discovery, its watcher and the saved hosts (`hostfiles.rs`).
    let (mut book, mut book_updates) = hostfiles::Book::start(&cfg);
    let store = Arc::new(Mutex::new(PasswordStore::new()));
    let engine = transfers::start(sink.clone());
    let shared = Shared { cfg: Arc::new(cfg), askpass, store: store.clone(), ids: PromptIds::default(), engine };
    let mut conns: HashMap<HostId, ConnHandle> = HashMap::new();
    // How each window shows, as it last said; nothing polls for a window until it says.
    let mut windows: BTreeMap<WindowId, Visibility> = BTreeMap::new();
    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                Some((_, Command::HostBook(cmd))) => book.command(cmd, &mut known),
                Some((from, cmd)) => dispatch(from, cmd, &known, &mut conns, &shared, &sink, &mut windows),
                None => break,
            },
            Some(req) = recv_opt(&mut requests) => route(req, &conns),
            Some(update) = book_updates.recv() => book.update(update, &mut known, &sink),
            Some(slept) = wakes.recv() => woke(slept, &conns),
        }
    }
    // The UI is gone: stop every connection and forget every secret.
    drop(conns);
    auth::lock(&store).clear_all();
}

/// Carries out `cmd` from window `from`.
fn dispatch<S: Sink>(
    from: WindowId,
    cmd: Command,
    known: &HashMap<HostId, HostSpec>,
    conns: &mut HashMap<HostId, ConnHandle>,
    shared: &Shared,
    sink: &S,
    windows: &mut BTreeMap<WindowId, Visibility>,
) {
    match cmd {
        Command::Connect { host } => {
            let Some(spec) = known.get(&host).cloned() else {
                let status = ConnStatus::Failed("Dromon doesn't know this host any more.".into());
                sink.emit_to(from, ServiceEvent::Status { host, status });
                return;
            };
            let c = task(spec.clone(), conns, shared, sink, windows);
            let _ = c.cmds.send(ConnCmd::Connect(from, spec));
        }
        Command::Verbose { host, on } => {
            // A host that never connected keeps it for when it does.
            let c = match known.get(&host) {
                Some(spec) => Some(task(spec.clone(), conns, shared, sink, windows)),
                None => conns.get(&host),
            };
            if let Some(c) = c {
                let _ = c.cmds.send(ConnCmd::Verbose(on));
            }
        }
        Command::ShowPrompts { host } => {
            if let Some(c) = conns.get(&host) {
                let _ = c.cmds.send(ConnCmd::ShowPrompts(from));
            }
        }
        Command::List { host, path } => {
            if let Some(c) = conns.get(&host) {
                let _ = c.cmds.send(ConnCmd::List(from, path));
            }
        }
        Command::Disconnect { host } => match conns.get(&host) {
            Some(c) => {
                let _ = c.cmds.send(ConnCmd::Disconnect);
            }
            None => {
                auth::lock(&shared.store).clear(&host);
                sink.emit(ServiceEvent::Status { host, status: ConnStatus::Disconnected });
            }
        },
        Command::AnswerPrompt { id, answer } => {
            let conn = shared.ids.owner(id).and_then(|host| conns.get(&host));
            match conn {
                Some(c) => {
                    if let Err(mpsc::error::SendError(ConnCmd::Answer(_, _, answer))) = c.cmds.send(ConnCmd::Answer(from, id, answer)) {
                        auth::wipe(answer);
                    }
                }
                None => auth::wipe(answer),
            }
        }
        Command::Transfer(cmd) => {
            let _ = shared.engine.send(transfers::EngineMsg::Command(cmd));
        }
        Command::FileOp { id, host, op } => {
            // The host's task runs it; a host without one isn't connected.
            let cmd = ConnCmd::FileOp(id, op);
            let unsent = match conns.get(&host) {
                Some(c) => c.cmds.send(cmd).is_err(),
                None => true,
            };
            if unsent {
                sink.emit(ServiceEvent::FileOpFailed { id, host, error: dromon_core::Error::ConnectionLost });
            }
        }
        Command::Window { visible, focused } => {
            let vis = match (visible, focused) {
                (false, _) => Visibility::Hidden,
                (true, true) => Visibility::Focused,
                (true, false) => Visibility::VisibleUnfocused,
            };
            // A window gaining focus checks every live connection (`wake.rs`).
            let gained = vis == Visibility::Focused && windows.get(&from) != Some(&Visibility::Focused);
            windows.insert(from, vis);
            for c in conns.values() {
                let _ = c.cmds.send(ConnCmd::Window(from, vis));
                if gained {
                    let _ = c.cmds.send(ConnCmd::Focus);
                }
            }
        }
        Command::Poll { host } => {
            if let Some(c) = conns.get(&host) {
                let _ = c.cmds.send(ConnCmd::PollNow(from));
            }
        }
        Command::WindowClosed => {
            windows.remove(&from);
            for c in conns.values() {
                let _ = c.cmds.send(ConnCmd::Closed(from));
            }
        }
        Command::FolderSize { id, host, path } => {
            // The host's task walks it; a host without one isn't connected.
            let unsent = conns.get(&host).is_none_or(|c| c.cmds.send(ConnCmd::Size(from, SizeCmd::Start { id, path })).is_err());
            if unsent {
                let total = crate::state::SizeTotal::default();
                sink.emit_to(from, ServiceEvent::FolderSizeFailed { id, host, total, error: dromon_core::Error::ConnectionLost });
            }
        }
        Command::CancelFolderSize { id, host } => {
            if let Some(c) = conns.get(&host) {
                let _ = c.cmds.send(ConnCmd::Size(from, SizeCmd::Cancel(id)));
            }
        }
        // The manager's loop takes these first.
        Command::HostBook(_) => {}
    }
}

/// `spec`'s host's task, started (and told how the windows show) if it has none running.
fn task<'c, S: Sink>(
    spec: HostSpec,
    conns: &'c mut HashMap<HostId, ConnHandle>,
    shared: &Shared,
    sink: &S,
    windows: &BTreeMap<WindowId, Visibility>,
) -> &'c ConnHandle {
    let host = spec.id.clone();
    if conns.get(&host).is_none_or(|c| c.cmds.is_closed()) {
        let c = conn::start(spec, shared.clone(), sink.clone());
        for (w, vis) in windows {
            let _ = c.cmds.send(ConnCmd::Window(*w, *vis));
        }
        conns.insert(host.clone(), c);
    }
    &conns[&host]
}

/// The computer woke after sleeping `slept`: every connection checks itself.
fn woke(slept: Duration, conns: &HashMap<HostId, ConnHandle>) {
    if crate::state::service::tracing() {
        eprintln!("dromon-trace: Wake slept_s={} hosts={}", slept.as_secs(), conns.len());
    }
    for c in conns.values() {
        let _ = c.cmds.send(ConnCmd::Wake);
    }
}

/// Hands an askpass request to its host's task; one for a host without a task is dropped,
/// which cancels it.
fn route(req: AskpassRequest, conns: &HashMap<HostId, ConnHandle>) {
    if let Some(c) = conns.get(&req.host) {
        let _ = c.asks.send(req);
    }
}
