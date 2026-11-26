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
//!
//! Variants ([`Variant`]): each is an image of its own folder under `tests/docker/` (its
//! Dockerfile, plus the shared files it copies), tagged by the hash of its build context, and
//! started the same way. [`Variant::Toxic`] starts a container of the default image and a
//! toxiproxy container (`tests/docker/toxiproxy/`, its own lifeline) whose proxy forwards to the
//! sshd container's address on the Docker network; the fixture's port is the proxy's.

use std::io::Read as _;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use testcontainers::bollard::Docker;
use testcontainers::bollard::models::{HostConfig, PortBinding};
use testcontainers::core::client::docker_client_instance;
use testcontainers::core::{BuildImageOptions, IntoContainerPort, WaitFor};
use testcontainers::runners::{AsyncBuilder, AsyncRunner};
use testcontainers::{ContainerAsync, ContainerRequest, GenericBuildableImage, GenericImage, ImageExt};

use crate::toxiproxy::{self, Toxiproxy};
use crate::{
    Account, ENCRYPTED_KEY, Fixture, HOST, IMAGE_NAME, KEEP_ENV, KEY, LABEL, NULL_DEVICE, Role, Server, State, Variant,
    client_keys, env_flag, note, probe_login, read_banner,
};

const CONTEXT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/docker");
pub(crate) const SSH_PORT: u16 = 22;
const LIFELINE_PORT: u16 = 9999;
pub(crate) const HOST_PUBLIC_KEY: &str = "/etc/ssh/ssh_host_ed25519_key.pub";

/// One image: its name, its Dockerfile and the other files of its build context, as paths
/// under `tests/docker/` and names in the context.
struct ImageSpec {
    name: &'static str,
    dockerfile: &'static str,
    files: &'static [(&'static str, &'static str)],
}

/// The files of `tests/docker/` the Debian images copy (README.md is not part of an image).
const DEBIAN_FILES: &[(&str, &str)] = &[
    ("sshd_config", "sshd_config"),
    ("pam-sshd", "pam-sshd"),
    ("google_authenticator", "google_authenticator"),
    ("entrypoint.sh", "entrypoint.sh"),
    ("seed.sh", "seed.sh"),
];

const TRIXIE: ImageSpec = ImageSpec { name: IMAGE_NAME, dockerfile: "Dockerfile", files: DEBIAN_FILES };
const BULLSEYE: ImageSpec =
    ImageSpec { name: "dromon-testkit-sshd-bullseye", dockerfile: "bullseye/Dockerfile", files: DEBIAN_FILES };
const ALPINE: ImageSpec = ImageSpec {
    name: "dromon-testkit-sshd-alpine",
    dockerfile: "alpine/Dockerfile",
    files: &[("alpine/sshd_config", "sshd_config"), ("entrypoint.sh", "entrypoint.sh"), ("seed.sh", "seed.sh")],
};
const TOXIPROXY: ImageSpec = ImageSpec {
    name: "dromon-testkit-toxiproxy",
    dockerfile: "toxiproxy/Dockerfile",
    files: &[("toxiproxy/toxiproxy.sh", "toxiproxy.sh")],
};

impl ImageSpec {
    fn of(variant: Variant) -> &'static ImageSpec {
        match variant {
            Variant::Trixie | Variant::Toxic => &TRIXIE,
            Variant::Bullseye => &BULLSEYE,
            Variant::Alpine => &ALPINE,
        }
    }
}

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

pub(crate) async fn start(variant: Variant) -> State {
    let docker = match reachable_docker().await {
        Ok(docker) => docker,
        Err(why) => return State::Unreachable(why),
    };
    match start_container(&docker, variant).await {
        Ok(fixture) => State::Ready(Box::new(fixture)),
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

/// Builds `spec`'s image unless its tag exists: the image, `name:tag`, and whether it was cached.
async fn build_image(docker: &Docker, spec: &ImageSpec) -> Result<(GenericImage, String, bool), String> {
    let tag = context_hash(spec)?;
    let image_name = format!("{}:{tag}", spec.name);
    let cached = docker.inspect_image(&image_name).await.is_ok();
    let mut buildable =
        GenericBuildableImage::new(spec.name, &tag).with_dockerfile(Path::new(CONTEXT).join(spec.dockerfile));
    for (src, dst) in spec.files {
        buildable = buildable.with_file(Path::new(CONTEXT).join(src), format!("./{dst}"));
    }
    let image = buildable
        .build_image_with(BuildImageOptions::new().with_skip_if_exists(true))
        .await
        .map_err(|e| format!("build {image_name}: {e}"))?;
    Ok((image, image_name, cached))
}

/// A container of `image`, ready once `ready` is on its stderr, with `ports` published on
/// 127.0.0.1 at random host ports, deleted by Docker once it stops (its lifeline ends it).
fn container_request(image: GenericImage, ports: &'static [u16], ready: &str, keep: bool) -> ContainerRequest<GenericImage> {
    let mut image = image.with_wait_for(WaitFor::message_on_stderr(ready));
    for port in ports {
        image = image.with_exposed_port(port.tcp());
    }
    let request = image
        .with_label(LABEL, std::process::id().to_string())
        .with_startup_timeout(Duration::from_secs(120))
        .with_host_config_modifier(move |host_config: &mut HostConfig| {
            // The ports are reachable from this machine only.
            host_config.auto_remove = Some(true);
            host_config.publish_all_ports = Some(false);
            let binding = || Some(vec![PortBinding { host_ip: Some(HOST.into()), host_port: Some(String::new()) }]);
            host_config.port_bindings = Some(ports.iter().map(|p| (format!("{p}/tcp"), binding())).collect());
        });
    if keep { request.with_env_var(KEEP_ENV, "1") } else { request }
}

/// Attaches the container's lifeline (none when kept).
async fn lifeline_of(container: &ContainerAsync<GenericImage>, keep: bool) -> Result<Option<TcpStream>, String> {
    if keep {
        return Ok(None);
    }
    let port = container.get_host_port_ipv4(LIFELINE_PORT).await.map_err(|e| format!("port {LIFELINE_PORT}: {e}"))?;
    attach_lifeline(port).map(Some)
}

async fn start_container(docker: &Docker, variant: Variant) -> Result<Fixture, String> {
    let started = Instant::now();
    let keep = env_flag(KEEP_ENV);
    let keys = client_keys(keep)?;
    let (image, image_name, cached) = build_image(docker, ImageSpec::of(variant)).await?;
    let image_ready = started.elapsed();

    let request = container_request(image, &[SSH_PORT, LIFELINE_PORT], "Server listening on", keep)
        .with_copy_to("/keys/id_ed25519.pub", keys.join(format!("{KEY}.pub")))
        .with_copy_to("/keys/id_ed25519_enc.pub", keys.join(format!("{ENCRYPTED_KEY}.pub")));
    // Until it is forgotten below, dropping `container` (an error on the way) removes it.
    let container = request.start().await.map_err(|e| format!("start {image_name}: {e}"))?;
    let container_id = container.id().to_owned();
    let mut port = container.get_host_port_ipv4(SSH_PORT).await.map_err(|e| format!("port {SSH_PORT}: {e}"))?;
    let lifeline = lifeline_of(&container, keep).await?;

    // The toxic variant: toxiproxy in front, on the sshd container's address.
    let mut proxy = None;
    if variant == Variant::Toxic {
        let ip = container.get_bridge_ip_address().await.map_err(|e| format!("the sshd container's address: {e}"))?;
        let (proxy_image, proxy_name, _) = build_image(docker, &TOXIPROXY).await?;
        let ports = &[toxiproxy::LISTEN_PORT, toxiproxy::API_PORT, LIFELINE_PORT];
        let started = container_request(proxy_image, ports, "dromon-toxiproxy: proxy sshd", keep)
            .with_env_var("UPSTREAM", format!("{ip}:{SSH_PORT}"))
            .start()
            .await
            .map_err(|e| format!("start {proxy_name}: {e}"))?;
        port = started
            .get_host_port_ipv4(toxiproxy::LISTEN_PORT)
            .await
            .map_err(|e| format!("port {}: {e}", toxiproxy::LISTEN_PORT))?;
        let api = started.get_host_port_ipv4(toxiproxy::API_PORT).await.map_err(|e| format!("port {}: {e}", toxiproxy::API_PORT))?;
        let api = SocketAddr::from((Ipv4Addr::LOCALHOST, api));
        let toxi = Toxiproxy::new(api, started.id().to_owned(), lifeline_of(&started, keep).await?);
        toxi.add_base()?;
        proxy = Some((started, toxi));
    }
    let (proxy_container, toxi) = proxy.map_or((None, None), |(c, t)| (Some(c), Some(t)));

    let banner = read_banner(port).await?;
    let server = Server::Docker(Container { id: container_id, image: image_name, _lifeline: lifeline });
    let fixture = Fixture::assemble(port, keys, accounts(), server, variant, banner, toxi);
    probe_login(&fixture).await?;
    // From here on the lifelines (or, kept, the developer) end the containers; never run
    // testcontainers' remove-on-drop, whose client belongs to this short-lived runtime.
    std::mem::forget(container);
    if let Some(c) = proxy_container {
        std::mem::forget(c);
    }

    let Server::Docker(c) = &fixture.server else { unreachable!() };
    let short = |id: &str| id[..12.min(id.len())].to_owned();
    let id = short(&c.id);
    let (via, proxy_id) = match &fixture.toxiproxy {
        Some(t) => (format!(" through toxiproxy {} (API {})", short(&t.container_id), t.api()), format!(" {}", short(&t.container_id))),
        None => (String::new(), String::new()),
    };
    note(format_args!(
        "dromon-testkit: {} sshd fixture on {HOST}:{port}{via}, {}, container {id}, image {} ({} in {:.1} s), \
         ready after {:.1} s",
        variant.name(),
        fixture.banner,
        c.image,
        if cached { "cached" } else { "built" },
        image_ready.as_secs_f64(),
        started.elapsed().as_secs_f64(),
    ));
    if keep {
        note(format_args!(
            "dromon-testkit: {KEEP_ENV}=1: the container keeps running; log in with \
             `ssh -F {NULL_DEVICE} -i {} -p {port} key@{HOST}`; clean up with `docker rm -f {id}{proxy_id}; rm -r {}`",
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

/// The image tag: the first 16 hex digits of SHA-1 over the build context's names and contents
/// (the Dockerfile first, as `Dockerfile`).
fn context_hash(spec: &ImageSpec) -> Result<String, String> {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    for (src, name) in std::iter::once(&(spec.dockerfile, "Dockerfile")).chain(spec.files) {
        let path = Path::new(CONTEXT).join(src);
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update((data.len() as u64).to_be_bytes());
        hasher.update(&data);
    }
    Ok(hasher.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect())
}
