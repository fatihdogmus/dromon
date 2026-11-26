//! Quick connect, saved hosts and "Add to SSH config" end to end against the sshd fixture
//! (`dromon-testkit`: the Docker image, or the native sshd): the real `App` with its view in a
//! headless Rune `Runtime`, the real service thread with its files task, the system `ssh`, the
//! `dromon` binary as askpass helper, SFTP to the fixture. Everything goes through the UI: the
//! sidebar's +, a pasted `user@127.0.0.1:port`, More, the identity file's picker (answered as the
//! platform's would be), Return, the host menu (native on macOS and Windows, drawn elsewhere),
//! the Add to SSH config sheet.
//!
//! Isolation, test-only: a temp folder stands in for `~/.ssh` (`ConfigSources::user_ssh_dir`)
//! with its own `config` (read by discovery and, with `-F`, by ssh), `known_hosts` and key copy;
//! another for the app's config folder (`hosts.conf`). The developer's `~/.ssh` and settings
//! are never read or written. `SSH_AUTH_SOCK` is kept from ssh.
//!
//! When the backend can't run the test prints a skip line and passes (`DROMON_REQUIRE_DOCKER=1`
//! fails). Windows: the inbox `ssh.exe` and the named-pipe askpass, against the Docker image in
//! WSL2 (CI); the key copy gets a current-user-only ACL (`copy_private_key`).

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{App, HostDirectory, Listing, ServiceEvent, Sheet};
use dromon_core::{ConnStatus, HostId, HostSource, PromptKind, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const SIZE: (f32, f32) = (1080.0, 680.0);
/// The longest one step (a connection, a listing, a discovery) may take.
const WAIT: Duration = Duration::from_secs(30);
/// The saved host's name.
const SAVED: &str = "fx-saved";

fn note(line: impl AsRef<str>) {
    let _ = writeln!(std::io::stderr(), "{}", line.as_ref());
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
    /// Stands in for `~/.ssh`.
    ssh_dir: PathBuf,
    config: PathBuf,
    key: PathBuf,
    hosts_conf: PathBuf,
    rt: Runtime<App>,
    events: mpsc::Receiver<ServiceEvent>,
    seen: Vec<String>,
    started: Instant,
}

impl Harness {
    fn new(name: &str) -> Option<Harness> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let fx = rt.block_on(dromon_testkit::fixture(name))?;
        let dir = tempfile::tempdir().unwrap();
        let ssh_dir = dir.path().join("ssh");
        std::fs::create_dir(&ssh_dir).unwrap();
        let key = ssh_dir.join("id_ed25519");
        copy_private_key(&fx.key(), &key).unwrap();
        let config = ssh_dir.join("config");
        std::fs::write(&config, isolation(&ssh_dir)).unwrap();
        let hosts_conf = dir.path().join("app").join("hosts.conf");

        let (tx, events) = mpsc::channel();
        let (backend, commands) = service::channel();
        let cfg = Config {
            ssh: dromon_ssh::ssh_program(),
            helper_exe: PathBuf::from(HELPER),
            verbose: false,
            config_file: Some(config.clone()),
            discover: Some(dromon_ssh::ConfigSources {
                user: Some(config.clone()),
                system: None,
                user_ssh_dir: ssh_dir.clone(),
            }),
            hosts_file: Some(hosts_conf.clone()),
            hosts: Vec::new(),
            without_env: vec!["SSH_AUTH_SOCK".into()],
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let mut rt = dromon_app::app().headless_runtime(SIZE, Env::new(true, 1.0));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        rt.update(move |s, _| *s = App::new(HostDirectory::default(), now, Box::new(backend)));
        rt.frame(0.0);
        let mut h = Harness {
            fx,
            _dir: dir,
            ssh_dir,
            config,
            key,
            hosts_conf,
            rt,
            events,
            seen: Vec::new(),
            started: Instant::now(),
        };
        // The files task reads hosts.conf (missing: nothing saved) first thing.
        h.pump_until("the saved hosts read", |h| h.seen.iter().any(|e| e.starts_with("HostBook(Saved(")));
        Some(h)
    }

    fn step(&self, what: &str) {
        note(format!("quick_e2e: {what} ({:.2} s)", self.started.elapsed().as_secs_f64()));
    }

    fn settle(&mut self) {
        for _ in 0..4 {
            self.rt.frame(0.25);
        }
    }

    fn apply(&mut self, ev: ServiceEvent) {
        self.seen.push(format!("{ev:?}"));
        self.rt.update(move |app, _| app.apply(ev));
    }

    /// Applies the service's events as they come, between frames, until `done` holds.
    fn pump_until(&mut self, what: &str, mut done: impl FnMut(&Harness) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            let mut any = false;
            while let Ok(ev) = self.events.try_recv() {
                self.apply(ev);
                any = true;
            }
            if any {
                self.settle();
            }
            if done(self) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let last: Vec<&String> = self.seen.iter().rev().take(25).collect();
                let log: Vec<String> = self.rt.state.conns.values().map(|c| c.log.to_text()).collect();
                panic!(
                    "timed out after {WAIT:?} waiting for {what}\nlast events (newest first):\n{last:#?}\nlogs:\n{}",
                    log.join("\n")
                );
            }
            match self.events.recv_timeout(left.min(Duration::from_millis(100))) {
                Ok(ev) => {
                    self.apply(ev);
                    self.settle();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the service thread is gone"),
            }
        }
    }

    fn click(&mut self, k: &str) {
        let (x, y) = self.rt.find(k).unwrap_or_else(|| panic!("no element {k:?}"));
        self.rt.pointer_move(x, y);
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.settle();
    }

    fn key(&mut self, key: Key) {
        self.rt.key(KeyEvent { key, mods: Mods::default() });
        self.settle();
    }

    fn shown(&self, k: &str) -> bool {
        self.rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
    }

    fn text_of(&self, k: &str) -> String {
        self.rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(k) && !e.exiting)
            .and_then(|e| e.text)
            .unwrap_or_default()
    }

    /// Picks `item` from `host`'s menu: the platform's own (macOS, Windows) or the drawn one.
    fn host_menu(&mut self, host: &str, item: &str) {
        let (x, y) = self.rt.find(&format!("host-{host}")).unwrap_or_else(|| panic!("no host {host} in the sidebar"));
        self.rt.pointer_move(x, y);
        self.rt.secondary_down(x, y);
        self.rt.secondary_up(x, y);
        self.settle();
        if self.rt.popup_menu().is_some() {
            self.rt.resolve_popup(Some(item)).unwrap();
            self.settle();
        } else {
            self.click(&format!("ctx-{item}"));
        }
    }

    /// The address of the key user on the fixture, as a user pastes it.
    fn address(&self) -> String {
        format!("{}@{}:{}", self.fx.user(Role::Key), self.fx.host(), self.fx.port())
    }

    /// The sidebar's +, the address pasted into Host, More, and the identity file through its
    /// picker.
    fn fill_quick_connect(&mut self) {
        self.click("sb-quick-connect");
        assert!(self.shown("quick"), "the popover opens");
        assert_eq!(self.rt.presented_focus_key(), Some("quick-host-input"));
        let address = self.address();
        self.rt.paste(address);
        self.settle();
        let q = &self.rt.state.ui.quick;
        let field = |f| q.edit(f).text.clone();
        use dromon_ssh::QuickField::{Host, Port, User};
        assert_eq!(field(Host), self.fx.host());
        assert_eq!(field(User), self.fx.user(Role::Key));
        assert_eq!(field(Port), self.fx.port().to_string());
        self.click("quick-more");
        self.click("quick-pick-identity");
        assert!(self.rt.file_dialog().is_some(), "the picker is open");
        let key = self.key.clone();
        self.rt.resolve_file_dialog(Some(key)).unwrap();
        self.settle();
        let picked = self.rt.state.ui.quick.edit(dromon_ssh::QuickField::IdentityFile).text.clone();
        assert_eq!(picked, self.key.to_string_lossy());
    }

    /// Waits until `path` lists in full on screen.
    fn wait_for_listing(&mut self, path: &str) -> Vec<String> {
        let path = RemotePath::new(path);
        self.pump_until(&format!("the listing of {path}"), |h| {
            h.rt.state.conn().is_some_and(|c| {
                c.status == ConnStatus::Ready
                    && c.cwd == path
                    && matches!(c.listing, Listing::Ready(_))
                    && !c.loading_more()
            })
        });
        self.rt.state.conn().unwrap().row_names().into_iter().map(str::to_owned).collect()
    }

    fn source(&self, id: &str) -> Option<HostSource> {
        self.rt.state.hosts.get(&HostId(id.into())).map(|h| h.source)
    }
}

/// The test's ssh config before anything is added: only settings for every host, so discovery
/// finds no host in it. Private known_hosts, no agent keys, no password or keyboard prompts.
fn isolation(ssh_dir: &Path) -> String {
    let mut c = String::from("Host *\n");
    for line in [
        format!("UserKnownHostsFile {}", ssh_dir.join("known_hosts").display()),
        format!("GlobalKnownHostsFile {NULL_DEVICE}"),
        "StrictHostKeyChecking ask".into(),
        "UpdateHostKeys no".into(),
        "HashKnownHosts no".into(),
        "IdentitiesOnly yes".into(),
        "AddKeysToAgent no".into(),
        "PreferredAuthentications publickey".into(),
    ] {
        writeln!(c, "    {line}").unwrap();
    }
    c
}

/// Quick connect by address and identity file → the listing, and the host in Recent; Save → in
/// Saved and in `hosts.conf`; Add to SSH config → a backup, the block appended, the host under
/// SSH Config (discovered from the file), out of `hosts.conf`, and it connects as a config host.
#[test]
fn quick_connect_save_and_add_to_ssh_config() {
    let Some(mut h) = Harness::new("quick_connect_save_and_add_to_ssh_config") else { return };
    let home = h.fx.home(Role::Key).as_str().to_owned();

    // 1. Quick connect.
    h.fill_quick_connect();
    h.click("quick-connect");
    assert!(!h.shown("quick"), "Connect closes the popover");
    let recent = format!("{}@{}:{}", h.fx.user(Role::Key), h.fx.host(), h.fx.port());
    assert_eq!(h.rt.state.active, Some(HostId(recent.clone())));
    h.pump_until("the host key sheet", |h| matches!(h.rt.state.ui.sheets.open(), Some(Sheet::Ask { prompt, .. }) if matches!(prompt.kind, PromptKind::HostKey { .. })));
    assert_eq!(h.text_of("sheet-title"), format!("First connection to {}", h.fx.host()));
    h.key(Key::Enter);
    let rows = h.wait_for_listing(&home);
    assert!(rows.iter().any(|r| r == "fixture"), "{rows:?}");
    assert_eq!(h.source(&recent), Some(HostSource::Recent));
    assert_eq!(h.rt.state.recent_hosts()[0].id.as_str(), recent);
    assert!(h.shown(&format!("host-{recent}")));
    let log = h.rt.state.conns[&HostId(recent.clone())].log.to_text();
    assert!(log.contains("-o HostName=127.0.0.1") && log.contains("-o IdentityFile="), "explicit options: {log}");
    h.step(&format!("quick connect {recent} with the picked key: listed {home}; in Recent"));

    // 2. Save it under a name.
    h.fill_quick_connect();
    h.click("quick-alias-input");
    h.key(Key::Text(SAVED.into()));
    h.click("quick-save");
    assert!(!h.shown("quick"));
    h.pump_until("the service's saved list with it", |h| {
        h.seen.iter().any(|e| e.starts_with("HostBook(Saved(") && e.contains(SAVED))
            && h.shown(&format!("host-{SAVED}"))
    });
    assert_eq!(h.source(SAVED), Some(HostSource::Saved));
    let conf = std::fs::read_to_string(&h.hosts_conf).unwrap();
    assert!(conf.contains(&format!("Host {SAVED}\n    HostName 127.0.0.1\n")), "{conf}");
    assert!(conf.contains("    IdentityFile "), "{conf}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&h.hosts_conf).unwrap().permissions().mode() & 0o777,
            0o600,
            "hosts.conf is the user's alone"
        );
    }
    h.step(&format!("saved {SAVED}: in Saved and in hosts.conf"));

    // 3. Add it to the ssh config.
    let before = std::fs::read_to_string(&h.config).unwrap();
    h.host_menu(SAVED, "host-add-to-config");
    h.pump_until("the sheet's block", |h| h.shown("add-config-block-text"));
    let block = h.text_of("add-config-block-text");
    assert!(
        block.starts_with(&format!(
            "Host {SAVED}\n    HostName 127.0.0.1\n    User {}\n    Port {}\n",
            h.fx.user(Role::Key),
            h.fx.port()
        )),
        "{block}"
    );
    assert!(h.text_of("add-config-backup").contains("config.dromon-backup-"));
    h.click("add-config-confirm");
    h.pump_until("the host under SSH Config, resolved, and the sheet gone", |h| {
        h.source(SAVED) == Some(HostSource::SshConfig)
            && h.rt.state.hosts.get(&HostId(SAVED.into())).is_some_and(|s| s.hostname.as_deref() == Some("127.0.0.1"))
            && !h.shown("add-config")
    });
    let backups: Vec<PathBuf> = std::fs::read_dir(&h.ssh_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains("config.dromon-backup-"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), before, "the backup is the file as it was");
    let after = std::fs::read_to_string(&h.config).unwrap();
    assert!(after.starts_with(&before), "the file is kept and appended to");
    assert!(after[before.len()..].starts_with(&format!("\nHost {SAVED}\n")), "{after}");
    assert!(!std::fs::read_to_string(&h.hosts_conf).unwrap().contains(SAVED), "out of hosts.conf");
    h.step(&format!(
        "added {SAVED} to the ssh config: backup {}, block appended, under SSH Config",
        backups[0].display()
    ));

    // 4. It connects as an SSH Config host (the host key is known from step 1).
    h.click(&format!("host-{SAVED}"));
    assert_eq!(h.rt.state.active, Some(HostId(SAVED.into())));
    h.wait_for_listing(&home);
    assert!(!h.rt.state.ui.sheets.is_open(), "nothing to ask: the key is known");
    let log = h.rt.state.conns[&HostId(SAVED.into())].log.to_text();
    assert!(!log.contains("-o HostName"), "a config host passes only its alias: {log}");
    h.step(&format!("{SAVED} connects from the ssh config and lists {home}"));
}
