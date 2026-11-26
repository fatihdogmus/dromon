//! The sshd fixture for Dromon's integration tests: one OpenSSH server per test process (test
//! binary), shared by all its tests, with the users, keys and seed data of `tests/docker/`.
//!
//! Two backends, picked by `DROMON_FIXTURE_BACKEND` ([`BACKEND_ENV`]):
//!
//! - `docker` (the default): the Debian image built from `tests/docker/` and run with
//!   testcontainers (`docker.rs`). Users `key`, `pw`, `kbd`, `keypw`, `enc` in `/home/<user>`.
//! - `native`: a private `sshd` of the machine itself, run as root through `sudo -n`, for CI
//!   runners without Docker (GitHub's macOS runners; `native.rs`). Its users are prefixed
//!   (`dromon_key`, …) so they can't collide with real accounts, and live in the platform's home
//!   folder (`/Users/dromon_key`, `/home/dromon_key`). It creates system users, so it refuses to
//!   start unless `CI` or `DROMON_FIXTURE_NATIVE_OK=1` ([`NATIVE_OK_ENV`]) is set.
//!
//! Tests never name users or home folders themselves: [`Fixture::user`], [`Fixture::home`] and
//! [`Fixture::destination`] take a [`Role`]. What only one backend can do is asked through
//! [`Fixture::supports`], which prints the skip line.
//!
//! [`fixture`] returns `None` after printing a skip line when the backend can't run here (Docker
//! not reachable; native: no CI, no sudo, no sshd), or panics instead with
//! `DROMON_REQUIRE_DOCKER=1` ([`REQUIRE_DOCKER_ENV`], for either backend), and whenever the
//! backend can run but the fixture doesn't start.
//!
//! Both backends generate this process's two ed25519 client keypairs with `ssh-keygen` into a
//! fresh temp folder (`dromon-testkit-<pid>-…`; folders of processes that are gone are deleted on
//! the next start), end the server when the test process exits (statics are never dropped: a
//! lifeline connection for Docker, a root watchdog for native), and check that a public-key login
//! works before the first test runs.
//!
//! Debugging: `DROMON_FIXTURE_KEEP=1` ([`KEEP_ENV`]) keeps the server (and the key folder) after
//! the tests; the start line prints the port, key folder and how to clean up.
//!
//! Windows: Docker only (the same Linux image, on a Linux Docker named by `DOCKER_HOST`; CI:
//! Docker in WSL2 on `tcp://127.0.0.1:2375`; WSL forwards the published 127.0.0.1 ports to
//! Windows). The ssh tools are the inbox OpenSSH ([`openssh`]) and the null device is `NUL`
//! ([`NULL_DEVICE`]).
//!
//! **Server matrix.** Besides that default server ([`Variant::Trixie`]) the Docker backend runs
//! other servers on request, one container (set) per variant and test process, started on first
//! use with [`fixture_of`]: OpenSSH 8.4p1 without `copy-data` ([`Variant::Bullseye`]), Alpine's
//! musl OpenSSH ([`Variant::Alpine`]), and the default server behind toxiproxy
//! ([`Variant::Toxic`], [`Fixture::toxiproxy`]). Same users, keys and seed data; what a variant
//! lacks is asked through [`Fixture::has`] / [`Fixture::supports`]. [`matrix::run`] runs one
//! check against every variant of [`matrix::variants`]: only the default one unless
//! `DROMON_SERVER_MATRIX=1` ([`matrix::MATRIX_ENV`]). See `docs/test-servers.md`.

use std::fmt;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dromon_core::RemotePath;
use tokio::sync::{OnceCell, RwLock};

mod docker;
pub mod matrix;
mod native;
mod totp;
pub mod toxiproxy;

pub use matrix::Variant;
pub use toxiproxy::Toxiproxy;
pub use totp::{STEP as TOTP_STEP, totp_at, totp_now, totp_secret, wrong_totp};

/// The fixture's users, by how they log in. [`Fixture::user`] names the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Public key only, no password; owns the seed data (`<home>/fixture`).
    Key,
    /// The SSH `password` method only: prompt `<user>@<host>'s password: `.
    Password,
    /// Keyboard-interactive through PAM: [`Fixture::password_prompt`] ([`PASSWORD`]):
    /// `(<user>@<host>) Password: `, or `Password:` without the space where the server's PAM is
    /// Apple's OpenPAM (native macOS); then, with [`Feature::Totp`],
    /// `(<user>@<host>) Verification code: ` ([`totp_now`]).
    Kbd,
    /// Two methods: public key ([`Fixture::encrypted_key`]), then password ([`PASSWORD`]).
    KeyThenPassword,
    /// Public key with the encrypted private key ([`Fixture::encrypted_key`], [`PASSPHRASE`]).
    EncryptedKey,
}

impl Role {
    pub const ALL: [Role; 5] = [Role::Key, Role::Password, Role::Kbd, Role::KeyThenPassword, Role::EncryptedKey];

    /// The account name in the Docker image (`tests/docker/Dockerfile`); the native backend
    /// prefixes it.
    pub fn base_name(self) -> &'static str {
        match self {
            Role::Key => "key",
            Role::Password => "pw",
            Role::Kbd => "kbd",
            Role::KeyThenPassword => "keypw",
            Role::EncryptedKey => "enc",
        }
    }

    /// Whether the account has [`PASSWORD`] (else it has no usable password).
    fn has_password(self) -> bool {
        matches!(self, Role::Password | Role::Kbd | Role::KeyThenPassword)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// What not every backend or [`Variant`] offers; asked through [`Fixture::has`] and
/// [`Fixture::supports`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// A TOTP second factor for [`Role::Kbd`] (libpam-google-authenticator, [`totp_now`]): Docker
    /// (not [`Variant::Alpine`], whose sshd has no PAM), and the native backend where that
    /// module is installed (macOS CI: Homebrew's `google-authenticator-libpam`; Debian:
    /// `libpam-google-authenticator`) and sshd has `PAMServiceName` (OpenSSH 9.8+).
    Totp,
    /// sftp-server advertises `copy-data` (OpenSSH 9.0+): not [`Variant::Bullseye`].
    CopyData,
    /// sftp-server advertises `limits@openssh.com` (OpenSSH 8.6+): not [`Variant::Bullseye`].
    Limits,
    /// sftp-server advertises `posix-rename@openssh.com`: every server here.
    PosixRename,
    /// toxiproxy sits between the client and sshd ([`Fixture::toxiproxy`]): [`Variant::Toxic`].
    Toxics,
}

/// Which server the fixture runs ([`BACKEND_ENV`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Docker,
    Native,
}

/// The password of the [`Role::Password`], [`Role::Kbd`] and [`Role::KeyThenPassword`] users.
pub const PASSWORD: &str = "dromon-test-pw";
/// The passphrase of [`Fixture::encrypted_key`].
pub const PASSPHRASE: &str = "dromon-test-passphrase";
/// The comment of [`Fixture::key`].
pub const KEY_COMMENT: &str = "dromon-fixture";
/// The comment of [`Fixture::encrypted_key`].
pub const ENCRYPTED_KEY_COMMENT: &str = "dromon-fixture-enc";
/// The address sshd listens on (Docker: the ports are published there).
pub const HOST: &str = "127.0.0.1";
/// The 1 GiB random file made by [`Fixture::big_file`], relative to the [`Role::Key`] home.
pub const BIG_FILE: &str = "fixture/big/big.bin";
pub const BIG_FILE_SIZE: u64 = 1 << 30;

/// `docker` (default) or `native`.
pub const BACKEND_ENV: &str = "DROMON_FIXTURE_BACKEND";
/// Set to `1` to fail, instead of skip, the fixture tests when the backend can't run here
/// (Docker isn't reachable; native: not allowed, no sudo, no sshd).
pub const REQUIRE_DOCKER_ENV: &str = "DROMON_REQUIRE_DOCKER";
/// Set to `1` to allow the native backend outside CI (it creates system users and runs sshd as
/// root: use a throwaway VM or container).
pub const NATIVE_OK_ENV: &str = "DROMON_FIXTURE_NATIVE_OK";
/// Set to `1` to keep the server running after the tests.
pub const KEEP_ENV: &str = "DROMON_FIXTURE_KEEP";
/// The image name; the tag is a hash of the build context.
pub const IMAGE_NAME: &str = "dromon-testkit-sshd";
/// Label on every fixture container; its value is the test process id.
pub const LABEL: &str = "dev.dromon.testkit.pid";
/// The file ssh options name to read or write nothing: `/dev/null`, on Windows `NUL`.
pub const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

const KEY_DIR_PREFIX: &str = "dromon-testkit-";
/// In a key folder: kept by `DROMON_FIXTURE_KEEP=1`, never swept.
const KEEP_MARKER: &str = "kept";
const KEY: &str = "id_ed25519";
const ENCRYPTED_KEY: &str = "id_ed25519_enc";

/// One fixture user on the server.
#[derive(Debug)]
struct Account {
    name: String,
    home: String,
    group: String,
}

/// What differs between the backends' servers.
#[derive(Debug)]
enum Server {
    Docker(docker::Container),
    Native(native::Sshd),
}

/// The running sshd fixture of this test process.
pub struct Fixture {
    port: u16,
    keys: PathBuf,
    /// By [`Role::index`].
    accounts: Vec<Account>,
    server: Server,
    variant: Variant,
    /// The server's SSH identification line, read at start.
    banner: String,
    /// [`Variant::Toxic`]: the proxy in front of sshd ([`Fixture::port`] is its port).
    toxiproxy: Option<Toxiproxy>,
    /// Held shared by [`matrix::run`] checks, exclusively by [`matrix::run_exclusive`] ones
    /// (a toxiproxy reset ends every connection through the proxy).
    access: RwLock<()>,
    big_file: OnceCell<Result<(), String>>,
}

impl fmt::Debug for Fixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fixture")
            .field("variant", &self.variant)
            .field("port", &self.port)
            .field("keys", &self.keys)
            .field("accounts", &self.accounts)
            .field("server", &self.server)
            .field("toxiproxy", &self.toxiproxy)
            .finish_non_exhaustive()
    }
}

enum State {
    Ready(Box<Fixture>),
    /// Docker doesn't answer: why.
    Unreachable(String),
    /// The backend (or this variant on it) can't run here: the whole reason.
    Unavailable(String),
    Failed(String),
}

/// Why a fixture isn't there and the test should skip (the skip line is already printed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Skipped(pub(crate) String);

/// This process's fixture ([`Variant::Trixie`]), started on first use. `None` (after printing a
/// skip line naming `test`) when the backend can't run here; panics instead with
/// `DROMON_REQUIRE_DOCKER=1`, and whenever the backend can run but the fixture doesn't start.
pub async fn fixture(test: &str) -> Option<&'static Fixture> {
    fixture_of(Variant::Trixie, test).await
}

/// This process's fixture for `variant` (one per variant, each started on first use), as
/// [`fixture`]. Variants other than [`Variant::Trixie`] need the Docker backend: with
/// `DROMON_FIXTURE_BACKEND=native` they skip (or fail with `DROMON_REQUIRE_DOCKER=1`).
pub async fn fixture_of(variant: Variant, test: &str) -> Option<&'static Fixture> {
    try_fixture(variant, test).await.ok()
}

pub(crate) async fn try_fixture(variant: Variant, test: &str) -> Result<&'static Fixture, Skipped> {
    static STATES: [OnceCell<State>; Variant::ALL.len()] = [const { OnceCell::const_new() }; Variant::ALL.len()];
    match STATES[variant.index()].get_or_init(|| start_in_thread(variant)).await {
        State::Ready(fixture) => Ok(&**fixture),
        State::Unreachable(why) if !env_flag(REQUIRE_DOCKER_ENV) => {
            note(format_args!(
                "skipping {test}: Docker is not reachable ({why}); set {REQUIRE_DOCKER_ENV}=1 to fail instead"
            ));
            Err(Skipped(format!("Docker is not reachable ({why})")))
        }
        State::Unreachable(why) => panic!("{test}: {REQUIRE_DOCKER_ENV}=1 but Docker is not reachable: {why}"),
        State::Unavailable(why) if !env_flag(REQUIRE_DOCKER_ENV) => {
            note(format_args!("skipping {test}: {why}; set {REQUIRE_DOCKER_ENV}=1 to fail instead"));
            Err(Skipped(why.clone()))
        }
        State::Unavailable(why) if native_backend() => {
            panic!("{test}: {REQUIRE_DOCKER_ENV}=1 but the native sshd fixture can't run: {why}")
        }
        State::Unavailable(why) => panic!("{test}: {REQUIRE_DOCKER_ENV}=1 but the {} sshd fixture can't run: {why}", variant.name()),
        State::Failed(why) => panic!("{test}: the {} sshd fixture didn't start: {why}", variant.name()),
    }
}

impl Fixture {
    fn assemble(
        port: u16,
        keys: PathBuf,
        accounts: Vec<Account>,
        server: Server,
        variant: Variant,
        banner: String,
        toxiproxy: Option<Toxiproxy>,
    ) -> Fixture {
        Fixture {
            port,
            keys,
            accounts,
            server,
            variant,
            banner,
            toxiproxy,
            access: RwLock::new(()),
            big_file: OnceCell::new(),
        }
    }

    /// Which server this is.
    pub fn variant(&self) -> Variant {
        self.variant
    }

    /// The server's SSH identification line, e.g. `SSH-2.0-OpenSSH_8.4p1 Debian-5+deb11u7`.
    pub fn banner(&self) -> &str {
        &self.banner
    }

    /// The toxiproxy in front of sshd ([`Variant::Toxic`] only, see [`Feature::Toxics`]).
    pub fn toxiproxy(&self) -> Option<&Toxiproxy> {
        self.toxiproxy.as_ref()
    }

    /// The address sshd is reached on, `127.0.0.1`.
    pub fn host(&self) -> &'static str {
        HOST
    }

    /// The port on [`HOST`] that reaches sshd.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// sshd's port as seen from the server itself (for a jump through the fixture to itself):
    /// Docker 22 inside the container, native the same as [`Fixture::port`].
    pub fn port_on_server(&self) -> u16 {
        match &self.server {
            Server::Docker(_) => docker::SSH_PORT,
            Server::Native(_) => self.port,
        }
    }

    pub fn backend(&self) -> Backend {
        match &self.server {
            Server::Docker(_) => Backend::Docker,
            Server::Native(_) => Backend::Native,
        }
    }

    /// Whether this fixture offers `feature`. When it doesn't, prints `skipping <test>: <why>`:
    /// the test then returns and passes. The native backend has [`Feature::Totp`] when its start
    /// set it up (the start line says which module, or why not).
    pub fn supports(&self, feature: Feature, test: &str) -> bool {
        match self.lacks(feature) {
            None => true,
            Some(why) => {
                note(format_args!("skipping {test}: {why}"));
                false
            }
        }
    }

    /// Whether this fixture offers `feature`, silently (to check what a session detects).
    pub fn has(&self, feature: Feature) -> bool {
        self.lacks(feature).is_none()
    }

    /// Why this fixture doesn't offer `feature`, or `None` when it does.
    pub fn lacks(&self, feature: Feature) -> Option<String> {
        let old = || format!("OpenSSH 8.4p1 ({}) has no", self.variant.name());
        match (feature, &self.server) {
            (Feature::Totp, Server::Native(sshd)) if !sshd.has_totp() => {
                Some("the native sshd has no TOTP PAM module (Docker fixture only)".into())
            }
            (Feature::Totp, Server::Docker(_)) if self.variant == Variant::Alpine => {
                Some("Alpine's sshd has no PAM (no keyboard-interactive, no TOTP)".into())
            }
            (Feature::CopyData, _) if self.variant == Variant::Bullseye => Some(format!("{} copy-data", old())),
            (Feature::Limits, _) if self.variant == Variant::Bullseye => Some(format!("{} limits@openssh.com", old())),
            (Feature::Toxics, _) if self.toxiproxy.is_none() => {
                Some(format!("no toxiproxy in front of the {} server (the toxic variant only)", self.variant.name()))
            }
            _ => None,
        }
    }

    /// The account name of `role`: `key` etc. in Docker, `dromon_key` etc. natively.
    pub fn user(&self, role: Role) -> &str {
        &self.accounts[role.index()].name
    }

    /// The home folder of `role` on the server.
    pub fn home(&self, role: Role) -> RemotePath {
        RemotePath::new(&self.accounts[role.index()].home)
    }

    /// `rel` (e.g. `fixture/mixed`) under the home folder of `role`.
    pub fn path(&self, role: Role, rel: &str) -> RemotePath {
        self.home(role).join(rel)
    }

    /// `user:group` of `role`, for `chown` in [`Fixture::exec`].
    pub fn owner(&self, role: Role) -> String {
        let account = &self.accounts[role.index()];
        format!("{}:{}", account.name, account.group)
    }

    /// The unencrypted private key, authorized for [`Role::Key`].
    pub fn key(&self) -> PathBuf {
        self.keys.join(KEY)
    }

    /// The private key encrypted with [`PASSPHRASE`], authorized for [`Role::EncryptedKey`] and
    /// [`Role::KeyThenPassword`].
    pub fn encrypted_key(&self) -> PathBuf {
        self.keys.join(ENCRYPTED_KEY)
    }

    /// The server's ed25519 public host key, a path on the server (for [`Fixture::exec`]).
    pub fn host_public_key(&self) -> String {
        match &self.server {
            Server::Docker(_) => docker::HOST_PUBLIC_KEY.to_owned(),
            Server::Native(sshd) => sshd.host_public_key(),
        }
    }

    /// The Docker container's id (`None` natively).
    pub fn container_id(&self) -> Option<&str> {
        match &self.server {
            Server::Docker(c) => Some(&c.id),
            Server::Native(_) => None,
        }
    }

    /// `name:tag` of the image the container runs (`None` natively).
    pub fn image(&self) -> Option<&str> {
        match &self.server {
            Server::Docker(c) => Some(&c.image),
            Server::Native(_) => None,
        }
    }

    /// `<user of role>@127.0.0.1`.
    pub fn destination(&self, role: Role) -> String {
        format!("{}@{HOST}", self.user(role))
    }

    /// The password prompt ssh shows (its askpass gets) when signing in as `role` to [`HOST`]:
    /// - [`Role::Kbd`] (keyboard-interactive): ssh's `(<user>@127.0.0.1) ` before the server PAM's
    ///   text, which is Linux-PAM's `pam_unix` `Password: ` (Docker, native Linux) or Apple's
    ///   OpenPAM `Password:`, without a space (native macOS);
    /// - [`Role::Password`], [`Role::KeyThenPassword`] (the SSH `password` method): ssh's own
    ///   `<user>@127.0.0.1's password: `.
    ///
    /// Panics for the roles without a password.
    pub fn password_prompt(&self, role: Role) -> String {
        match role {
            Role::Kbd => {
                let pam = match &self.server {
                    Server::Docker(_) => "Password: ",
                    Server::Native(sshd) => sshd.pam_password_prompt(),
                };
                format!("({}) {pam}", self.destination(role))
            }
            Role::Password | Role::KeyThenPassword => format!("{}'s password: ", self.destination(role)),
            Role::Key | Role::EncryptedKey => panic!("{role:?} has no password"),
        }
    }

    /// Non-interactive ssh options for a public-key login with [`Fixture::key`], isolated from
    /// the developer's ssh config, known hosts and agent keys: add `destination` and a command
    /// or `-s … sftp`.
    pub fn ssh_options(&self) -> Vec<String> {
        let mut args: Vec<String> = ["-T", "-x", "-e", "none", "-F", NULL_DEVICE].map(String::from).into();
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=no",
            &format!("UserKnownHostsFile={NULL_DEVICE}"),
            &format!("GlobalKnownHostsFile={NULL_DEVICE}"),
            "LogLevel=ERROR",
            "IdentitiesOnly=yes",
            "ConnectTimeout=10",
        ] {
            args.extend(["-o".into(), option.into()]);
        }
        args.extend(["-i".into(), self.key().to_string_lossy().into_owned(), "-p".into(), self.port.to_string()]);
        args
    }

    /// Runs `command` on the server as root; its stdout, or why it failed. Docker: inside the
    /// container, through the Docker API (the client testcontainers uses), not the `docker`
    /// CLI: on Windows, the CLI talking to WSL's Docker over TCP ran commands but returned none
    /// of their output. Native: `sudo -n <command>` in `/`.
    ///
    /// Blocking; callable from async tests too (it runs on a thread of its own).
    pub fn exec(&self, command: &[&str]) -> Result<String, String> {
        match &self.server {
            Server::Docker(c) => c.exec(command),
            Server::Native(sshd) => sshd.exec(command),
        }
    }

    /// SHA-1 of a file on the server as lowercase hex (`sha1sum`, on macOS `shasum -a 1`, as
    /// root through [`Fixture::exec`]): compare with [`file_sha1`].
    pub fn sha1(&self, path: &str) -> Result<String, String> {
        let out = if self.bsd_tools() { self.exec(&["shasum", "-a", "1", path])? } else { self.exec(&["sha1sum", path])? };
        out.split_whitespace().next().map(str::to_owned).ok_or_else(|| format!("no hash for {path}: {out:?}"))
    }

    /// Ends every ssh session of `role` from the server's side, as a server restart does:
    /// `pkill -u <user>` (SIGTERM to its `sshd-session` and `sftp-server`). Returns how many
    /// processes were signalled. Other users' sessions are untouched; tests sharing `role`
    /// must not run at the same time.
    pub fn kill_sessions(&self, role: Role) -> Result<usize, String> {
        // pgrep and pkill exit 1 when nothing matches.
        let pids = self.exec(&["sh", "-c", "pgrep -u \"$1\"; pkill -u \"$1\"; exit 0", "sh", self.user(role)])?;
        Ok(pids.lines().filter(|l| !l.trim().is_empty()).count())
    }

    /// Creates [`BIG_FILE`] in the [`Role::Key`] home (1 GiB of random data, owned by that user)
    /// on first call and returns its path; later calls return at once. Only the slow `#[ignore]`
    /// tests need it, so it is not part of the seed.
    pub async fn big_file(&self) -> String {
        let path = self.path(Role::Key, BIG_FILE).as_str().to_owned();
        let made = self
            .big_file
            .get_or_init(|| async {
                let start = Instant::now();
                let size = if self.bsd_tools() { "stat -f %z" } else { "stat -c %s" };
                let script = format!(
                    "set -e; f={path}; [ \"$({size} $f 2>/dev/null || echo 0)\" = {BIG_FILE_SIZE} ] && exit 0; \
                     mkdir -p \"${{f%/*}}\"; head -c {BIG_FILE_SIZE} /dev/urandom >\"$f.tmp\"; mv \"$f.tmp\" \"$f\""
                );
                let out = tokio::process::Command::new(openssh("ssh"))
                    .args(self.ssh_options())
                    .arg(self.destination(Role::Key))
                    .arg(script)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .map_err(|e| format!("spawn ssh: {e}"))?;
                if !out.status.success() {
                    return Err(format!("{}: {}", out.status, String::from_utf8_lossy(&out.stderr)));
                }
                note(format_args!("dromon-testkit: {path} ready after {:.1} s", start.elapsed().as_secs_f64()));
                Ok(())
            })
            .await;
        if let Err(e) = made {
            panic!("creating {path} failed: {e}");
        }
        path
    }

    /// Whether the server's tools are BSD's (`stat -f`, `shasum`): the native backend on macOS.
    fn bsd_tools(&self) -> bool {
        matches!(&self.server, Server::Native(sshd) if sshd.bsd_tools())
    }
}

/// An OpenSSH client tool (`ssh`, `ssh-keygen`, `ssh-keyscan`, `scp`): on Windows the inbox
/// OpenSSH's `C:\Windows\System32\OpenSSH\<name>.exe` when present (the folder of
/// `dromon_ssh::ssh_program`), else `<name>.exe` from `PATH`; elsewhere `<name>` from `PATH`.
pub fn openssh(name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let inbox = Path::new(r"C:\Windows\System32\OpenSSH").join(format!("{name}.exe"));
        if inbox.is_file() {
            return inbox;
        }
        PathBuf::from(format!("{name}.exe"))
    }
    #[cfg(not(windows))]
    PathBuf::from(name)
}

/// Copies a private key so ssh still accepts the copy. Unix keeps the 0600 mode. On Windows the
/// copy would get its folder's inherited ACL, and Win32-OpenSSH refuses a key other users may
/// read ("UNPROTECTED PRIVATE KEY FILE"), so the copy's ACL becomes the current user alone.
pub fn copy_private_key(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::copy(src, dst).map_err(|e| format!("copy {} to {}: {e}", src.display(), dst.display()))?;
    #[cfg(windows)]
    restrict_to_current_user(dst)?;
    Ok(())
}

/// `icacls <path> /inheritance:r /grant:r <user>:F`: no inherited entries, full control for the
/// current user and nobody else.
#[cfg(windows)]
fn restrict_to_current_user(path: &Path) -> Result<(), String> {
    let user = std::env::var("USERNAME").map_err(|_| "USERNAME is not set".to_owned())?;
    let out = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("spawn icacls: {e}"))?;
    if !out.status.success() {
        return Err(format!("icacls {}: {}: {}", path.display(), out.status, String::from_utf8_lossy(&out.stdout).trim()));
    }
    Ok(())
}

/// SHA-1 of a local file as lowercase hex, the format of [`Fixture::sha1`]: an integrity check
/// that needs no hashing tool on the host.
pub fn file_sha1(path: &Path) -> std::io::Result<String> {
    use sha1::{Digest, Sha1};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Starts the fixture on a thread with its own runtime: every `#[tokio::test]` has a runtime of
/// its own, which ends with that test, and nothing of the fixture may depend on it.
async fn start_in_thread(variant: Variant) -> State {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new().name(format!("dromon-testkit-{}", variant.name())).spawn(move || {
        let state = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime.block_on(start(variant)),
            Err(e) => State::Failed(format!("tokio runtime: {e}")),
        };
        let _ = tx.send(state);
    });
    if let Err(e) = spawned {
        return State::Failed(format!("spawn the fixture thread: {e}"));
    }
    rx.await.unwrap_or_else(|_| State::Failed("the fixture thread panicked (see its message above)".into()))
}

async fn start(variant: Variant) -> State {
    match std::env::var(BACKEND_ENV).as_deref() {
        Err(_) | Ok("" | "docker") => docker::start(variant).await,
        Ok("native") if variant == Variant::Trixie => native::start().await,
        Ok("native") => State::Unavailable(format!(
            "the {} server runs on the Docker backend only ({BACKEND_ENV}=native)",
            variant.name()
        )),
        Ok(other) => State::Failed(format!("{BACKEND_ENV}={other}: expected `docker` (the default) or `native`")),
    }
}

/// Whether `DROMON_FIXTURE_BACKEND` picks the native backend.
pub(crate) fn native_backend() -> bool {
    std::env::var(BACKEND_ENV).is_ok_and(|v| v == "native")
}

/// The SSH identification line the server at `HOST:port` sends first (`SSH-2.0-…`), retried
/// briefly while it comes up.
async fn read_banner(port: u16) -> Result<String, String> {
    use tokio::io::AsyncBufReadExt as _;
    let mut last = String::new();
    for _ in 0..20 {
        let attempt = async {
            let stream = tokio::net::TcpStream::connect((HOST, port)).await.map_err(|e| e.to_string())?;
            let mut lines = tokio::io::BufReader::new(stream).lines();
            // Servers may send other lines before the identification (RFC 4253 §4.2).
            while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
                if line.starts_with("SSH-") {
                    return Ok(line.trim_end().to_owned());
                }
            }
            Err("closed before the identification line".to_owned())
        };
        match tokio::time::timeout(Duration::from_secs(10), attempt).await {
            Ok(Ok(banner)) => return Ok(banner),
            Ok(Err(e)) => last = e,
            Err(_) => last = "no identification line within 10 s".into(),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(format!("SSH banner on {HOST}:{port}: {last}"))
}

/// The client keypairs in a fresh key folder: [`KEY`] and [`ENCRYPTED_KEY`].
fn client_keys(keep: bool) -> Result<PathBuf, String> {
    let keys = key_dir(keep)?;
    keygen(&keys.join(KEY), "", KEY_COMMENT)?;
    keygen(&keys.join(ENCRYPTED_KEY), PASSPHRASE, ENCRYPTED_KEY_COMMENT)?;
    Ok(keys)
}

/// A public-key login as [`Role::Key`], retried briefly: proves the keys, port and sshd all work.
async fn probe_login(fixture: &Fixture) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..20 {
        let out = tokio::process::Command::new(openssh("ssh"))
            .args(fixture.ssh_options())
            .arg(fixture.destination(Role::Key))
            .arg("true")
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| format!("spawn ssh: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        last = format!("{}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(format!("no public-key login as {} on {HOST}:{}: {last}", fixture.user(Role::Key), fixture.port))
}

/// A fresh key folder for this process, after deleting those of processes that are gone.
fn key_dir(keep: bool) -> Result<PathBuf, String> {
    sweep_stale_key_dirs();
    let dir = tempfile::Builder::new()
        .prefix(&format!("{KEY_DIR_PREFIX}{}-", std::process::id()))
        .tempdir()
        .map_err(|e| format!("key folder: {e}"))?
        .keep();
    if keep {
        std::fs::write(dir.join(KEEP_MARKER), "").map_err(|e| format!("key folder: {e}"))?;
    }
    Ok(dir)
}

/// Statics are never dropped, so a key folder outlives its process; the next start deletes it.
fn sweep_stale_key_dirs() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix(KEY_DIR_PREFIX))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        if pid == std::process::id() || path.join(KEEP_MARKER).exists() || process_alive(pid) {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

#[cfg(not(windows))]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `tasklist` lists the process as a CSV row with its pid quoted, or prints an `INFO:` line
/// without quotes when there is none. If tasklist can't run, the folder is kept.
#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_or(true, |out| String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\"")))
}

/// Win32-OpenSSH's ssh-keygen writes the private key with an ACL for the current user alone,
/// which its ssh requires.
fn keygen(path: &Path, passphrase: &str, comment: &str) -> Result<(), String> {
    let out = std::process::Command::new(openssh("ssh-keygen"))
        .args(["-q", "-t", "ed25519", "-N", passphrase, "-C", comment, "-f"])
        .arg(path)
        .output()
        .map_err(|e| format!("spawn ssh-keygen: {e}"))?;
    if !out.status.success() {
        return Err(format!("ssh-keygen {}: {}", path.display(), String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

/// Writes straight to stderr, which libtest doesn't capture, so the line shows in a plain
/// `cargo test` too.
fn note(args: fmt::Arguments<'_>) {
    let _ = writeln!(std::io::stderr(), "{args}");
}
