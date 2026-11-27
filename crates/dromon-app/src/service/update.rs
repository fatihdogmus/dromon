//! The service's updater (`state/update.rs` has the plan): one task that checks GitHub's latest
//! release, and downloads, checks and installs a newer one, all off the UI thread (HTTP and
//! files on tokio's blocking threads).
//!
//! - **Checks**: one unauthenticated `GET {api}/releases/latest`, with a plain `Dromon` user
//!   agent and nothing else about the user or the machine; drafts and pre-releases are ignored
//!   and only a newer version is offered. At launch and then once a day while automatic checks
//!   are on (the UI says, [`UpdateCommand::Auto`]); a failed one tries again within the hour.
//! - **Installs**: `SHA256SUMS` and its minisign signature first, checked against the release
//!   key built into the app (`update_verify.rs`), then the platform's file into the app's cache,
//!   whose SHA-256 must be the one the signed sums list; only then `update_platform::install`.
//!   File names and URLs are built from the version, never taken from GitHub's answer.
//! - **HTTPS only**: the agent refuses plain `http` (and redirects to it). Tests give it a local
//!   endpoint ([`Endpoint`]) and allow `http` there; nothing in tests reaches GitHub.
//!
//! The service starts the updater on the first update command (`service/mod.rs`); only the app
//! sends one (its restored settings, Settings, the app menu), never a headless run.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use minisign_verify::PublicKey;
use serde::Deserialize;
use tokio::sync::mpsc;

use super::{Sink, update_platform, update_verify};
use crate::state::update::{self as plan, Install, Method, Offer, Package, UpdateCommand, UpdateEvent, Version};
use crate::state::{ServiceEvent, WindowId};

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;

/// Dromon's repository: GitHub's API for it, and its pages (releases and their files).
const API: &str = "https://api.github.com/repos/fatihdogmus/dromon";
const SITE: &str = "https://github.com/fatihdogmus/dromon";

/// The most each answer may be: the release's JSON, the checksums, their signature, a file.
const MAX_RELEASE: u64 = 1 << 20;
const MAX_SUMS: u64 = 64 << 10;
const MAX_SIGNATURE: u64 = 4 << 10;
const MAX_FILE: u64 = 1 << 30;
/// How often the updater asks itself whether a check is due.
const TICK: Duration = Duration::from_secs(60 * 60);

/// Where releases are asked for and downloaded from.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// The repository's API (`…/repos/<owner>/<repo>`).
    pub api: String,
    /// The repository's pages (`https://github.com/<owner>/<repo>`).
    pub site: String,
    /// Refuse anything but HTTPS: always, but for tests' local endpoint.
    pub https_only: bool,
}

impl Endpoint {
    pub fn github() -> Endpoint {
        Endpoint { api: API.to_owned(), site: SITE.to_owned(), https_only: true }
    }

    fn latest(&self) -> String {
        format!("{}/releases/latest", self.api)
    }

    fn file(&self, version: &Version, name: &str) -> String {
        format!("{}/releases/download/{version}/{name}", self.site)
    }

    fn page(&self, version: &Version) -> String {
        format!("{}/releases/tag/{version}", self.site)
    }
}

/// What the updater works with.
#[derive(Clone, Debug)]
pub struct Setup {
    pub endpoint: Endpoint,
    /// How this copy was installed; `None`: found out at the first check
    /// (`update_platform::detect`).
    pub install: Option<Install>,
    /// The key `SHA256SUMS` must be signed with, or why there's none.
    pub key: Result<PublicKey, String>,
    /// Where downloads go.
    pub cache: PathBuf,
}

impl Setup {
    /// The app's: GitHub, the built-in release key, the app's cache folder.
    pub fn for_app() -> Setup {
        let cache = rune::dirs::AppDirs::new(crate::persist::APP_ID)
            .map_or_else(|| std::env::temp_dir().join("dromon"), |d| d.cache().to_path_buf());
        Setup {
            endpoint: Endpoint::github(),
            install: None,
            key: update_verify::release_key(),
            cache: cache.join("updates"),
        }
    }
}

/// The updater's end of the commands.
pub type Updater = mpsc::UnboundedSender<(WindowId, UpdateCommand)>;

/// Starts the updater task on the current runtime.
pub fn start<S: Sink>(sink: S, setup: Setup) -> Updater {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(rx, sink, setup));
    tx
}

/// A blocking job's end.
enum Done {
    Checked(Install, Result<Option<Offer>, String>),
    Installed(WindowId, Result<bool, String>),
}

struct Worker<S> {
    sink: S,
    setup: Arc<Setup>,
    done: mpsc::UnboundedSender<Done>,
    auto: bool,
    /// When the last check that got an answer ran.
    last: Option<Instant>,
    /// What the last check found.
    offer: Option<Offer>,
    install: Option<Install>,
    /// A check or an install runs (or the update is in place: nothing more runs).
    busy: bool,
    /// Install asked while a check ran: from this window, once it's done.
    queued: Option<WindowId>,
}

async fn run<S: Sink>(mut commands: mpsc::UnboundedReceiver<(WindowId, UpdateCommand)>, sink: S, setup: Setup) {
    let (done, mut dones) = mpsc::unbounded_channel();
    let install = setup.install.clone();
    let mut w = Worker {
        sink,
        setup: Arc::new(setup),
        done,
        auto: false,
        last: None,
        offer: None,
        install,
        busy: false,
        queued: None,
    };
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                Some((from, cmd)) => w.command(from, cmd),
                None => break,
            },
            _ = tick.tick() => w.check_if_due(),
            Some(done) = dones.recv() => w.finished(done),
        }
    }
}

impl<S: Sink> Worker<S> {
    fn emit(&self, ev: UpdateEvent) {
        self.sink.emit(ServiceEvent::Update(ev));
    }

    fn command(&mut self, from: WindowId, cmd: UpdateCommand) {
        match cmd {
            UpdateCommand::Auto(on) => {
                self.auto = on;
                self.check_if_due();
            }
            UpdateCommand::Check => self.check(),
            UpdateCommand::Install => self.install(from),
        }
    }

    fn check_if_due(&mut self) {
        if plan::due(self.auto, self.last.map(|t| t.elapsed())) {
            self.check();
        }
    }

    fn check(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.emit(UpdateEvent::Checking);
        let (setup, install, done) = (self.setup.clone(), self.install.clone(), self.done.clone());
        tokio::task::spawn_blocking(move || {
            let install = install.unwrap_or_else(update_platform::detect);
            let found = check(&setup, &install);
            let _ = done.send(Done::Checked(install, found));
        });
    }

    fn install(&mut self, from: WindowId) {
        if self.busy {
            self.queued = Some(from);
            return;
        }
        let Some(offer) = self.offer.clone() else {
            self.emit(UpdateEvent::InstallFailed("no newer version was found; check for updates again".to_owned()));
            return;
        };
        let install = self.install.clone().unwrap_or(Install::Other);
        self.busy = true;
        self.emit(UpdateEvent::Installing);
        let (setup, done) = (self.setup.clone(), self.done.clone());
        tokio::task::spawn_blocking(move || {
            let result = install_offer(&setup, &offer, &install);
            let _ = done.send(Done::Installed(from, result));
        });
    }

    fn finished(&mut self, done: Done) {
        self.busy = false;
        match done {
            Done::Checked(install, found) => {
                self.install = Some(install);
                match found {
                    Ok(offer) => {
                        self.last = Some(Instant::now());
                        self.offer = offer.clone();
                        self.emit(offer.map_or(UpdateEvent::UpToDate, UpdateEvent::Available));
                    }
                    Err(why) => self.emit(UpdateEvent::CheckFailed(why)),
                }
                if let Some(from) = self.queued.take() {
                    self.install(from);
                }
            }
            Done::Installed(from, Ok(relaunch)) => {
                // In place: the app quits now; nothing else runs here.
                self.busy = true;
                self.sink.emit_to(from, ServiceEvent::Update(UpdateEvent::Installed { relaunch }));
            }
            Done::Installed(_, Err(why)) => self.emit(UpdateEvent::InstallFailed(why)),
        }
    }
}

/// The HTTP agent: HTTPS only (but for tests), Dromon's own user agent and nothing about the
/// user, timeouts for a stalled network.
fn agent(endpoint: &Endpoint) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(endpoint.https_only)
        .user_agent("Dromon")
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

/// Why a request failed, as the user reads it.
fn network(e: &ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(404) => "GitHub has no such release".to_owned(),
        ureq::Error::StatusCode(code) => format!("GitHub answered {code}"),
        ureq::Error::RequireHttpsOnly(_) => "the address isn't HTTPS".to_owned(),
        e => format!("couldn't reach GitHub ({e})"),
    }
}

/// GET `url` whole, at most `limit` bytes.
fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut response =
        agent.get(url).header("Accept", "application/vnd.github+json").call().map_err(|e| network(&e))?;
    response.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| network(&e))
}

/// GitHub's latest release, as far as updates care.
#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// The version of the latest release in GitHub's answer; `None` for a draft or a pre-release.
fn parse_latest(body: &[u8]) -> Result<Option<Version>, String> {
    let latest: Latest = serde_json::from_slice(body).map_err(|_| "GitHub's answer wasn't a release".to_owned())?;
    if latest.draft || latest.prerelease {
        return Ok(None);
    }
    Version::parse(&latest.tag_name)
        .map(Some)
        .ok_or_else(|| "the latest release isn't named by a version Dromon knows".to_owned())
}

/// What a check finds: a newer release and how this copy takes it, or `None`.
fn check(setup: &Setup, install: &Install) -> Result<Option<Offer>, String> {
    let endpoint = &setup.endpoint;
    let body = get(&agent(endpoint), &endpoint.latest(), MAX_RELEASE)?;
    let Some(version) = parse_latest(&body)?.filter(|v| plan::is_newer(v, &Version::running())) else {
        return Ok(None);
    };
    let method = plan::method(install, setup.key.is_ok(), &version);
    Ok(Some(Offer { page: endpoint.page(&version), method, version }))
}

/// Downloads, checks and installs `offer`: `Ok(relaunch)` once it's in place.
fn install_offer(setup: &Setup, offer: &Offer, install: &Install) -> Result<bool, String> {
    let Method::Install(package) = offer.method else {
        return Err("this copy of Dromon doesn't install updates itself".to_owned());
    };
    let key = setup.key.as_ref().map_err(Clone::clone)?;
    let agent = agent(&setup.endpoint);
    let file = fetch_verified(&agent, &setup.endpoint, &offer.version, package, key, &setup.cache)?;
    update_platform::install(package, &file, install, &setup.cache)
}

/// `package`'s file of `version` in `dir`, checked: the signed sums first, then the file
/// against them. A file that doesn't check is deleted.
fn fetch_verified(
    agent: &ureq::Agent,
    endpoint: &Endpoint,
    version: &Version,
    package: Package,
    key: &PublicKey,
    dir: &Path,
) -> Result<PathBuf, String> {
    let name = package.asset(version);
    let sums = get(agent, &endpoint.file(version, "SHA256SUMS"), MAX_SUMS)?;
    let signature = get(agent, &endpoint.file(version, "SHA256SUMS.minisig"), MAX_SIGNATURE)?;
    let signature = String::from_utf8(signature).map_err(|_| update_verify::Refused::Signature.to_string())?;
    update_verify::signed_sums(key, version, &sums, &signature).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    let part = dir.join(format!("{name}.part"));
    let digest = download(agent, &endpoint.file(version, &name), &part);
    let checked = digest.and_then(|d| update_verify::listed(&sums, &name, &d).map_err(|e| e.to_string()));
    if let Err(e) = checked {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let file = dir.join(&name);
    std::fs::rename(&part, &file).map_err(|e| format!("couldn't keep the download: {e}"))?;
    Ok(file)
}

/// Streams `url` into `path` (at most [`MAX_FILE`]); returns its SHA-256.
fn download(agent: &ureq::Agent, url: &str, path: &Path) -> Result<Vec<u8>, String> {
    let response = agent.get(url).call().map_err(|e| network(&e))?;
    let mut body = response.into_body().into_with_config().limit(MAX_FILE).reader();
    let mut out = std::fs::File::create(path).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    let mut sha = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0; 64 << 10];
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("the download stopped: {e}"))?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        std::io::Write::write_all(&mut out, &buf[..n])
            .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    }
    out.sync_all().map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    Ok(sha.finish().as_ref().to_vec())
}
