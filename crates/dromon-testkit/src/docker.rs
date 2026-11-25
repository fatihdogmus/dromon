//! The Docker backend (the default), run with testcontainers:
//!
//! 1. Checks that Docker answers a ping. If it doesn't, every [`fixture`](crate::fixture) call
//!    prints a skip line and returns `None`, or panics when `DROMON_REQUIRE_DOCKER=1` (CI).
//! 2. Generates this process's two ed25519 keypairs (`crate::client_keys`).
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

use std::io::Read as _;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use testcontainers::bollard::Docker;
use testcontainers::bollard::models::{HostConfig, PortBinding};
use testcontainers::core::client::docker_client_instance;
use testcontainers::core::{BuildImageOptions, IntoContainerPort, WaitFor};
use testcontainers::runners::{AsyncBuilder, AsyncRunner};
use testcontainers::{GenericBuildableImage, ImageExt};
use tokio::sync::OnceCell;

use crate::{
    Account, ENCRYPTED_KEY, Fixture, HOST, IMAGE_NAME, KEEP_ENV, KEY, LABEL, NULL_DEVICE, Role, Server, State,
    client_keys, env_flag, note, probe_login,
};

const CONTEXT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/docker");
/// Everything the Dockerfile uses (README.md is not part of the image).
const CONTEXT_FILES: [&str; 6] = ["Dockerfile", "sshd_config", "pam-sshd", "google_authenticator", "entrypoint.sh", "seed.sh"];
pub(crate) const SSH_PORT: u16 = 22;
const LIFELINE_PORT: u16 = 9999;
pub(crate) const HOST_PUBLIC_KEY: &str = "/etc/ssh/ssh_host_ed25519_key.pub";

/// The running container.
#[derive(Debug)]
pub(crate) struct Container {
    pub(crate) id: String,
    pub(crate) image: String,
    /// Closed by the kernel when the process exits, which ends the container.
    _lifeline: Option<TcpStream>,
}

impl Container {
    /// `docker exec` as root through the API, on a thread with a runtime of its own.
    pub(crate) fn exec(&self, command: &[&str]) -> Result<String, String> {
        let id = self.id.clone();
        let cmd: Vec<String> = command.iter().map(|s| (*s).to_owned()).collect();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| format!("runtime: {e}"))?;
            rt.block_on(exec_in(&id, cmd))
        })
        .join()
        .map_err(|_| "the exec thread panicked".to_owned())?
    }
}

pub(crate) async fn start() -> State {
    let docker = match reachable_docker().await {
        Ok(docker) => docker,
        Err(why) => return State::Unreachable(why),
    };
    match start_container(&docker).await {
        Ok(fixture) => State::Ready(fixture),
        Err(why) => State::Failed(why),
    }
}

/// The image's users: `/home/<name>`, each in a group of its own name.
fn accounts() -> Vec<Account> {
    Role::ALL
        .iter()
        .map(|role| {
            let name = role.base_name().to_owned();
            Account { home: format!("/home/{name}"), group: name.clone(), name }
        })
        .collect()
}

/// `docker exec` through the API: create the exec, collect its attached stdout and stderr, then
/// read its exit code.
async fn exec_in(container: &str, cmd: Vec<String>) -> Result<String, String> {
    use futures_util::StreamExt as _;
    use testcontainers::bollard::container::LogOutput;
    use testcontainers::bollard::exec::StartExecResults;
    use testcontainers::bollard::models::ExecConfig;

    let docker = docker_client_instance().await.map_err(|e| format!("docker client: {e}"))?;
    let config = ExecConfig { cmd: Some(cmd.clone()), attach_stdout: Some(true), attach_stderr: Some(true), ..Default::default() };
    let created = docker.create_exec(container, config).await.map_err(|e| format!("docker exec {cmd:?}: {e}"))?;
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    match docker.start_exec(&created.id, None).await.map_err(|e| format!("docker exec {cmd:?}: {e}"))? {
        StartExecResults::Attached { mut output, .. } => {
            while let Some(chunk) = output.next().await {
                match chunk.map_err(|e| format!("docker exec {cmd:?}: {e}"))? {
                    LogOutput::StdOut { message } | LogOutput::Console { message } => stdout.extend_from_slice(&message),
                    LogOutput::StdErr { message } => stderr.extend_from_slice(&message),
                    LogOutput::StdIn { .. } => {}
                }
            }
        }
        StartExecResults::Detached => return Err(format!("docker exec {cmd:?}: started detached")),
    }
    let code = docker.inspect_exec(&created.id).await.map_err(|e| format!("docker exec {cmd:?}: {e}"))?.exit_code;
    if code != Some(0) {
        return Err(format!("docker exec {cmd:?}: exit {code:?}: {}", String::from_utf8_lossy(&stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
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
    let keys = client_keys(keep)?;

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
        accounts: accounts(),
        server: Server::Docker(Container { id: container_id, image: image_name, _lifeline: lifeline }),
        big_file: OnceCell::new(),
    };
    probe_login(&fixture).await?;
    // From here on the lifeline (or, kept, the developer) ends the container; never run
    // testcontainers' remove-on-drop, whose client belongs to this short-lived runtime.
    std::mem::forget(container);

    let Server::Docker(c) = &fixture.server else { unreachable!() };
    let id = &c.id[..12.min(c.id.len())];
    note(format_args!(
        "dromon-testkit: sshd fixture on {HOST}:{port}, container {id}, image {} ({} in {:.1} s), ready after {:.1} s",
        c.image,
        if cached { "cached" } else { "built" },
        image_ready.as_secs_f64(),
        started.elapsed().as_secs_f64(),
    ));
    if keep {
        note(format_args!(
            "dromon-testkit: {KEEP_ENV}=1: the container keeps running; log in with \
             `ssh -F {NULL_DEVICE} -i {} -p {port} key@{HOST}`; clean up with `docker rm -f {id}; rm -r {}`",
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
