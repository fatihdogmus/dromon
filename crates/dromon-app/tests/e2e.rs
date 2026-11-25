//! The whole app end to end against a real sshd: the real `App` with its view, handlers and
//! `on_key` in a headless Rune `Runtime`, the real service thread (`service::start`), the
//! system `ssh`, the real `dromon` binary as ssh's askpass helper, and SFTP to the sshd
//! fixture (`dromon-testkit`, one server for this test binary: the Docker image, or the native
//! sshd; user names and homes through `Fixture::user`/`home`). Everything a user does goes
//! through the UI: clicking a host in the sidebar, typing into a sheet's field, Return,
//! clicking buttons by key, type-to-select.
//!
//! Isolation, test-only: each test writes its own ssh config (`Config.config_file`, `-F`) with
//! host aliases `fx-key`, `fx-pw`, `fx-kbd` and `fx-enc` for the fixture, its own
//! `known_hosts` and `IdentitiesOnly yes`; discovery reads the same file (`Config.discover`),
//! so the aliases show up as SSH-config hosts; `SSH_AUTH_SOCK` is kept from ssh
//! (`Config.without_env`). The developer's `~/.ssh` is never read.
//!
//! The service's events reach the app the way `RuneSink` delivers them in a window: the test's
//! sink forwards them over a channel, and the test applies them on the runtime's thread
//! between frames (`Harness::pump_until`). Every event and every connection-log line is checked
//! for the secrets typed.
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails). The TOTP test needs the Docker backend.
//!
//! Windows: the same tests with the inbox `ssh.exe` and the named-pipe askpass, against the
//! Docker image in WSL2 (CI). The key copies get a current-user-only ACL
//! (`dromon_testkit::copy_private_key`), the config names `NUL` for "no file", and the host
//! key's fingerprint is read on the server, not with `ssh-keyscan`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{App, HostDirectory, ListError, Listing, ServiceEvent, Sheet};
use dromon_core::{ConnStatus, EntryKind, HostId, HostSource, LinkTarget, PromptKind, RemotePath};
use dromon_testkit::{Feature, Fixture, NULL_DEVICE, PASSPHRASE, PASSWORD, Role, copy_private_key, totp_now};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const SIZE: (f32, f32) = (1080.0, 680.0);
/// The longest one step (a connection, a listing) may take.
const WAIT: Duration = Duration::from_secs(20);
const WRONG_PASSWORD: &str = "not-the-password";

/// `fx-pw` tests kill `pw`'s sessions server-side, so they take turns.
static PW_USER: Mutex<()> = Mutex::new(());

fn pw_turn() -> MutexGuard<'static, ()> {
    PW_USER.lock().unwrap_or_else(|p| p.into_inner())
}

/// A line on stderr that libtest doesn't capture, so it shows in a plain `cargo test`.
fn note(line: impl AsRef<str>) {
    let _ = writeln!(std::io::stderr(), "{}", line.as_ref());
}

fn host(alias: &str) -> HostId {
    HostId(alias.into())
}

fn fixture(test: &str) -> Option<&'static Fixture> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(dromon_testkit::fixture(test))
}

/// The service's sink: forwards every event to the test's thread.
#[derive(Clone)]
struct TestSink(mpsc::Sender<ServiceEvent>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send(ev);
    }
}

/// One test's app, service thread and private ssh files on the shared sshd.
struct Harness {
    name: &'static str,
    fx: &'static Fixture,
    dir: tempfile::TempDir,
    rt: Runtime<App>,
    events: mpsc::Receiver<ServiceEvent>,
    /// `Debug` of every event applied, for the secret checks and failure messages.
    seen: Vec<String>,
    started: Instant,
    /// What the test typed that must never show up anywhere.
    secrets: Vec<String>,
    /// One-time codes typed: checked as whole numbers (entry sizes and dates hold digits).
    codes: Vec<String>,
}

impl Harness {
    fn new(name: &'static str) -> Option<Harness> {
        let fx = fixture(name)?;
        let started = Instant::now();
        let dir = tempfile::tempdir().unwrap();
        let config = write_ssh_config(fx, dir.path());

        let (tx, events) = mpsc::channel();
        let (backend, commands) = service::channel();
        let cfg = Config {
            ssh: dromon_ssh::ssh_program(),
            helper_exe: PathBuf::from(HELPER),
            verbose: false,
            config_file: Some(config.clone()),
            discover: Some(dromon_ssh::ConfigSources { user: Some(config), system: None, user_ssh_dir: dir.path().to_path_buf() }),
            hosts: Vec::new(),
            without_env: vec!["SSH_AUTH_SOCK".into()],
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");

        let mut rt = dromon_app::app().headless_runtime(SIZE, Env::new(true, 1.0));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        rt.update(move |s, _| *s = App::new(HostDirectory::default(), now, Box::new(backend)));
        rt.frame(0.0);
        let mut h = Harness { name, fx, dir, rt, events, seen: Vec::new(), started, secrets: Vec::new(), codes: Vec::new() };
        // Discovery: the four aliases, then what `ssh -F <config> -G` resolved for them.
        h.pump_until("the fixture's aliases in the sidebar, resolved", |rt| {
            ["fx-key", "fx-pw", "fx-kbd", "fx-enc"].iter().all(|a| {
                rt.find(&format!("host-{a}")).is_some()
                    && rt.state.hosts.get(&host(a)).is_some_and(|h| h.hostname.as_deref() == Some("127.0.0.1"))
            })
        });
        Some(h)
    }

    /// The home folder of `role` on the server (`/home/key` in Docker).
    fn home(&self, role: Role) -> String {
        self.fx.home(role).as_str().to_owned()
    }

    fn step(&self, what: &str) {
        note(format!("e2e {}: {what} ({:.2} s)", self.name, self.started.elapsed().as_secs_f64()));
    }

    // --- Events in -------------------------------------------------------------------------

    /// Lets springs and exits finish.
    fn settle(&mut self) {
        for _ in 0..4 {
            self.rt.frame(0.25);
        }
    }

    fn apply(&mut self, ev: ServiceEvent) {
        self.seen.push(format!("{ev:?}"));
        self.rt.update(move |app, _| app.apply(ev));
    }

    /// Applies every event that's there; settles if there were any.
    fn drain(&mut self) {
        let mut any = false;
        while let Ok(ev) = self.events.try_recv() {
            self.apply(ev);
            any = true;
        }
        if any {
            self.settle();
        }
    }

    /// Applies the service's events as they come, between frames, until `done` holds.
    fn pump_until(&mut self, what: &str, mut done: impl FnMut(&Runtime<App>) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            self.drain();
            if done(&self.rt) {
                return;
            }
            self.wait_one(deadline, what);
        }
    }

    /// Like [`Harness::pump_until`], until an event since `from` (its `Debug`) matches.
    fn pump_until_event(&mut self, from: usize, what: &str, pick: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            self.drain();
            if self.seen[from..].iter().any(|e| pick(e)) {
                return;
            }
            self.wait_one(deadline, what);
        }
    }

    /// Waits a little for the next event and applies it; panics past `deadline`.
    fn wait_one(&mut self, deadline: Instant, what: &str) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let last: Vec<&String> = self.seen.iter().rev().take(25).collect::<Vec<_>>().into_iter().rev().collect();
            panic!("{}: timed out after {WAIT:?} waiting for {what}\nlast events:\n{last:#?}\nconnection log:\n{}", self.name, self.log());
        }
        match self.events.recv_timeout(left.min(Duration::from_millis(100))) {
            Ok(ev) => {
                self.apply(ev);
                self.settle();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!("{}: the service thread is gone", self.name),
        }
    }

    // --- Input -----------------------------------------------------------------------------

    fn click(&mut self, k: &str) {
        let (x, y) = self.rt.find(k).unwrap_or_else(|| panic!("{}: no element {k:?}", self.name));
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.settle();
    }

    fn double_click(&mut self, k: &str) {
        let (x, y) = self.rt.find(k).unwrap_or_else(|| panic!("{}: no element {k:?}", self.name));
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.rt.frame(0.05);
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.settle();
    }

    fn key(&mut self, key: Key) {
        self.rt.key(KeyEvent { key, mods: Mods::default() });
        self.settle();
    }

    fn type_text(&mut self, s: &str) {
        self.key(Key::Text(s.into()));
    }

    /// Types into the open sheet's field and presses Return.
    fn answer(&mut self, s: &str) {
        assert_eq!(self.rt.presented_focus_key(), Some("sheet-input"), "{}: the sheet's field has the keyboard", self.name);
        self.type_text(s);
        self.key(Key::Enter);
    }

    // --- Reading back ----------------------------------------------------------------------

    fn shown(&self, k: &str) -> bool {
        shown(&self.rt, k)
    }

    fn text_of(&self, k: &str) -> String {
        text_of(&self.rt, k)
    }

    /// The open sheet's prompt.
    fn sheet_prompt(&self) -> Option<(PromptKind, String, Option<String>)> {
        match self.rt.state.ui.sheets.open()? {
            Sheet::Ask { prompt, .. } => Some((prompt.kind.clone(), prompt.text.clone(), prompt.error.clone())),
            _ => None,
        }
    }

    fn status(&self) -> Option<ConnStatus> {
        self.rt.state.conn().map(|c| c.status.clone())
    }

    fn log(&self) -> String {
        self.rt.state.conns.values().map(|c| c.log.to_text()).collect::<Vec<_>>().join("\n")
    }

    /// Waits for a sheet asking for `kind` (by variant) with `title`.
    fn wait_for_sheet(&mut self, title: &str, want: fn(&PromptKind) -> bool) -> (PromptKind, String, Option<String>) {
        let title = title.to_owned();
        self.pump_until(&format!("a sheet “{title}”"), |rt| {
            text_of(rt, "sheet-title") == title
                && matches!(rt.state.ui.sheets.open(), Some(Sheet::Ask { prompt, .. }) if want(&prompt.kind))
        });
        self.sheet_prompt().unwrap()
    }

    /// Trusts the host key on its sheet (Return) and returns the fingerprint it showed.
    fn trust_host_key(&mut self, alias: &str) -> String {
        let (kind, _, _) = self.wait_for_sheet(&format!("First connection to {alias}"), |k| matches!(k, PromptKind::HostKey { .. }));
        let shown = self.text_of("sheet-fingerprint");
        let PromptKind::HostKey { key_type, fingerprint } = kind else { unreachable!() };
        assert_eq!(key_type.as_deref(), Some("ED25519"));
        assert_eq!(fingerprint.as_deref(), Some(shown.as_str()));
        self.key(Key::Enter);
        shown
    }

    /// Waits until the folder `path` is listed in full and on screen.
    fn wait_for_listing(&mut self, path: &str) -> Vec<String> {
        let path = RemotePath::new(path);
        self.pump_until(&format!("the listing of {path}"), |rt| {
            rt.state.conn().is_some_and(|c| {
                c.status == ConnStatus::Ready && c.cwd == path && matches!(c.listing, Listing::Ready(_)) && !c.loading_more()
            }) && rt.find("rows").is_some()
        });
        self.rt.state.conn().unwrap().row_names().into_iter().map(str::to_owned).collect()
    }

    /// Clicks `alias` in the sidebar: it becomes the active host and starts connecting.
    fn connect(&mut self, alias: &str) {
        self.click(&format!("host-{alias}"));
        assert_eq!(self.rt.state.active, Some(host(alias)));
    }

    /// Asserts that nothing typed reached an event or the connection log.
    fn assert_no_secrets(&self) {
        let log = self.log();
        let haystacks = self.seen.iter().map(String::as_str).chain(std::iter::once(log.as_str()));
        let mut checked = 0;
        for hay in haystacks {
            for s in &self.secrets {
                assert!(!hay.contains(s.as_str()), "{}: a secret reached {hay:?}", self.name);
            }
            for code in &self.codes {
                assert!(!contains_number(hay, code), "{}: the code {code} reached {hay:?}", self.name);
            }
            checked += 1;
        }
        let lines = log.lines().count();
        self.step(&format!(
            "no secret in {} events or {lines} log lines ({} secrets, {} codes checked)",
            checked - 1,
            self.secrets.len(),
            self.codes.len()
        ));
    }
}

fn shown(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

fn text_of(rt: &Runtime<App>, k: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).and_then(|e| e.text).unwrap_or_default()
}

/// Whether `number` appears in `hay` on its own, not inside a longer run of digits.
fn contains_number(hay: &str, number: &str) -> bool {
    let bytes = hay.as_bytes();
    hay.match_indices(number).any(|(at, m)| {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(at + m.len()).copied();
        !before.is_some_and(|b| b.is_ascii_digit()) && !after.is_some_and(|b| b.is_ascii_digit())
    })
}

/// The test's ssh config: one alias per fixture user (ssh keeps the first value it reads, so
/// the per-host lines win), then the isolation settings for all of them.
fn write_ssh_config(fx: &Fixture, dir: &Path) -> PathBuf {
    // Copies of the keys in the test's folder (still private: mode 0600, on Windows an ACL for
    // the current user alone): a short path, since ssh cuts the path in its passphrase prompt
    // at 100 bytes.
    let key = dir.join("id_ed25519");
    let enc = dir.join("id_ed25519_enc");
    copy_private_key(&fx.key(), &key).unwrap();
    copy_private_key(&fx.encrypted_key(), &enc).unwrap();
    assert!(enc.to_str().unwrap().len() <= 100, "{}", enc.display());

    let port = fx.port();
    let hosts = [
        ("fx-key", Role::Key, Some(&key), "publickey"),
        ("fx-pw", Role::Password, None, "password"),
        ("fx-kbd", Role::Kbd, None, "keyboard-interactive"),
        ("fx-enc", Role::EncryptedKey, Some(&enc), "publickey"),
    ];
    let mut config = String::new();
    for (alias, role, identity, method) in hosts {
        writeln!(config, "Host {alias}\n    HostName {}\n    Port {port}\n    User {}", fx.host(), fx.user(role)).unwrap();
        match identity {
            Some(file) => writeln!(config, "    IdentityFile {}\n    PubkeyAuthentication yes", file.display()).unwrap(),
            None => writeln!(config, "    PubkeyAuthentication no").unwrap(),
        }
        writeln!(config, "    PreferredAuthentications {method}").unwrap();
    }
    writeln!(config, "Host *").unwrap();
    for line in [
        format!("UserKnownHostsFile {}", dir.join("known_hosts").display()),
        format!("GlobalKnownHostsFile {NULL_DEVICE}"),
        "StrictHostKeyChecking ask".into(),
        "UpdateHostKeys no".into(),
        "HashKnownHosts no".into(),
        "IdentitiesOnly yes".into(),
        "AddKeysToAgent no".into(),
        "NumberOfPasswordPrompts 3".into(),
    ] {
        writeln!(config, "    {line}").unwrap();
    }
    let path = dir.join("ssh_config");
    std::fs::write(&path, config).unwrap();
    path
}

/// The fixture's host key fingerprint, read from the host key itself on the server
/// (`ssh-keygen -l` there, as `dromon-ssh/tests/askpass_e2e.rs` does). Not `ssh-keyscan` here:
/// Windows' 9.5 offers a key exchange it can't do (sntrup761x25519) and gives up.
fn fixture_fingerprint(fx: &Fixture) -> String {
    let key = fx.host_public_key();
    let line = fx
        .exec(&["ssh-keygen", "-l", "-E", "sha256", "-f", &key])
        .unwrap_or_else(|e| panic!("the fingerprint of the server's host key {key}: {e}"));
    // `256 SHA256:… root@<host> (ED25519)`
    let fp = line.split_whitespace().nth(1).unwrap_or_default().to_owned();
    assert!(fp.starts_with("SHA256:"), "{line:?}");
    fp
}

/// Signs in to `fx-pw` with the right password (host key first) and waits for its home.
fn sign_in_pw(h: &mut Harness) {
    h.connect("fx-pw");
    h.trust_host_key("fx-pw");
    let (_, text, error) = h.wait_for_sheet("Sign in to fx-pw", |k| *k == PromptKind::Password);
    assert_eq!((text, error), (password_prompt(h.fx), None));
    h.secrets.push(PASSWORD.into());
    h.answer(PASSWORD);
    let rows = h.wait_for_listing(&h.home(Role::Password));
    assert!(rows.iter().any(|r| r == ".profile"), "{rows:?}");
}

/// `pw@127.0.0.1's password: ` with the fixture's name for the password user.
fn password_prompt(fx: &Fixture) -> String {
    format!("{}@127.0.0.1's password: ", fx.user(Role::Password))
}

/// a. A public-key host: the host-key sheet shows the server's real fingerprint; trusted, the
/// home folder lists; the mixed seed folder has a dir-like symlink, a dangling one and a
/// dotfile; a folder without access shows the permission-denied state.
#[test]
fn key_host_trust_list_and_browse() {
    let Some(mut h) = Harness::new("key_host_trust_list_and_browse") else { return };
    h.step("sidebar has fx-key, fx-pw, fx-kbd, fx-enc (SSH Config, resolved)");
    let spec = h.rt.state.hosts.get(&host("fx-key")).unwrap().clone();
    assert_eq!(spec.source, HostSource::SshConfig);
    assert_eq!((spec.user.as_deref(), spec.port), (Some(h.fx.user(Role::Key)), Some(h.fx.port())));

    let expected = fixture_fingerprint(h.fx);
    h.connect("fx-key");
    let fingerprint = h.trust_host_key("fx-key");
    assert_eq!(fingerprint, expected, "the sheet shows the server's own fingerprint");
    h.step(&format!("host-key sheet showed {fingerprint} (= the server's host key), trusted"));

    let key_home = h.home(Role::Key);
    let mixed = format!("{key_home}/fixture/mixed");
    let home = h.wait_for_listing(&key_home);
    // Read only now: ssh writes known_hosts a moment after it gets the answer, and once the
    // folder lists the connection is up, so the file is there (CI once read it too early).
    let known = std::fs::read_to_string(h.dir.path().join("known_hosts")).unwrap();
    assert!(known.starts_with(&format!("[127.0.0.1]:{} ssh-ed25519 ", h.fx.port())), "{known}");
    assert!(home.iter().any(|r| r == "fixture"), "{home:?}");
    assert!(!h.shown("sheet"), "connected: the sheet is gone");
    assert_eq!(h.text_of("status-summary"), format!("{} items", home.len()));
    h.step(&format!("listed {key_home}: {home:?}"));

    h.double_click("rows/fixture");
    h.wait_for_listing(&format!("{key_home}/fixture"));
    h.double_click("rows/mixed");
    let rows = h.wait_for_listing(&mixed);
    let c = h.rt.state.conn().unwrap();
    let kind = |name: &str| c.find(name).unwrap_or_else(|| panic!("no {name} in {rows:?}")).kind;
    assert_eq!(kind("link-dir"), EntryKind::Symlink { target: Some(LinkTarget::Dir) });
    assert!(kind("link-dir").is_dir_like());
    assert_eq!(kind("link-file"), EntryKind::Symlink { target: Some(LinkTarget::File) });
    assert_eq!(kind("dangling"), EntryKind::Symlink { target: None });
    assert_eq!(kind("locked"), EntryKind::Dir);
    assert!(rows.iter().any(|r| r == ".hidden"), "dotfiles are shown by default");
    assert_eq!(rows.len(), 100, "{rows:?}");
    h.step(&format!("listed fixture/mixed: {} entries incl. link-dir (→ dir), dangling, .hidden", rows.len()));

    // Type-to-select and Return open the symlinked folder.
    h.type_text("link-d");
    assert_eq!(h.rt.state.conn().unwrap().selection.cursor(), Some("link-dir"));
    h.key(Key::Enter);
    assert_eq!(h.wait_for_listing(&format!("{mixed}/link-dir")), ["inside.txt"]);
    h.click("tb-back");
    h.wait_for_listing(&mixed);
    h.step("opened link-dir (inside.txt) and came back");

    h.type_text("locked");
    assert_eq!(h.rt.state.conn().unwrap().selection.cursor(), Some("locked"));
    h.key(Key::Enter);
    h.pump_until("the permission-denied state", |rt| {
        rt.state.conn().is_some_and(|c| c.listing == Listing::Failed(ListError::PermissionDenied)) && shown(rt, "denied")
    });
    assert_eq!(h.status(), Some(ConnStatus::Ready), "a folder without access doesn't break the connection");
    h.step("opened locked: “You don't have access to this folder”");
    h.assert_no_secrets();
}

/// b. A wrong password asks again in place with the error; the right one lists the home.
#[test]
fn password_wrong_then_right() {
    let _turn = pw_turn();
    let Some(mut h) = Harness::new("password_wrong_then_right") else { return };
    h.connect("fx-pw");
    h.trust_host_key("fx-pw");
    let (_, text, error) = h.wait_for_sheet("Sign in to fx-pw", |k| *k == PromptKind::Password);
    assert_eq!((text, error), (password_prompt(h.fx), None));
    assert!(h.shown("sheet-note"), "a password may be remembered");
    h.secrets.extend([WRONG_PASSWORD.to_owned(), PASSWORD.to_owned()]);
    h.answer(WRONG_PASSWORD);
    h.step("typed a wrong password");
    h.pump_until("the retry with its error", |rt| text_of(rt, "sheet-error") == dromon_ssh::WRONG_PASSWORD);
    assert_eq!(h.status(), Some(ConnStatus::NeedsInput));
    h.step(&format!("sheet shows “{}”", dromon_ssh::WRONG_PASSWORD));
    h.answer(PASSWORD);
    let pw_home = h.home(Role::Password);
    let rows = h.wait_for_listing(&pw_home);
    assert!(!h.shown("sheet"));
    h.step(&format!("right password: listed {pw_home} {rows:?}"));
    h.assert_no_secrets();
}

/// c. Keyboard-interactive: the password, then a verification code that is never remembered.
#[test]
fn keyboard_interactive_password_then_code() {
    let Some(mut h) = Harness::new("keyboard_interactive_password_then_code") else { return };
    if !h.fx.supports(Feature::Totp, h.name) {
        return;
    }
    let kbd = h.fx.user(Role::Kbd);
    h.connect("fx-kbd");
    h.trust_host_key("fx-kbd");
    let (_, text, _) = h.wait_for_sheet("Sign in to fx-kbd", |k| *k == PromptKind::Password);
    // `… Password: `, or `… Password:` from Apple's OpenPAM: a password sheet either way.
    assert_eq!(text, h.fx.password_prompt(Role::Kbd));
    assert!(h.shown("sheet-note"), "the password is rememberable");
    h.secrets.push(PASSWORD.into());
    h.answer(PASSWORD);
    let (kind, text, _) = h.wait_for_sheet("Sign in to fx-kbd", |k| *k == PromptKind::Other);
    assert_eq!(text, format!("({kbd}@127.0.0.1) Verification code: "));
    assert!(!kind.rememberable());
    assert!(!h.shown("sheet-note"), "the code sheet doesn't offer to remember it");
    h.step("password sheet (remembered), then code sheet (not remembered)");
    let code = totp_now();
    h.codes.push(code.clone());
    h.answer(&code);
    let kbd_home = h.home(Role::Kbd);
    let rows = h.wait_for_listing(&kbd_home);
    h.step(&format!("code accepted: listed {kbd_home} {rows:?}"));
    h.assert_no_secrets();
}

/// d. An encrypted key: the passphrase sheet names the key file.
#[test]
fn encrypted_key_passphrase() {
    let Some(mut h) = Harness::new("encrypted_key_passphrase") else { return };
    // The path as the ssh config names it (`write_ssh_config`): ssh prints it as given. Not
    // canonicalized: on macOS the temp folder is under /var, a link to /private/var.
    let key = h.dir.path().join("id_ed25519_enc").to_str().unwrap().to_owned();
    h.connect("fx-enc");
    h.trust_host_key("fx-enc");
    let (kind, text, _) = h.wait_for_sheet("Sign in to fx-enc", |k| matches!(k, PromptKind::Passphrase { .. }));
    assert_eq!(kind, PromptKind::Passphrase { key_path: Some(key.clone()) });
    assert_eq!(text, format!("Enter passphrase for key '{key}': "));
    assert!(h.text_of("sheet-prompt").contains(&key), "the sheet names the key");
    h.step(&format!("passphrase sheet names {key}"));
    h.secrets.push(PASSPHRASE.into());
    h.answer(PASSPHRASE);
    let enc_home = h.home(Role::EncryptedKey);
    let rows = h.wait_for_listing(&enc_home);
    h.step(&format!("listed {enc_home} {rows:?}"));
    h.assert_no_secrets();
}

/// e. The server ends the session: the app reconnects on its own, answers the password from
/// memory (no sheet), and lists the folder again.
#[test]
fn server_drop_reconnects_silently() {
    let _turn = pw_turn();
    let Some(mut h) = Harness::new("server_drop_reconnects_silently") else { return };
    sign_in_pw(&mut h);
    h.step("signed in to fx-pw");

    let mark = h.seen.len();
    let pw = h.fx.user(Role::Password);
    let pw_home = h.home(Role::Password);
    let killed = h.fx.kill_sessions(Role::Password).unwrap();
    assert!(killed > 0, "{pw} had no processes to kill");
    h.step(&format!("killed {killed} processes of {pw} on the server"));
    h.pump_until("Reconnecting", |rt| matches!(rt.state.conn().map(|c| &c.status), Some(ConnStatus::Reconnecting { .. })));
    let banner = h.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("banner") && !e.exiting).and_then(|e| e.label);
    assert_eq!(banner.as_deref(), Some("Reconnecting to fx-pw…"), "the reconnect banner shows");
    let status = h.status();
    h.step(&format!("status {status:?}, banner “{}”", banner.unwrap_or_default()));
    let name = h.name;
    h.pump_until("Ready again, the folder listed again", |rt| {
        assert!(!rt.state.ui.sheets.is_open(), "{name}: a sheet opened during the reconnect");
        rt.state.conn().is_some_and(|c| c.status == ConnStatus::Ready && matches!(c.listing, Listing::Ready(_)) && !c.loading_more())
    });
    // The refresh after the reconnect: a whole new listing of the folder on screen.
    h.pump_until_event(mark, &format!("the relisting of {pw_home}"), |e| {
        e.starts_with("ListingPage") && e.contains(&pw_home) && e.contains("done: true")
    });
    h.drain();
    let since = &h.seen[mark..];
    assert!(!since.iter().any(|e| e.starts_with("Prompt(")), "no prompt after the drop: {since:#?}");
    assert!(since.iter().any(|e| e.starts_with("ListingPage") && e.contains(&pw_home) && e.contains("first: true")), "{since:#?}");
    let rows = h.rt.state.conn().unwrap().row_names().into_iter().map(str::to_owned).collect::<Vec<_>>();
    assert!(rows.iter().any(|r| r == ".profile"), "{rows:?}");
    assert!(h.log().contains("answered from memory"), "{}", h.log());
    assert!(h.log().contains("Reconnected."));
    assert!(!h.shown("banner"));
    h.step(&format!("Ready again with no sheet; password answered from memory; {pw_home} listed again"));
    h.assert_no_secrets();
}

/// f. Disconnect from the connection log forgets the password: connecting again asks for it.
#[test]
fn disconnect_forgets_the_password() {
    let _turn = pw_turn();
    let Some(mut h) = Harness::new("disconnect_forgets_the_password") else { return };
    sign_in_pw(&mut h);
    h.step("signed in to fx-pw");

    h.click("status-conn");
    assert!(h.shown("log-lines"), "the status opens the log");
    assert!(h.text_of("log-lines").contains("Connected."), "{}", h.text_of("log-lines"));
    h.click("log-disconnect");
    h.pump_until("Disconnected", |rt| rt.state.conn().is_some_and(|c| c.status == ConnStatus::Disconnected));
    h.click("log-close");
    assert!(!h.shown("sheet"));
    assert!(h.shown("disconnected"));
    h.step("disconnected from the log sheet");

    let mark = h.seen.len();
    h.click("state-connect");
    let (_, _, error) = h.wait_for_sheet("Sign in to fx-pw", |k| *k == PromptKind::Password);
    assert_eq!(error, None);
    assert!(h.seen[mark..].iter().any(|e| e.starts_with("Prompt(")), "the password is asked, not answered from memory");
    assert!(!h.log().contains("answered from memory"), "{}", h.log());
    h.step("Connect asks for the password again");
    h.answer(PASSWORD);
    h.wait_for_listing(&h.home(Role::Password));
    h.step("signed in again");
    h.assert_no_secrets();
}

/// ⌘ on macOS, Ctrl elsewhere, as the platform reports the primary modifier.
fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

/// g. File operations through the UI on the real server: ⌘⇧N + a name + Return makes a folder;
/// F2 renames it; ⌘C then ⌘V of a file in its own folder makes “name (1)” (a server-side
/// copy); Delete through the sheet removes the folder. Each checked on the server itself.
#[test]
fn file_operations_from_the_keyboard() {
    let Some(mut h) = Harness::new("file_operations_from_the_keyboard") else { return };
    let fx = h.fx;
    let scratch = fx.path(Role::Key, &format!("scratch/e2e-fileops-{}", std::process::id()));
    let dir = scratch.as_str().to_owned();
    let make = format!("rm -rf '{dir}' && mkdir -p '{dir}' && printf 'hello' > '{dir}/page.txt' && chown -R {} '{dir}'", fx.owner(Role::Key));
    fx.exec(&["sh", "-c", &make]).unwrap();
    let on_server = |test: &str, rel: &str| fx.exec(&["sh", "-c", &format!("test {test} '{dir}/{rel}' && echo yes || echo no")]).unwrap().trim() == "yes";
    // The OS clipboard: an in-memory one, as a test's.
    h.rt.use_clipboard(&rune::clipboard::memory());

    h.connect("fx-key");
    h.trust_host_key("fx-key");
    h.wait_for_listing(&h.home(Role::Key));
    let to = scratch.clone();
    h.rt.update(move |s, _| {
        s.act(dromon_app::state::Action::GoTo(to));
    });
    assert_eq!(h.wait_for_listing(&dir), ["page.txt"]);
    h.step(&format!("listed {dir}"));

    let mods = |shift: bool| Mods { shift, ..primary() };
    h.rt.key(KeyEvent { key: Key::Command('n'), mods: mods(true) });
    h.settle();
    assert_eq!(h.rt.presented_focus_key(), Some("edit-input"), "the new folder's field has the keyboard");
    h.type_text("made");
    h.key(Key::Enter);
    h.pump_until("the new folder listed and selected", |rt| {
        rt.state.conn().is_some_and(|c| c.find("made").is_some() && c.selection.contains("made")) && !shown(rt, "edit-input")
    });
    assert!(on_server("-d", "made"), "the folder exists on the server");
    h.step("⌘⇧N made “made”");

    h.key(Key::Function(2));
    assert_eq!(h.rt.presented_focus_key(), Some("edit-input"));
    h.type_text("renamed");
    h.key(Key::Enter);
    h.pump_until("the renamed folder listed and selected", |rt| {
        rt.state.conn().is_some_and(|c| c.find("renamed").is_some() && c.find("made").is_none() && c.selection.contains("renamed"))
    });
    assert!(on_server("-d", "renamed") && !on_server("-e", "made"), "renamed on the server");
    h.step("F2 renamed it to “renamed”");

    h.click("rows/page.txt");
    h.rt.key(KeyEvent { key: Key::Command('c'), mods: mods(false) });
    h.settle();
    h.rt.key(KeyEvent { key: Key::Command('v'), mods: mods(false) });
    h.settle();
    h.pump_until("the copy job done", |rt| {
        rt.state.transfers.jobs.iter().any(|j| j.kind == dromon_transfer::JobKind::RemoteCopy && j.state == dromon_transfer::TaskState::Done)
    });
    assert!(on_server("-f", "page (1).txt"), "the copy is on the server as “page (1).txt”");
    assert_eq!(fx.exec(&["cat", &format!("{dir}/page (1).txt")]).unwrap(), "hello");
    // No refresh: the job's end lists the folder on screen again by itself.
    h.pump_until("the copy listed", |rt| rt.state.conn().is_some_and(|c| c.find("page (1).txt").is_some()));
    h.step("⌘C ⌘V copied page.txt to “page (1).txt”, listed when the job ended");
    // ⌘R lists it again too (here: nothing changed).
    h.rt.key(KeyEvent { key: Key::Command('r'), mods: mods(false) });
    h.settle();
    h.pump_until("still listed after ⌘R", |rt| rt.state.conn().is_some_and(|c| c.find("page (1).txt").is_some()));

    h.click("rows/renamed");
    if cfg!(target_os = "macos") {
        h.rt.key(KeyEvent { key: Key::Backspace, mods: primary() });
        h.settle();
    } else {
        h.key(Key::Delete);
    }
    assert_eq!(h.text_of("sheet-title"), "Delete ‘renamed’ permanently?");
    h.click("delete-confirm");
    h.pump_until("the folder gone from the listing", |rt| rt.state.conn().is_some_and(|c| c.find("renamed").is_none()));
    assert!(!on_server("-e", "renamed"), "deleted on the server");
    assert!(h.rt.state.toasts().is_empty(), "the delete's toast went when it was done");
    h.step("the delete sheet removed “renamed”");
    let _ = fx.exec(&["rm", "-rf", &dir]);
}

#[test]
fn numbers_are_matched_whole() {
    assert!(contains_number("code 123456 sent", "123456"));
    assert!(contains_number("123456", "123456"));
    assert!(!contains_number("mtime: 1712345678", "123456"));
    assert!(!contains_number("1234567", "123456"));
}
