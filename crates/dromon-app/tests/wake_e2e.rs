//! Connection checks and verbose logging end to end against the sshd fixture (`dromon-testkit`:
//! the Docker image, or the native sshd): the real service thread (`service::start`) with its
//! connection tasks, the system `ssh`, and SFTP to the fixture. Commands go in through the
//! service's channel as the UI sends them.
//!
//! - Verbose logging reconnects with `ssh -v`, whose `debug1:` lines reach the log, and back.
//! - A server gone silent (its sessions stopped with SIGSTOP: the TCP connection stays open,
//!   nothing answers, as after a sleep or a network change) is found by the probe a window's
//!   focus runs, and the connection comes back within seconds instead of after ServerAlive's
//!   45 s. (`Fixture::kill_sessions` would close the connection, which ssh notices by itself.)
//!
//! Each test writes its own ssh config (the key-auth alias `fx-key`, with a private
//! `known_hosts` that accepts the fixture's key). Stopping sessions stops every session of the
//! key user, so the tests here run one at a time ([`ONE_AT_A_TIME`]).
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails). Runs on Linux, macOS and Windows alike (the key copy gets
//! a current-user-only ACL via `copy_private_key`, and the config names `NULL_DEVICE`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{Backend, Command, ServiceEvent};
use dromon_core::{ConnStatus, HostId, HostSpec, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);

/// Held by each test: stopping the key user's sessions would stop another test's too.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn host() -> HostId {
    HostId("fx-key".into())
}

#[derive(Clone)]
struct TestSink(mpsc::Sender<ServiceEvent>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send(ev);
    }
}

struct Harness {
    fx: &'static Fixture,
    _dir: tempfile::TempDir,
    backend: service::ChannelBackend,
    events: mpsc::Receiver<ServiceEvent>,
    /// The folder the connection opened at.
    start: RemotePath,
    /// Every connection-log line so far.
    log: Vec<String>,
    /// Processes stopped by [`Harness::freeze`], killed when the test ends.
    frozen: Vec<String>,
    _one: MutexGuard<'static, ()>,
}

impl Harness {
    /// The service, connected to `fx-key`, its home folder listed.
    fn new(name: &str) -> Option<Harness> {
        let one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let fx = rt.block_on(dromon_testkit::fixture(name))?;
        let dir = tempfile::tempdir().unwrap();
        let config = write_ssh_config(fx, dir.path());
        let (tx, events) = mpsc::channel();
        let (backend, commands) = service::channel();
        let cfg = Config {
            ssh: dromon_ssh::ssh_program(),
            helper_exe: PathBuf::from(HELPER),
            verbose: false,
            config_file: Some(config),
            discover: None,
            hosts_file: None,
            hosts: vec![HostSpec::from_config_alias("fx-key")],
            without_env: vec!["SSH_AUTH_SOCK".into()],
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let mut h = Harness { fx, _dir: dir, backend, events, start: RemotePath::root(), log: Vec::new(), frozen: Vec::new(), _one: one };
        h.backend.send(Command::Connect { host: host() });
        let mut start = None;
        h.wait_for("the connection to open", |ev| {
            if let ServiceEvent::Opened { start: s, .. } = ev {
                start = Some(s.clone());
            }
            start.is_some()
        });
        h.start = start.unwrap();
        h.list();
        Some(h)
    }

    /// Lists the start folder and waits for it whole.
    fn list(&mut self) {
        self.backend.send(Command::List { host: host(), path: self.start.clone() });
        self.listed();
    }

    /// Waits for a whole listing of the start folder.
    fn listed(&mut self) {
        let start = self.start.clone();
        self.wait_for("the listing", |ev| matches!(ev, ServiceEvent::ListingPage { path, done: true, .. } if *path == start));
    }

    /// Applies events until `done` says so.
    fn wait_for(&mut self, what: &str, mut done: impl FnMut(&ServiceEvent) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = self.events.recv_timeout(left).unwrap_or_else(|_| panic!("timed out waiting for {what}\nconnection log:\n{}", self.log.join("\n")));
            match &ev {
                ServiceEvent::Log { line, .. } => self.log.push(line.clone()),
                ServiceEvent::Prompt(p) => panic!("unexpected prompt: {p:?}\nconnection log:\n{}", self.log.join("\n")),
                ServiceEvent::Status { status: ConnStatus::Failed(why), .. } => {
                    panic!("the connection failed: {why}\nconnection log:\n{}", self.log.join("\n"))
                }
                _ => {}
            }
            if done(&ev) {
                return;
            }
        }
    }

    fn ready(&mut self) {
        self.wait_for("Ready", |ev| matches!(ev, ServiceEvent::Status { status: ConnStatus::Ready, .. }));
    }

    /// The next ssh command line the service runs.
    fn next_run(&mut self) -> String {
        let from = self.log.len();
        self.wait_for("ssh to run", |ev| matches!(ev, ServiceEvent::Log { line, .. } if line.starts_with("Running ")));
        self.log[from..].iter().rfind(|l| l.starts_with("Running ")).cloned().unwrap()
    }

    /// Stops every process of the key user on the server (its sshd session and sftp-server):
    /// the connection stays open and nothing answers.
    fn freeze(&mut self) {
        let script = "pids=$(pgrep -u \"$1\"); [ -n \"$pids\" ] && kill -STOP $pids; echo $pids";
        let out = self.fx.exec(&["sh", "-c", script, "sh", self.fx.user(Role::Key)]).unwrap();
        self.frozen = out.split_whitespace().map(str::to_owned).collect();
        assert!(!self.frozen.is_empty(), "no session to stop");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if !self.frozen.is_empty() {
            let mut args = vec!["kill", "-KILL"];
            args.extend(self.frozen.iter().map(String::as_str));
            let _ = self.fx.exec(&args);
        }
    }
}

fn write_ssh_config(fx: &Fixture, dir: &Path) -> PathBuf {
    let key = dir.join("id_ed25519");
    copy_private_key(&fx.key(), &key).unwrap();
    let mut config = String::new();
    writeln!(config, "Host fx-key\n    HostName {}\n    Port {}\n    User {}", fx.host(), fx.port(), fx.user(Role::Key)).unwrap();
    writeln!(config, "    IdentityFile {}\n    IdentitiesOnly yes\n    PreferredAuthentications publickey", key.display()).unwrap();
    writeln!(config, "    UserKnownHostsFile {}\n    GlobalKnownHostsFile {NULL_DEVICE}", dir.join("known_hosts").display()).unwrap();
    writeln!(config, "    StrictHostKeyChecking accept-new\n    UpdateHostKeys no").unwrap();
    let path = dir.join("ssh_config");
    std::fs::write(&path, config).unwrap();
    path
}

#[test]
fn verbose_logging_reconnects_with_sshs_debug_lines_and_back() {
    let Some(mut h) = Harness::new("verbose") else { return };
    assert!(!h.log.iter().any(|l| l.starts_with("debug1:")), "quiet by default:\n{}", h.log.join("\n"));

    // On: the connection comes back at once with -v, the folder on screen listed again, and
    // ssh's debug lines in the log.
    h.backend.send(Command::Verbose { host: host(), on: true });
    h.wait_for("Verbose", |ev| matches!(ev, ServiceEvent::Verbose { on: true, .. }));
    let run = h.next_run();
    assert!(run.contains(" -v ") && run.ends_with(" -s fx-key sftp"), "{run}");
    let from = h.log.len();
    h.ready();
    h.listed();
    let debug: Vec<&String> = h.log[from..].iter().filter(|l| l.starts_with("debug1:")).collect();
    assert!(debug.len() >= 3, "ssh -v's lines in the log:\n{}", h.log.join("\n"));
    println!("{} debug1 lines, e.g. {:?}", debug.len(), debug[0]);

    // Off: back without them.
    h.backend.send(Command::Verbose { host: host(), on: false });
    let run = h.next_run();
    assert!(!run.contains(" -v "), "{run}");
    let from = h.log.len();
    h.ready();
    h.listed();
    assert!(!h.log[from..].iter().any(|l| l.starts_with("debug1:")), "{}", h.log[from..].join("\n"));
}

#[test]
fn a_focus_probe_finds_a_silent_server_and_reconnects_long_before_server_alive() {
    let Some(mut h) = Harness::new("silent") else { return };
    h.freeze();

    // The window gains focus: the probe gets no answer within 5 s, and the silent reconnect
    // takes over (ServerAlive: 15 s × 3).
    let focused = Instant::now();
    h.backend.send(Command::Window { visible: true, focused: true });
    h.wait_for("the probe to give up", |ev| matches!(ev, ServiceEvent::Log { line, .. } if line.contains("didn't answer a check")));
    let gave_up = focused.elapsed();
    h.wait_for("Reconnecting", |ev| matches!(ev, ServiceEvent::Status { status: ConnStatus::Reconnecting { attempt: 1 }, .. }));
    h.ready();
    h.listed();
    let back = focused.elapsed();
    println!("the probe gave up after {gave_up:?}; back after {back:?}");
    assert!(gave_up >= Duration::from_millis(4500) && gave_up < Duration::from_secs(10), "{gave_up:?}");
    assert!(back < Duration::from_secs(30), "{back:?}");
}
