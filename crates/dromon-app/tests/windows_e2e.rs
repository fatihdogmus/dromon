//! Two main windows (⌘N) end to end against the sshd fixture (`dromon-testkit`: the Docker
//! image, or the native sshd): the real service thread (`service::start`) with its connection
//! tasks, the system `ssh`, SFTP to the fixture, and two app states on top, the first window's
//! and the one ⌘N makes of it (`App::spawn`: its own backend over the same channel, the same
//! app-wide state). Each event goes where the app's sink sends it: to both windows, or to the
//! one it's for (a prompt, a host opening in the window that asked).
//!
//! Each test writes its own ssh config (a key-auth alias `fx-key` and a password alias `fx-pw`,
//! with a private `known_hosts` that accepts the fixture's key) and works in its own scratch
//! folders on the server (in the key user's home). Changes are made on the server with
//! `fx.exec`.
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails). Runs on Linux, macOS and Windows alike (the key copy gets
//! a current-user-only ACL via `copy_private_key`, and the config names `NULL_DEVICE`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{Action, App, HostDirectory, Listing, ServiceEvent, WindowId};
use dromon_core::{ConnStatus, HostId, HostSpec, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, PASSWORD, Role, copy_private_key};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);

fn key_host() -> HostId {
    HostId("fx-key".into())
}

fn pw_host() -> HostId {
    HostId("fx-pw".into())
}

/// Each event with the window it's for (`None`: every window), as the app's sink sends it.
#[derive(Clone)]
struct TestSink(mpsc::Sender<(Option<WindowId>, ServiceEvent)>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send((None, ev));
    }
    fn emit_to(&self, window: WindowId, ev: ServiceEvent) {
        let _ = self.0.send((Some(window), ev));
    }
}

struct Harness {
    fx: &'static Fixture,
    _dir: tempfile::TempDir,
    /// The first window's state, and the second's.
    a: App,
    b: App,
    events: mpsc::Receiver<(Option<WindowId>, ServiceEvent)>,
    /// This test's folder on the server.
    scratch: RemotePath,
    /// Every prompt, with the window it went to.
    prompts: Vec<Option<WindowId>>,
    /// Connection-log lines, for failure messages.
    log: Vec<String>,
}

impl Harness {
    fn new(name: &str) -> Option<Harness> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let fx = rt.block_on(dromon_testkit::fixture(name))?;
        let dir = tempfile::tempdir().unwrap();
        let config = write_ssh_config(fx, dir.path());
        let (tx, events) = mpsc::channel();
        let (backend, commands) = service::channel();
        let hosts = vec![HostSpec::from_config_alias("fx-key"), HostSpec::from_config_alias("fx-pw")];
        let cfg = Config {
            ssh: dromon_ssh::ssh_program(),
            helper_exe: PathBuf::from(HELPER),
            verbose: false,
            config_file: Some(config),
            discover: None,
            hosts_file: None,
            hosts: hosts.clone(),
            without_env: vec!["SSH_AUTH_SOCK".into()],
            editing_root: None,
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let scratch = fx.path(Role::Key, &format!("scratch/windows-{name}-{}", std::process::id()));
        let s = scratch.as_str();
        let make = format!("rm -rf '{s}' && mkdir -p '{s}/one' '{s}/two' && chown -R {} '{s}'", fx.owner(Role::Key));
        fx.exec(&["sh", "-c", &make]).unwrap();
        let mut a = App::new(HostDirectory { hosts }, 0, Box::new(backend));
        // ⌘N's window: the same service and app-wide state, its own backend end.
        let b = a.spawn().expect("the channel serves another window");
        assert_eq!((a.window, b.window), (WindowId(0), WindowId(1)));
        Some(Harness { fx, _dir: dir, a, b, events, scratch, prompts: Vec::new(), log: Vec::new() })
    }

    /// The next event within `wait`, applied to the windows it's for.
    fn next(&mut self, wait: Duration) -> Option<(Option<WindowId>, ServiceEvent)> {
        let (to, ev) = self.events.recv_timeout(wait).ok()?;
        match &ev {
            ServiceEvent::Log { host, line } => self.log.push(format!("[{}] {line}", host.0)),
            ServiceEvent::Prompt(_) => self.prompts.push(to),
            ServiceEvent::Status { status: ConnStatus::Failed(why), .. } => {
                panic!("a connection failed: {why}\nconnection log:\n{}", self.log.join("\n"))
            }
            _ => {}
        }
        match to {
            None => {
                self.a.apply(ev.clone());
                self.b.apply(ev.clone());
            }
            Some(w) if w == self.a.window => self.a.apply(ev.clone()),
            Some(_) => self.b.apply(ev.clone()),
        }
        Some((to, ev))
    }

    /// Applies events until `done` says so (after applying the one it's shown).
    fn pump(&mut self, what: &str, mut done: impl FnMut(&App, &App, Option<WindowId>, &ServiceEvent) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (to, ev) = self
                .next(left)
                .unwrap_or_else(|| panic!("timed out waiting for {what}\nconnection log:\n{}", self.log.join("\n")));
            if done(&self.a, &self.b, to, &ev) {
                return;
            }
        }
    }

    /// Applies whatever arrives within `wait`.
    fn settle(&mut self, wait: Duration) {
        let deadline = Instant::now() + wait;
        while self.next(deadline.saturating_duration_since(Instant::now())).is_some() {}
    }

    fn folder(&self, rel: &str) -> RemotePath {
        self.scratch.join(rel)
    }

    /// Runs `cmd` on the server (as root), then hands the scratch folder to the key user.
    fn remote(&self, cmd: &str) {
        let s = self.scratch.as_str();
        self.fx.exec(&["sh", "-c", &format!("{cmd} && chown -R {} '{s}'", self.fx.owner(Role::Key))]).unwrap();
    }
}

/// Whether `app` shows `host`'s `path`, listed.
fn shows(app: &App, host: &HostId, path: &RemotePath) -> bool {
    app.active.as_ref() == Some(host)
        && app.conn().is_some_and(|c| c.opened && c.cwd == *path && matches!(c.listing, Listing::Ready(_)))
}

fn has(app: &App, name: &str) -> bool {
    app.conn().is_some_and(|c| c.find(name).is_some())
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
        "    IdentityFile {}\n    PubkeyAuthentication yes\n    PreferredAuthentications publickey",
        key.display()
    )
    .unwrap();
    writeln!(
        config,
        "Host fx-pw\n    HostName {}\n    Port {}\n    User {}",
        fx.host(),
        fx.port(),
        fx.user(Role::Password)
    )
    .unwrap();
    writeln!(
        config,
        "    PubkeyAuthentication no\n    PreferredAuthentications password\n    NumberOfPasswordPrompts 3"
    )
    .unwrap();
    writeln!(config, "Host *\n    IdentitiesOnly yes\n    UserKnownHostsFile {}", dir.join("known_hosts").display())
        .unwrap();
    writeln!(
        config,
        "    GlobalKnownHostsFile {NULL_DEVICE}\n    StrictHostKeyChecking accept-new\n    UpdateHostKeys no"
    )
    .unwrap();
    let path = dir.join("ssh_config");
    std::fs::write(&path, config).unwrap();
    path
}

#[test]
fn two_windows_on_one_host_each_see_what_is_made_in_their_own_folder() {
    let Some(mut h) = Harness::new("folders") else { return };
    let (one, two) = (h.folder("one"), h.folder("two"));
    // The first window connects and goes to `one`.
    h.a.act(Action::SelectHost(key_host()));
    h.pump("the first window to open the host", |a, _, _, _| {
        a.conn().is_some_and(|c| c.opened && matches!(c.listing, Listing::Ready(_)))
    });
    h.a.act(Action::GoTo(one.clone()));
    h.pump("one to list in the first window", |a, _, _, _| shows(a, &key_host(), &one));
    // The second asks for the same host: it opens there alone, on the same connection, in the
    // folder remembered last (the first window's), then goes to `two`.
    h.b.act(Action::SelectHost(key_host()));
    h.pump("the host to open in the second window", |_, _, to, ev| {
        matches!(ev, ServiceEvent::Opened { .. }) && to == Some(WindowId(1))
    });
    h.pump("the second window's listing", |_, b, _, _| shows(b, &key_host(), &one));
    h.b.act(Action::GoTo(two.clone()));
    h.pump("two to list in the second window", |_, b, _, _| shows(b, &key_host(), &two));
    assert!(shows(&h.a, &key_host(), &one), "the first window stays in one");

    // Both show; the first has the focus.
    h.a.window_focus(true);
    h.b.window_focus(false);
    h.settle(Duration::from_millis(500));
    h.remote(&format!("printf a > '{}/in-one.txt' && printf b > '{}/in-two.txt'", one.as_str(), two.as_str()));
    h.pump("each file in its window", |a, b, _, _| has(a, "in-one.txt") && has(b, "in-two.txt"));
    assert!(shows(&h.a, &key_host(), &one) && shows(&h.b, &key_host(), &two), "refreshes, not navigations");
    assert!(!has(&h.a, "in-two.txt") && !has(&h.b, "in-one.txt"));
    assert!(
        h.a.conn().unwrap().fresh.is_new("in-one.txt") && h.b.conn().unwrap().fresh.is_new("in-two.txt"),
        "highlighted as new"
    );
    assert!(h.prompts.is_empty(), "no prompts for a key");
}

#[test]
fn a_password_prompt_shows_in_the_focused_window_and_is_answered_once() {
    let Some(mut h) = Harness::new("prompt") else { return };
    h.a.window_focus(true);
    h.b.window_focus(false);
    // The second window connects the password host while the first has the focus.
    h.b.act(Action::SelectHost(pw_host()));
    h.pump("the password sheet", |a, _, _, _| {
        a.ui.sheets.open().and_then(|s| s.prompt()).is_some_and(|p| p.host == pw_host())
    });
    assert!(!h.b.ui.sheets.is_open(), "the sheet is the focused window's alone");
    assert_eq!(h.prompts, [Some(WindowId(0))]);
    // Answered there once: the host opens in the window that asked for it.
    h.a.ui.sheets.edit_mut().replace_text(PASSWORD);
    h.a.act(Action::SubmitPrompt);
    let home = h.fx.home(Role::Password);
    h.pump("the password host's home in the second window", |_, b, _, _| shows(b, &pw_host(), &home));
    h.settle(Duration::from_millis(300));
    assert_eq!(h.prompts.len(), 1, "asked once");
    assert!(!h.a.ui.sheets.is_open() && !h.b.ui.sheets.is_open(), "no sheet left anywhere");
    assert!(!h.a.conns.contains_key(&pw_host()), "the first window never showed the host");
    assert_eq!(h.a.active, None);
}
