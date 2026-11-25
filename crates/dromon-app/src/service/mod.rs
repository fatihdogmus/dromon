//! The service thread (§4.1): a dedicated `dromon-service` thread running a multi-threaded tokio
//! runtime, which discovers hosts, runs one ssh per connected host, answers ssh's prompts
//! through the askpass helper and the UI's sheets, and serves SFTP listings.
//!
//! - UI → service: [`ChannelBackend`] puts each [`Command`] on an unbounded channel; sending
//!   never blocks the UI.
//! - Service → UI: every [`ServiceEvent`] goes to a [`Sink`]; in the app that is [`RuneSink`],
//!   which applies it on the UI thread with `AppHandle::update_if`.
//!
//! The manager ([`run`]) routes commands and askpass requests to one task per host
//! (`conn.rs`), whose lifecycle is the pure machine in `lifecycle.rs` and whose prompts go
//! through `auth.rs`; its file operations (new folder, rename, delete) run in `fileops.rs`
//! on its SFTP session. The password store lives here only: cleared per host on Disconnect, and
//! all of it when the UI goes away. `hostfiles.rs` owns the files: discovery and its watcher,
//! the saved hosts' `hosts.conf`, and "Add to SSH config".

mod auth;
mod conn;
mod fileops;
mod hostfiles;
pub(crate) mod hosts;
/// Also drives the fixture backend's connections, so both change status alike.
pub(crate) mod lifecycle;
/// Adaptive polling of the folder on screen (§4.5); the fixture backend's polls use it too.
pub(crate) mod poll;
mod transfers;

// A shell-script fake ssh and OpenSSH's sftp-server: Unix only.
#[cfg(all(test, unix))]
mod tests;

use std::collections::HashMap;
use std::ffi::OsString;
use std::future::pending;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dromon_core::{ConnStatus, HostId, HostSpec};
use dromon_sftp::Visibility;
use dromon_ssh::{AskpassRequest, AskpassServer, ConfigSources, PasswordStore, ssh_program};
use tokio::sync::mpsc;

use self::auth::PromptIds;
use self::conn::{ConnCmd, ConnHandle, Shared};
use crate::state::{App, Backend, Command, ServiceEvent};

/// Where the service's events go.
pub trait Sink: Clone + Send + Sync + 'static {
    fn emit(&self, ev: ServiceEvent);
}

/// The app's sink: applies each event on the UI thread. The view re-runs once per batch of
/// updates drained together.
pub struct RuneSink(pub rune::AppHandle<App>);

impl Clone for RuneSink {
    fn clone(&self) -> Self {
        RuneSink(self.0.clone())
    }
}

impl Sink for RuneSink {
    fn emit(&self, ev: ServiceEvent) {
        self.0.update_if(move |app, tx| {
            app.apply(ev);
            // A refresh that changed rows animates them (§4.5), though no input caused it.
            if app.take_animate() {
                tx.animated();
            }
            true
        });
    }
}

/// The UI's end of the command channel.
pub struct ChannelBackend {
    tx: mpsc::UnboundedSender<Command>,
}

impl Backend for ChannelBackend {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        // A service that's gone (it only stops when the app quits) has nothing to answer.
        if let Err(mpsc::error::SendError(Command::AnswerPrompt { answer, .. })) = self.tx.send(cmd) {
            auth::wipe(answer);
        }
        Vec::new()
    }
}

/// The service's end of the command channel.
pub type Commands = mpsc::UnboundedReceiver<Command>;

/// A command channel: the backend for the app, the receiver for [`start`].
pub fn channel() -> (ChannelBackend, Commands) {
    let (tx, rx) = mpsc::unbounded_channel();
    (ChannelBackend { tx }, rx)
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

/// The manager: routes the UI's commands and ssh's askpass requests to the host tasks.
pub async fn run<S: Sink>(mut commands: Commands, sink: S, cfg: Config) {
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
    // Nothing polls until the UI says the window shows.
    let mut window = Visibility::Hidden;
    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                Some(Command::HostBook(cmd)) => book.command(cmd, &mut known),
                Some(cmd) => dispatch(cmd, &known, &mut conns, &shared, &sink, &mut window),
                None => break,
            },
            Some(req) = recv_opt(&mut requests) => route(req, &conns),
            Some(update) = book_updates.recv() => book.update(update, &mut known, &sink),
        }
    }
    // The UI is gone: stop every connection and forget every secret.
    drop(conns);
    auth::lock(&store).clear_all();
}

fn dispatch<S: Sink>(
    cmd: Command,
    known: &HashMap<HostId, HostSpec>,
    conns: &mut HashMap<HostId, ConnHandle>,
    shared: &Shared,
    sink: &S,
    window: &mut Visibility,
) {
    match cmd {
        Command::Connect { host } => {
            let Some(spec) = known.get(&host).cloned() else {
                let status = ConnStatus::Failed("Dromon doesn't know this host any more.".into());
                sink.emit(ServiceEvent::Status { host, status });
                return;
            };
            let alive = conns.get(&host).is_some_and(|c| !c.cmds.is_closed());
            if !alive {
                let c = conn::start(spec.clone(), shared.clone(), sink.clone());
                let _ = c.cmds.send(ConnCmd::Window(*window));
                conns.insert(host.clone(), c);
            }
            if let Some(c) = conns.get(&host) {
                let _ = c.cmds.send(ConnCmd::Connect(spec));
            }
        }
        Command::List { host, path } => {
            if let Some(c) = conns.get(&host) {
                let _ = c.cmds.send(ConnCmd::List(path));
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
                    if let Err(mpsc::error::SendError(ConnCmd::Answer(_, answer))) = c.cmds.send(ConnCmd::Answer(id, answer)) {
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
            *window = match (visible, focused) {
                (false, _) => Visibility::Hidden,
                (true, true) => Visibility::Focused,
                (true, false) => Visibility::VisibleUnfocused,
            };
            for c in conns.values() {
                let _ = c.cmds.send(ConnCmd::Window(*window));
            }
        }
        Command::Poll { host } => {
            if let Some(c) = conns.get(&host) {
                let _ = c.cmds.send(ConnCmd::PollNow);
            }
        }
        // The manager's loop takes these first.
        Command::HostBook(_) => {}
    }
}

/// Hands an askpass request to its host's task; one for a host without a task is dropped,
/// which cancels it.
fn route(req: AskpassRequest, conns: &HashMap<HostId, ConnHandle>) {
    if let Some(c) = conns.get(&req.host) {
        let _ = c.asks.send(req);
    }
}
