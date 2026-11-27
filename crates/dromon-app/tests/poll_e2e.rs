//! Adaptive polling (§4.5, §6.8) end to end against the sshd fixture (`dromon-testkit`: the
//! Docker image, or the native sshd): the real service thread (`service::start`) with its
//! connection tasks, the system `ssh`, SFTP to the fixture, and the app's own state (`App`) on
//! top, applying every event as the window would. Changes are made on the server with
//! `fx.exec`; the tests send no command after them, only tell the app how the window shows.
//!
//! Each test writes its own ssh config (one key-auth alias, `fx-key`, with a private
//! `known_hosts` that accepts the fixture's key) and works in its own scratch folder on the
//! server (in the key user's home).
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails). Runs on Linux, macOS and Windows alike (the key copy gets
//! a current-user-only ACL via `copy_private_key`, and the config names `NULL_DEVICE`).
//!
//! The 20,000-entry limit is tested here with a real folder of 20,001 files (made in one `exec`,
//! listed once); the schedule's bands themselves are unit-tested in `dromon-sftp` and the
//! service's `poll_tests.rs`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{Action, App, FileAction, HostDirectory, Listing, ServiceEvent};
use dromon_core::{ConnStatus, Error, HostId, HostSpec, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);
/// The focused interval for a small folder (§4.5).
const INTERVAL: Duration = Duration::from_secs(2);
/// What a loaded CI runner may add to an interval: ssh round trips, the poll itself.
const SLACK: Duration = Duration::from_secs(3);

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
    /// The app's state on the real service, as the window would hold it.
    app: App,
    events: mpsc::Receiver<ServiceEvent>,
    /// This test's folder on the server.
    scratch: RemotePath,
    /// Connection-log lines, for failure messages.
    log: Vec<String>,
}

impl Harness {
    /// The service and the app, connected, with a fresh scratch folder on screen.
    fn new(name: &str) -> Option<Harness> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let fx = rt.block_on(dromon_testkit::fixture(name))?;
        let dir = tempfile::tempdir().unwrap();
        let config = write_ssh_config(fx, dir.path());
        let (tx, events) = mpsc::channel();
        let (backend, commands) = service::channel();
        let spec = HostSpec::from_config_alias("fx-key");
        let cfg = Config {
            ssh: dromon_ssh::ssh_program(),
            helper_exe: PathBuf::from(HELPER),
            verbose: false,
            config_file: Some(config),
            discover: None,
            hosts_file: None,
            hosts: vec![spec.clone()],
            without_env: vec!["SSH_AUTH_SOCK".into()],
            editing_root: None,
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let scratch = fx.path(Role::Key, &format!("scratch/poll-{name}-{}", std::process::id()));
        let make = format!("rm -rf '{0}' && mkdir -p '{0}' && chown {1} '{0}'", scratch.as_str(), fx.owner(Role::Key));
        fx.exec(&["sh", "-c", &make]).unwrap();
        let app = App::new(HostDirectory { hosts: vec![spec] }, 0, Box::new(backend));
        let mut h = Harness { fx, _dir: dir, app, events, scratch, log: Vec::new() };
        h.app.act(Action::SelectHost(host()));
        h.pump("the connection to open", |app, _| {
            app.conn().is_some_and(|c| c.opened && matches!(c.listing, Listing::Ready(_)))
        });
        let scratch = h.scratch.clone();
        h.go(&scratch);
        Some(h)
    }

    /// Applies events to the app until `done` says so (after applying the one it's shown).
    fn pump(&mut self, what: &str, mut done: impl FnMut(&App, &ServiceEvent) -> bool) -> ServiceEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = self
                .next(left)
                .unwrap_or_else(|| panic!("timed out waiting for {what}\nconnection log:\n{}", self.log.join("\n")));
            if done(&self.app, &ev) {
                return ev;
            }
        }
    }

    /// The next event within `wait`, applied to the app.
    fn next(&mut self, wait: Duration) -> Option<ServiceEvent> {
        let ev = self.events.recv_timeout(wait).ok()?;
        match &ev {
            ServiceEvent::Log { host, line } => self.log.push(format!("[{}] {line}", host.0)),
            ServiceEvent::Prompt(p) => self.log.push(format!("unexpected prompt: {p:?}")),
            ServiceEvent::Status { status: ConnStatus::Failed(why), .. } => {
                panic!("the connection failed: {why}\nconnection log:\n{}", self.log.join("\n"))
            }
            _ => {}
        }
        self.app.apply(ev.clone());
        Some(ev)
    }

    /// Every listing event (pages, refreshes, failures) for `path` within `wait`.
    fn listings_for(&mut self, path: &RemotePath, wait: Duration) -> Vec<ServiceEvent> {
        let deadline = Instant::now() + wait;
        let mut seen = Vec::new();
        while let Some(ev) = self.next(deadline.saturating_duration_since(Instant::now())) {
            let about = match &ev {
                ServiceEvent::ListingPage { path: p, .. }
                | ServiceEvent::Refreshed { path: p, .. }
                | ServiceEvent::ListFailed { path: p, .. } => p == path,
                _ => false,
            };
            if about {
                seen.push(ev);
            }
        }
        seen
    }

    /// Navigates to `path` and waits until it's listed whole.
    fn go(&mut self, path: &RemotePath) {
        self.app.act(Action::GoTo(path.clone()));
        let p = path.clone();
        self.pump("the folder to list", |app, ev| {
            matches!(ev, ServiceEvent::ListingPage { path, done: true, .. } if *path == p)
                && app.conn().is_some_and(|c| c.cwd == p)
        });
    }

    /// A refresh of `path` holding `name`, and how long it took.
    fn refresh_with(&mut self, path: &RemotePath, name: &str) -> Duration {
        let start = Instant::now();
        self.pump("a refresh", |_, ev| {
            matches!(ev, ServiceEvent::Refreshed { path: p, entries, .. } if p == path && entries.iter().any(|e| e.name == name))
        });
        start.elapsed()
    }

    /// Runs `cmd` on the server (as root), then hands what it made to the key user.
    fn remote(&self, cmd: &str) {
        let s = self.scratch.as_str();
        self.fx.exec(&["sh", "-c", &format!("{cmd} && chown -R {} '{s}'", self.fx.owner(Role::Key))]).unwrap();
    }

    fn rows(&self) -> Vec<String> {
        self.app.conn().map(|c| c.row_names().into_iter().map(str::to_owned).collect()).unwrap_or_default()
    }
}

fn write_ssh_config(fx: &Fixture, dir: &Path) -> PathBuf {
    let key = dir.join("id_ed25519");
    copy_private_key(&fx.key(), &key).unwrap();
    let mut config = String::new();
    writeln!(
        config,
        "Host fx-key\n    HostName {}\n    Port {}\n    User {}",
        fx.host(),
        fx.port(),
        fx.user(Role::Key)
    )
    .unwrap();
    writeln!(
        config,
        "    IdentityFile {}\n    IdentitiesOnly yes\n    PreferredAuthentications publickey",
        key.display()
    )
    .unwrap();
    writeln!(
        config,
        "    UserKnownHostsFile {}\n    GlobalKnownHostsFile {NULL_DEVICE}",
        dir.join("known_hosts").display()
    )
    .unwrap();
    writeln!(config, "    StrictHostKeyChecking accept-new\n    UpdateHostKeys no").unwrap();
    let path = dir.join("ssh_config");
    std::fs::write(&path, config).unwrap();
    path
}

#[test]
fn a_file_made_on_the_server_arrives_as_a_refresh_within_the_interval() {
    let Some(mut h) = Harness::new("arrives") else { return };
    let scratch = h.scratch.clone();
    h.app.window_focus(true);
    // Let the focus poll (nothing new) go by.
    let _ = h.listings_for(&scratch, Duration::from_millis(500));

    h.remote(&format!("printf hello > '{}/new.txt'", scratch.as_str()));
    let took = h.refresh_with(&scratch, "new.txt");
    println!("an external file arrived {took:?} after it was made");
    assert!(took <= INTERVAL + SLACK, "took {took:?}");
    let c = h.app.conn().unwrap();
    assert!(c.find("new.txt").is_some() && c.fresh.is_new("new.txt"), "a refresh, highlighted: {:?}", h.rows());
    assert_eq!(c.cwd, scratch, "not a navigation");

    // Polls that find nothing new send nothing.
    let quiet = h.listings_for(&scratch, INTERVAL * 2 + Duration::from_millis(500));
    assert!(quiet.is_empty(), "{quiet:?}");
}

#[test]
fn nothing_arrives_while_the_window_is_hidden_and_focus_polls_at_once() {
    let Some(mut h) = Harness::new("hidden") else { return };
    let scratch = h.scratch.clone();
    h.app.window_focus(true);
    h.app.window_visibility(false);
    let _ = h.listings_for(&scratch, Duration::from_millis(500));

    h.remote(&format!("printf a > '{}/while-hidden.txt'", scratch.as_str()));
    let hidden = h.listings_for(&scratch, INTERVAL * 2 + Duration::from_secs(1));
    assert!(hidden.is_empty(), "nothing while hidden: {hidden:?}");
    assert!(h.app.conn().unwrap().find("while-hidden.txt").is_none());

    // Back on screen: it's polled again within an interval.
    h.app.window_visibility(true);
    let took = h.refresh_with(&scratch, "while-hidden.txt");
    assert!(took <= INTERVAL + SLACK, "took {took:?}");

    // Unfocused, the interval doubles (4 s); gaining focus polls at once.
    h.app.window_focus(false);
    let _ = h.listings_for(&scratch, Duration::from_millis(300));
    h.remote(&format!("printf b > '{}/on-focus.txt'", scratch.as_str()));
    h.app.window_focus(true);
    let took = h.refresh_with(&scratch, "on-focus.txt");
    println!("a focus poll arrived after {took:?}");
    assert!(took < Duration::from_millis(1800), "sooner than any interval: {took:?}");
}

#[test]
fn deleting_the_folder_on_screen_goes_up_to_the_nearest_parent_with_a_toast() {
    let Some(mut h) = Harness::new("vanished") else { return };
    let scratch = h.scratch.clone();
    let deeper = scratch.join("sub").join("deeper");
    h.remote(&format!("mkdir -p '{0}' && printf x > '{0}/f.txt'", deeper.as_str()));
    h.go(&deeper);
    h.app.window_focus(true);
    let _ = h.listings_for(&deeper, Duration::from_millis(500));

    h.remote(&format!("rm -rf '{}/sub'", scratch.as_str()));
    // The poll finds it gone; the app climbs one listing per level: sub is gone too, scratch lists.
    let ev = h.pump(
        "the vanished folder's failure",
        |_, ev| matches!(ev, ServiceEvent::ListFailed { path, .. } if *path == deeper),
    );
    assert!(matches!(ev, ServiceEvent::ListFailed { error: Error::NotFound(_), .. }), "{ev:?}");
    let sub = scratch.join("sub");
    h.pump("sub's failure", |_, ev| matches!(ev, ServiceEvent::ListFailed { path, .. } if *path == sub));
    h.pump(
        "the parent's listing",
        |_, ev| matches!(ev, ServiceEvent::ListingPage { path, done: true, .. } if *path == scratch),
    );
    let c = h.app.conn().unwrap();
    assert_eq!(c.cwd, scratch);
    assert!(matches!(c.listing, Listing::Ready(_)) && c.find("sub").is_none());
    let toasts: Vec<String> = h.app.toasts().into_iter().map(|t| t.text).collect();
    assert_eq!(toasts, [format!("{deeper} was removed")]);
}

#[test]
fn a_folder_over_twenty_thousand_entries_is_not_polled() {
    let Some(mut h) = Harness::new("big") else { return };
    let scratch = h.scratch.clone();
    let big = scratch.join("big");
    let made = Instant::now();
    h.remote(&format!("mkdir '{0}' && cd '{0}' && seq 1 20001 | sed 's/^/f/' | xargs touch", big.as_str()));
    println!("made 20,001 files in {:?}", made.elapsed());
    let listed = Instant::now();
    h.go(&big);
    println!("listed them in {:?}", listed.elapsed());
    assert_eq!(h.app.conn().unwrap().entries().len(), 20_001);
    h.app.window_focus(true);

    h.remote(&format!("touch '{}/late'", big.as_str()));
    let polled = h.listings_for(&big, INTERVAL * 2 + Duration::from_secs(1));
    assert!(polled.is_empty(), "never polled, not even on focus: {} events", polled.len());

    // ⌘R still lists it.
    h.app.act(Action::File(FileAction::Refresh));
    h.pump("the refresh", |_, ev| matches!(ev, ServiceEvent::ListingPage { path, done: true, .. } if *path == big));
    assert_eq!(h.app.conn().unwrap().entries().len(), 20_002);
}
