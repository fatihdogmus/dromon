//! The Docker sshd fixture for Dromon's integration tests, run with testcontainers.
//!
//! [`fixture`] starts one container per test process (test binary), shared by all its tests:
//!
//! 1. Checks that Docker answers a ping. If it doesn't, every [`fixture`] call prints a skip line
//!    and returns `None`, or panics when `DROMON_REQUIRE_DOCKER=1` (CI).
//! 2. Generates this process's two ed25519 keypairs with `ssh-keygen` into a fresh temp folder
//!    (`dromon-testkit-<pid>-…`; folders of processes that are gone are deleted on the next
//!    start).
//! 3. Builds the image from `tests/docker/` as `dromon-testkit-sshd:<hash of the build context>`,
//!    skipping the build when that tag exists, so an unchanged fixture builds once per machine.
//!    The image contains no keys: the public keys are copied into the created container before
//!    it starts.
//! 4. Starts the container with ports 22 (sshd) and 9999 (lifeline) published on 127.0.0.1 at
//!    random host ports, waits for sshd's `Server listening on` line, attaches the lifeline and
//!    checks that a public-key login works.
//!
//! Cleanup: a static is never dropped, so testcontainers' remove-on-drop can't run, and its
//! `watchdog` feature only reacts to SIGINT/SIGTERM/SIGQUIT. Instead the test process holds a TCP
//! connection to the container's port 9999 for its whole life. When the process exits (normally,
//! on a panic or when killed) the kernel closes it; the container's entrypoint then stops sshd
//! and exits, and Docker deletes the container (`AutoRemove`). A container whose lifeline never
//! connects ends after 120 s.
//!
//! Debugging: `DROMON_FIXTURE_KEEP=1` skips the lifeline, so the container (and the key folder)
//! outlive the tests; the start line prints the port, key folder and the `docker rm -f` command.

use std::fmt;
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use testcontainers::bollard::Docker;
use testcontainers::bollard::models::{HostConfig, PortBinding};
use testcontainers::core::client::docker_client_instance;
use testcontainers::core::{BuildImageOptions, IntoContainerPort, WaitFor};
use testcontainers::runners::{AsyncBuilder, AsyncRunner};
use testcontainers::{GenericBuildableImage, ImageExt};
use tokio::sync::OnceCell;

mod totp;

pub use totp::{STEP as TOTP_STEP, totp_at, totp_now, totp_secret, wrong_totp};

/// Public key only, no password; owns the seed data (`/home/key/fixture`).
pub const USER_KEY: &str = "key";
/// The SSH `password` method only: prompt `pw@<host>'s password: `.
pub const USER_PASSWORD: &str = "pw";
/// Keyboard-interactive through PAM: `(kbd@<host>) Password: ` ([`PASSWORD`]), then
/// `(kbd@<host>) Verification code: ` ([`totp_now`]).
pub const USER_KBD: &str = "kbd";
/// Two methods: public key ([`Fixture::encrypted_key`]), then password ([`PASSWORD`]).
pub const USER_KEY_THEN_PASSWORD: &str = "keypw";
/// Public key with the encrypted private key ([`Fixture::encrypted_key`], [`PASSPHRASE`]).
pub const USER_ENCRYPTED_KEY: &str = "enc";
/// The password of `pw`, `kbd` and `keypw`.
pub const PASSWORD: &str = "dromon-test-pw";
/// The passphrase of [`Fixture::encrypted_key`].
pub const PASSPHRASE: &str = "dromon-test-passphrase";
/// The comment of [`Fixture::key`].
pub const KEY_COMMENT: &str = "dromon-fixture";
/// The comment of [`Fixture::encrypted_key`].
pub const ENCRYPTED_KEY_COMMENT: &str = "dromon-fixture-enc";
/// The address the ports are published on.
pub const HOST: &str = "127.0.0.1";
/// The 1 GiB random file made by [`Fixture::big_file`].
pub const BIG_FILE: &str = "/home/key/fixture/big/big.bin";
pub const BIG_FILE_SIZE: u64 = 1 << 30;

/// Set to `1` to fail, instead of skip, the fixture tests when Docker isn't reachable.
pub const REQUIRE_DOCKER_ENV: &str = "DROMON_REQUIRE_DOCKER";
/// Set to `1` to keep the container running after the tests.
pub const KEEP_ENV: &str = "DROMON_FIXTURE_KEEP";
/// The image name; the tag is a hash of the build context.
pub const IMAGE_NAME: &str = "dromon-testkit-sshd";
/// Label on every fixture container; its value is the test process id.
pub const LABEL: &str = "dev.dromon.testkit.pid";

const CONTEXT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/docker");
/// Everything the Dockerfile uses (README.md is not part of the image).
const CONTEXT_FILES: [&str; 6] = ["Dockerfile", "sshd_config", "pam-sshd", "google_authenticator", "entrypoint.sh", "seed.sh"];
const SSH_PORT: u16 = 22;
const LIFELINE_PORT: u16 = 9999;
const KEY_DIR_PREFIX: &str = "dromon-testkit-";
/// In a key folder: kept by `DROMON_FIXTURE_KEEP=1`, never swept.
const KEEP_MARKER: &str = "kept";
const KEY: &str = "id_ed25519";
const ENCRYPTED_KEY: &str = "id_ed25519_enc";

/// The running sshd fixture of this test process.
pub struct Fixture {
    port: u16,
    keys: PathBuf,
    container_id: String,
    image: String,
    /// Closed by the kernel when the process exits, which ends the container.
    _lifeline: Option<TcpStream>,
    big_file: OnceCell<Result<(), String>>,
}

impl fmt::Debug for Fixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fixture")
            .field("port", &self.port)
            .field("keys", &self.keys)
            .field("container_id", &self.container_id)
            .field("image", &self.image)
            .finish_non_exhaustive()
    }
}

enum State {
    Ready(Fixture),
    Unreachable(String),
    Failed(String),
}

/// This process's fixture, started on first use. `None` (after printing a skip line naming
/// `test`) when Docker isn't reachable; panics instead with `DROMON_REQUIRE_DOCKER=1`, and
/// whenever Docker is reachable but the fixture doesn't start.
pub async fn fixture(test: &str) -> Option<&'static Fixture> {
    static STATE: OnceCell<State> = OnceCell::const_new();
    match STATE.get_or_init(start_in_thread).await {
        State::Ready(fixture) => Some(fixture),
        State::Unreachable(why) if !env_flag(REQUIRE_DOCKER_ENV) => {
            note(format_args!(
                "skipping {test}: Docker is not reachable ({why}); set {REQUIRE_DOCKER_ENV}=1 to fail instead"
            ));
            None
        }
        State::Unreachable(why) => panic!("{test}: {REQUIRE_DOCKER_ENV}=1 but Docker is not reachable: {why}"),
        State::Failed(why) => panic!("{test}: the sshd fixture didn't start: {why}"),
    }
}

impl Fixture {
    /// The address sshd is published on, `127.0.0.1`.
    pub fn host(&self) -> &'static str {
        HOST
    }

    /// The host port that reaches the container's port 22.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The unencrypted private key, authorized for [`USER_KEY`].
    pub fn key(&self) -> PathBuf {
        self.keys.join(KEY)
    }

    /// The private key encrypted with [`PASSPHRASE`], authorized for [`USER_ENCRYPTED_KEY`] and
    /// [`USER_KEY_THEN_PASSWORD`].
    pub fn encrypted_key(&self) -> PathBuf {
        self.keys.join(ENCRYPTED_KEY)
    }

    pub fn container_id(&self) -> &str {
        &self.container_id
    }

    /// `name:tag` of the image the container runs.
    pub fn image(&self) -> &str {
        &self.image
    }

    /// `user@127.0.0.1`.
    pub fn destination(&self, user: &str) -> String {
        format!("{user}@{HOST}")
    }

    /// Non-interactive ssh options for a public-key login with [`Fixture::key`], isolated from
    /// the developer's ssh config, known hosts and agent keys: add `destination` and a command
    /// or `-s … sftp`.
    pub fn ssh_options(&self) -> Vec<String> {
        let mut args: Vec<String> = ["-T", "-x", "-e", "none", "-F", "/dev/null"].map(String::from).into();
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=no",
            "UserKnownHostsFile=/dev/null",
            "GlobalKnownHostsFile=/dev/null",
            "LogLevel=ERROR",
            "IdentitiesOnly=yes",
            "ConnectTimeout=10",
        ] {
            args.extend(["-o".into(), option.into()]);
        }
        args.extend(["-i".into(), self.key().to_string_lossy().into_owned(), "-p".into(), self.port.to_string()]);
        args
    }

    /// Runs `command` inside the container as root (`docker exec`); its stdout, or why it failed.
    pub fn exec(&self, command: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("docker")
            .arg("exec")
            .arg(&self.container_id)
            .args(command)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("spawn docker exec: {e}"))?;
        if !out.status.success() {
            return Err(format!("docker exec {command:?}: {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Ends every ssh session of `user` from the server's side, as a server restart does:
    /// `pkill -u <user>` (SIGTERM to its `sshd-session` and `sftp-server`). Returns how many
    /// processes were signalled. Other users' sessions are untouched; tests sharing `user`
    /// must not run at the same time.
    pub fn kill_sessions(&self, user: &str) -> Result<usize, String> {
        // pgrep and pkill exit 1 when nothing matches.
        let pids = self.exec(&["sh", "-c", "pgrep -u \"$1\"; pkill -u \"$1\"; exit 0", "sh", user])?;
        Ok(pids.lines().filter(|l| !l.trim().is_empty()).count())
    }

    /// Creates [`BIG_FILE`] (1 GiB of random data, owned by `key`) on first call; later calls
    /// return at once. Only the slow `#[ignore]` tests need it, so it is not part of the seed.
    pub async fn big_file(&self) -> &'static str {
        let made = self
            .big_file
            .get_or_init(|| async {
                let start = Instant::now();
                let script = format!(
                    "set -e; f={BIG_FILE}; [ \"$(stat -c %s $f 2>/dev/null || echo 0)\" = {BIG_FILE_SIZE} ] && exit 0; \
                     mkdir -p \"${{f%/*}}\"; head -c {BIG_FILE_SIZE} /dev/urandom >\"$f.tmp\"; mv \"$f.tmp\" \"$f\""
                );
                let out = tokio::process::Command::new("ssh")
                    .args(self.ssh_options())
                    .arg(self.destination(USER_KEY))
                    .arg(script)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .map_err(|e| format!("spawn ssh: {e}"))?;
                if !out.status.success() {
                    return Err(format!("{}: {}", out.status, String::from_utf8_lossy(&out.stderr)));
                }
                note(format_args!("dromon-testkit: {BIG_FILE} ready after {:.1} s", start.elapsed().as_secs_f64()));
                Ok(())
            })
            .await;
        if let Err(e) = made {
            panic!("creating {BIG_FILE} failed: {e}");
        }
        BIG_FILE
    }
}

/// Starts the fixture on a thread with its own runtime: every `#[tokio::test]` has a runtime of
/// its own, which ends with that test, and nothing of the fixture may depend on it.
async fn start_in_thread() -> State {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new().name("dromon-testkit".into()).spawn(move || {
        let state = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime.block_on(start()),
            Err(e) => State::Failed(format!("tokio runtime: {e}")),
        };
        let _ = tx.send(state);
    });
    if let Err(e) = spawned {
        return State::Failed(format!("spawn the fixture thread: {e}"));
    }
    rx.await.unwrap_or_else(|_| State::Failed("the fixture thread panicked (see its message above)".into()))
}

async fn start() -> State {
    let docker = match reachable_docker().await {
        Ok(docker) => docker,
        Err(why) => return State::Unreachable(why),
    };
    match start_container(&docker).await {
        Ok(fixture) => State::Ready(fixture),
        Err(why) => State::Failed(why),
    }
}

async fn reachable_docker() -> Result<Docker, String> {
    let docker = docker_client_instance().await.map_err(|e| e.to_string())?;
    match tokio::time::timeout(Duration::from_secs(10), docker.ping()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err("no answer to a ping within 10 s".into()),
    }
    // A daemon running Windows containers (GitHub's Windows runners) can't run the Linux image:
    // for the fixture that is no Docker at all.
    let info = docker.info().await.map_err(|e| e.to_string())?;
    match info.os_type.as_deref() {
        Some("linux") | None => Ok(docker),
        Some(other) => Err(format!("the daemon runs {other} containers, the fixture needs Linux ones")),
    }
}

async fn start_container(docker: &Docker) -> Result<Fixture, String> {
    let started = Instant::now();
    let keep = env_flag(KEEP_ENV);
    let keys = key_dir(keep)?;
    keygen(&keys.join(KEY), "", KEY_COMMENT)?;
    keygen(&keys.join(ENCRYPTED_KEY), PASSPHRASE, ENCRYPTED_KEY_COMMENT)?;

    let tag = context_hash()?;
    let image_name = format!("{IMAGE_NAME}:{tag}");
    let cached = docker.inspect_image(&image_name).await.is_ok();
    let mut buildable = GenericBuildableImage::new(IMAGE_NAME, &tag).with_dockerfile(Path::new(CONTEXT).join("Dockerfile"));
    for file in CONTEXT_FILES.iter().filter(|f| **f != "Dockerfile") {
        buildable = buildable.with_file(Path::new(CONTEXT).join(file), format!("./{file}"));
    }
    let image = buildable
        .build_image_with(BuildImageOptions::new().with_skip_if_exists(true))
        .await
        .map_err(|e| format!("build {image_name}: {e}"))?;
    let image_ready = started.elapsed();

    let mut request = image
        .with_exposed_port(SSH_PORT.tcp())
        .with_exposed_port(LIFELINE_PORT.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server listening on"))
        .with_copy_to("/keys/id_ed25519.pub", keys.join(format!("{KEY}.pub")))
        .with_copy_to("/keys/id_ed25519_enc.pub", keys.join(format!("{ENCRYPTED_KEY}.pub")))
        .with_label(LABEL, std::process::id().to_string())
        .with_startup_timeout(Duration::from_secs(120))
        .with_host_config_modifier(|host_config: &mut HostConfig| {
            // Docker deletes the container once it stops (the lifeline ends it), and the ports
            // are reachable from this machine only.
            host_config.auto_remove = Some(true);
            host_config.publish_all_ports = Some(false);
            let binding = || Some(vec![PortBinding { host_ip: Some(HOST.into()), host_port: Some(String::new()) }]);
            host_config.port_bindings = Some(
                [(format!("{SSH_PORT}/tcp"), binding()), (format!("{LIFELINE_PORT}/tcp"), binding())].into_iter().collect(),
            );
        });
    if keep {
        request = request.with_env_var(KEEP_ENV, "1");
    }
    // Until it is forgotten below, dropping `container` (an error on the way) removes it.
    let container = request.start().await.map_err(|e| format!("start {image_name}: {e}"))?;
    let container_id = container.id().to_owned();
    let port = container.get_host_port_ipv4(SSH_PORT).await.map_err(|e| format!("port {SSH_PORT}: {e}"))?;
    let lifeline = if keep {
        None
    } else {
        let lifeline_port =
            container.get_host_port_ipv4(LIFELINE_PORT).await.map_err(|e| format!("port {LIFELINE_PORT}: {e}"))?;
        Some(attach_lifeline(lifeline_port)?)
    };

    let fixture = Fixture {
        port,
        keys,
        container_id,
        image: image_name,
        _lifeline: lifeline,
        big_file: OnceCell::new(),
    };
    probe_login(&fixture).await?;
    // From here on the lifeline (or, kept, the developer) ends the container; never run
    // testcontainers' remove-on-drop, whose client belongs to this short-lived runtime.
    std::mem::forget(container);

    let id = &fixture.container_id[..12.min(fixture.container_id.len())];
    note(format_args!(
        "dromon-testkit: sshd fixture on {HOST}:{port}, container {id}, image {} ({} in {:.1} s), ready after {:.1} s",
        fixture.image,
        if cached { "cached" } else { "built" },
        image_ready.as_secs_f64(),
        started.elapsed().as_secs_f64(),
    ));
    if keep {
        note(format_args!(
            "dromon-testkit: {KEEP_ENV}=1: the container keeps running; log in with \
             `ssh -F /dev/null -i {} -p {port} key@{HOST}`; clean up with `docker rm -f {id}; rm -r {}`",
            fixture.key().display(),
            fixture.keys.display(),
        ));
    }
    Ok(fixture)
}

/// Connects to the container's lifeline port and waits for its `ready`, retrying while the
/// listener comes up.
fn attach_lifeline(port: u16) -> Result<TcpStream, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut last = String::new();
    for _ in 0..50 {
        match try_lifeline(&addr) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e,
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("lifeline on {addr}: {last}"))
}

fn try_lifeline(addr: &SocketAddr) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect_timeout(addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
    let mut ready = [0u8; 6];
    stream.read_exact(&mut ready).map_err(|e| e.to_string())?;
    if &ready != b"ready\n" {
        return Err(format!("unexpected greeting {ready:?}"));
    }
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    Ok(stream)
}

/// A public-key login as `key`, retried briefly: proves the keys, ports and sshd all work.
async fn probe_login(fixture: &Fixture) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..20 {
        let out = tokio::process::Command::new("ssh")
            .args(fixture.ssh_options())
            .arg(fixture.destination(USER_KEY))
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
    Err(format!("no public-key login as {USER_KEY} on {HOST}:{}: {last}", fixture.port))
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

fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn keygen(path: &Path, passphrase: &str, comment: &str) -> Result<(), String> {
    let out = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", passphrase, "-C", comment, "-f"])
        .arg(path)
        .output()
        .map_err(|e| format!("spawn ssh-keygen: {e}"))?;
    if !out.status.success() {
        return Err(format!("ssh-keygen {}: {}", path.display(), String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

/// The image tag: the first 16 hex digits of SHA-1 over the build context's names and contents.
fn context_hash() -> Result<String, String> {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    for file in CONTEXT_FILES {
        let path = Path::new(CONTEXT).join(file);
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        hasher.update(file.as_bytes());
        hasher.update([0]);
        hasher.update((data.len() as u64).to_be_bytes());
        hasher.update(&data);
    }
    Ok(hasher.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

/// Writes straight to stderr, which libtest doesn't capture, so the line shows in a plain
/// `cargo test` too.
fn note(args: fmt::Arguments<'_>) {
    let _ = writeln!(std::io::stderr(), "{args}");
}
