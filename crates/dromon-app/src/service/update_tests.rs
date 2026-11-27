//! The updater against a local fake of GitHub (plain HTTP on 127.0.0.1, which only tests'
//! endpoints allow): what a check finds, what a download must prove, and the whole way from the
//! launch check to the toast and Settings' Check now. Nothing here reaches GitHub; signatures
//! are the test-only key's (`update_testdata/`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use super::*;
use crate::fixture;
use crate::service::update_verify::tests::{DMG, DMG_NAME, SIGNATURE, SUMS, TEST_KEY};
use crate::state::update::{Button, Manual, Phase, UpdateAction};
use crate::state::{Action, App, Backend, Command, NoStore, Saved, SettingsSection};

const WAIT: Duration = Duration::from_secs(20);

/// A fake of GitHub's API and release files: `routes` maps a path, and how many times it was
/// asked for before, to a body; anything else is a 404. Each request is kept.
struct Server {
    url: String,
    asked: Arc<Mutex<Vec<String>>>,
}

fn serve(routes: impl Fn(&str, usize) -> Option<Vec<u8>> + Send + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    std::thread::spawn(move || {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|n| n > 0) && header != "\r\n" {
                log.lock().unwrap().push(header.trim_end().to_owned());
                header.clear();
            }
            let n = counts.entry(path.clone()).or_default();
            let body = routes(&path, *n);
            *n += 1;
            log.lock().unwrap().push(format!("GET {path}"));
            let (status, body) = match body {
                Some(body) => ("200 OK", body),
                None => ("404 Not Found", b"{}".to_vec()),
            };
            let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(&body));
        }
    });
    Server { url, asked }
}

impl Server {
    fn endpoint(&self) -> Endpoint {
        Endpoint { api: format!("{}/api", self.url), site: format!("{}/site", self.url), https_only: false }
    }

    fn paths(&self) -> Vec<String> {
        self.asked.lock().unwrap().iter().filter_map(|l| l.strip_prefix("GET ").map(str::to_owned)).collect()
    }
}

fn release(tag: &str) -> Vec<u8> {
    format!(r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"html_url":"https://evil.invalid/","assets":[]}}"#)
        .into_bytes()
}

fn test_key() -> Result<PublicKey, String> {
    update_verify::parse_key(TEST_KEY)
}

const FILES: &str = "/site/releases/download/9.9.9/";

/// GitHub with 9.9.9 out, its files as given.
fn github(sums: &'static [u8], signature: &'static str, dmg: Vec<u8>) -> Server {
    serve(move |path, _| match path {
        "/api/releases/latest" => Some(release("9.9.9")),
        _ => match path.strip_prefix(FILES)? {
            "SHA256SUMS" => Some(sums.to_vec()),
            "SHA256SUMS.minisig" => Some(signature.as_bytes().to_vec()),
            DMG_NAME => Some(dmg.clone()),
            _ => None,
        },
    })
}

fn setup(endpoint: Endpoint, key: Result<PublicKey, String>, cache: &Path) -> Setup {
    Setup {
        endpoint,
        install: Some(Install::MacApp { team: Some("ABCDE12345".into()) }),
        key,
        cache: cache.to_path_buf(),
    }
}

fn nine() -> Version {
    Version::parse("9.9.9").unwrap()
}

#[test]
fn githubs_answer_names_the_latest_full_release() {
    assert_eq!(parse_latest(&release("0.3.0")), Ok(Version::parse("0.3.0")));
    let draft = br#"{"tag_name":"0.3.0","draft":true,"prerelease":false}"#;
    let pre = br#"{"tag_name":"0.3.0-rc.1","draft":false,"prerelease":true}"#;
    assert_eq!((parse_latest(draft), parse_latest(pre)), (Ok(None), Ok(None)));
    assert!(parse_latest(br#"{"tag_name":"v0.3.0"}"#).unwrap_err().contains("isn't named by a version"));
    assert_eq!(parse_latest(b"<html>"), Err("GitHub's answer wasn't a release".into()));
}

#[test]
fn a_check_offers_only_a_newer_version_with_its_own_page() {
    let server = serve(|path, n| (path == "/api/releases/latest").then(|| release(["9.9.9", "0.0.1"][n.min(1)])));
    let dir = tempfile::tempdir().unwrap();
    let setup = setup(server.endpoint(), test_key(), dir.path());
    let install = Install::MacApp { team: Some("ABCDE12345".into()) };
    let offer = check(&setup, &install).unwrap().expect("9.9.9 is newer");
    assert_eq!(offer.version, nine());
    assert_eq!(offer.page, format!("{}/site/releases/tag/9.9.9", server.url), "built, not GitHub's html_url");
    assert_eq!(offer.method, Method::Install(Package::Dmg));
    assert_eq!(check(&setup, &install), Ok(None), "never a downgrade");
    // One request each, with Dromon's user agent and nothing about the user.
    assert_eq!(server.paths(), ["/api/releases/latest", "/api/releases/latest"]);
    let headers = server.asked.lock().unwrap().clone();
    assert!(headers.iter().any(|h| h.eq_ignore_ascii_case("user-agent: Dromon")), "{headers:?}");
    assert!(
        !headers
            .iter()
            .any(|h| h.to_lowercase().starts_with("authorization") || h.to_lowercase().starts_with("cookie"))
    );
}

#[test]
fn without_a_release_key_a_check_still_finds_it_but_never_installs() {
    let server = github(SUMS, SIGNATURE, DMG.to_vec());
    let dir = tempfile::tempdir().unwrap();
    let placeholder = update_verify::parse_key("untrusted comment: PLACEHOLDER: no release key yet\n");
    let setup = setup(server.endpoint(), placeholder, dir.path());
    let offer = check(&setup, setup.install.as_ref().unwrap()).unwrap().unwrap();
    assert_eq!(offer.method, Method::Page(Manual::NoKey));
    // Even asked to install it anyway (an `Install` method), it refuses before downloading.
    let forced = Offer { method: Method::Install(Package::Dmg), ..offer };
    let err = install_offer(&setup, &forced, setup.install.as_ref().unwrap()).unwrap_err();
    assert_eq!(err, "This build of Dromon has no usable release key.");
    assert_eq!(server.paths(), ["/api/releases/latest"], "nothing downloaded");
}

#[test]
fn https_only_refuses_plain_http() {
    let server = serve(|_, _| Some(release("9.9.9")));
    let dir = tempfile::tempdir().unwrap();
    let endpoint = Endpoint { https_only: true, ..server.endpoint() };
    let setup = setup(endpoint, test_key(), dir.path());
    assert_eq!(check(&setup, &Install::Other), Err("the address isn't HTTPS".into()));
    assert!(server.paths().is_empty(), "nothing was sent");
    assert!(Endpoint::github().https_only && Endpoint::github().api.starts_with("https://"));
    assert!(Endpoint::github().site.starts_with("https://github.com/"));
}

/// Downloads 9.9.9's DMG from `server` with the test key into a fresh folder.
fn fetch(server: &Server) -> (tempfile::TempDir, Result<PathBuf, String>) {
    let dir = tempfile::tempdir().unwrap();
    let key = test_key().unwrap();
    let endpoint = server.endpoint();
    let got = fetch_verified(&agent(&endpoint), &endpoint, &nine(), Package::Dmg, &key, dir.path());
    (dir, got)
}

fn files(dir: &tempfile::TempDir) -> Vec<String> {
    std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
}

#[test]
fn a_signed_listed_download_is_kept() {
    let server = github(SUMS, SIGNATURE, DMG.to_vec());
    let (dir, got) = fetch(&server);
    let file = got.unwrap();
    assert_eq!(file, dir.path().join(DMG_NAME));
    assert_eq!(std::fs::read(&file).unwrap(), DMG);
    assert_eq!(files(&dir), [DMG_NAME]);
    let order: Vec<String> = server.paths().iter().map(|p| p.trim_start_matches(FILES).to_owned()).collect();
    assert_eq!(order, ["SHA256SUMS", "SHA256SUMS.minisig", DMG_NAME], "the signature before the big file");
}

#[test]
fn a_tampered_download_is_refused_and_deleted() {
    let mut dmg = DMG.to_vec();
    dmg[3] ^= 0x20;
    let server = github(SUMS, SIGNATURE, dmg);
    let (dir, got) = fetch(&server);
    assert_eq!(got, Err(update_verify::Refused::Mismatch.to_string()));
    assert!(files(&dir).is_empty(), "nothing kept: {:?}", files(&dir));
}

#[test]
fn tampered_sums_are_refused_before_the_download() {
    static TAMPERED: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let sums = TAMPERED.get_or_init(|| {
        // The DMG's line rewritten to vouch for another file: the signature no longer covers it.
        let mut evil = DMG.to_vec();
        evil.extend_from_slice(b"evil");
        let line = format!(
            "{}  {DMG_NAME}\n",
            update_verify::to_hex(ring::digest::digest(&ring::digest::SHA256, &evil).as_ref())
        );
        let rest = std::str::from_utf8(SUMS).unwrap().lines().skip(1).map(|l| format!("{l}\n")).collect::<String>();
        format!("{line}{rest}").into_bytes()
    });
    let server = github(sums, SIGNATURE, [DMG, b"evil"].concat());
    let (dir, got) = fetch(&server);
    assert_eq!(got, Err(update_verify::Refused::Signature.to_string()));
    assert!(files(&dir).is_empty());
    assert!(!server.paths().iter().any(|p| p.ends_with(DMG_NAME)), "the file wasn't downloaded");
}

#[test]
fn another_keys_signature_is_refused() {
    let other = include_str!("update_testdata/SHA256SUMS.other-key.minisig");
    let server = github(SUMS, other, DMG.to_vec());
    let (dir, got) = fetch(&server);
    assert_eq!(got, Err(update_verify::Refused::Signature.to_string()));
    assert!(files(&dir).is_empty());
}

#[test]
fn a_missing_signature_is_refused() {
    let server = serve(|path, _| match path.strip_prefix(FILES)? {
        "SHA256SUMS" => Some(SUMS.to_vec()),
        DMG_NAME => Some(DMG.to_vec()),
        _ => None,
    });
    let (dir, got) = fetch(&server);
    assert_eq!(got, Err("GitHub has no such release".into()));
    assert!(files(&dir).is_empty());
}

/// Events from the updater, as a window would get them.
#[derive(Clone)]
struct Events(Arc<Mutex<std_mpsc::Sender<ServiceEvent>>>);

impl Sink for Events {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.lock().unwrap().send(ev);
    }
}

/// A window's backend whose update commands go to the updater (and nothing else anywhere).
struct ToUpdater(Updater);

impl Backend for ToUpdater {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        if let Command::Update(cmd) = cmd {
            let _ = self.0.send((WindowId::FIRST, cmd));
        }
        Vec::new()
    }
}

/// Applies the updater's events to `s` until `done` holds of its update state.
fn until(s: &mut App, events: &std_mpsc::Receiver<ServiceEvent>, done: impl Fn(&Phase) -> bool) {
    while !done(&s.update.phase) {
        let ev = events.recv_timeout(WAIT).expect("the updater answers");
        s.apply(ev);
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap()
}

#[test]
fn the_launch_check_shows_the_toast_and_check_now_shows_its_result() {
    // 9.9.9 at launch; by the time the user checks again, the latest is this version itself.
    let running = Version::running().to_string();
    let server = serve(move |path, n| {
        (path == "/api/releases/latest").then(|| release(if n == 0 { "9.9.9" } else { &running }))
    });
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime();
    let _in = rt.enter();
    let (tx, events) = std_mpsc::channel();
    let updater = start(Events(Arc::new(Mutex::new(tx))), setup(server.endpoint(), test_key(), dir.path()));

    let mut s = fixture::app(fixture::demo());
    s.backend = Box::new(ToUpdater(updater));
    // Launch: the settings are restored, automatic checks are on, the service checks.
    s.remember(Saved::default(), Box::new(NoStore));
    until(&mut s, &events, |p| matches!(p, Phase::Available(_)));
    let toast = s.update.toast().expect("the toast");
    assert_eq!(toast.text, "Dromon 9.9.9 is available");
    assert_eq!(toast.buttons, [Button::Update, Button::Later, Button::Notes]);

    // Settings › General › Check now.
    s.act(Action::OpenSettings);
    s.act(Action::SettingsSection(SettingsSection::General));
    s.act(Action::Update(UpdateAction::CheckNow));
    until(&mut s, &events, |p| *p == Phase::UpToDate);
    assert_eq!(s.update.status(&Version::running()), format!("Dromon {} is up to date", Version::running()));
    assert!(s.update.toast().is_none());
    assert_eq!(server.paths(), ["/api/releases/latest", "/api/releases/latest"], "one request per check");
}

#[test]
fn installing_checks_the_download_then_hands_it_to_the_platform() {
    let server = github(SUMS, SIGNATURE, DMG.to_vec());
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime();
    let _in = rt.enter();
    let (tx, events) = std_mpsc::channel();
    let updater = start(Events(Arc::new(Mutex::new(tx))), setup(server.endpoint(), test_key(), dir.path()));
    let mut s = fixture::app(fixture::demo());
    s.backend = Box::new(ToUpdater(updater));
    s.act(Action::Update(UpdateAction::CheckNow));
    until(&mut s, &events, |p| matches!(p, Phase::Available(_)));
    s.act(Action::Update(UpdateAction::Update));
    until(&mut s, &events, |p| !matches!(p, Phase::Installing(_)));
    // The download passed both checks and reached the macOS installer, which runs on macOS
    // only; elsewhere it stops there, with the app as it was.
    let Phase::InstallFailed { error, .. } = &s.update.phase else { panic!("{:?}", s.update.phase) };
    if !cfg!(target_os = "macos") {
        assert_eq!(error, "the macOS app updates only on macOS");
    }
    assert_eq!(std::fs::read(dir.path().join(DMG_NAME)).unwrap(), DMG, "checked and kept");
}

#[test]
fn automatic_checks_wait_for_the_setting() {
    let server = serve(|_, _| Some(release("9.9.9")));
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime();
    let _in = rt.enter();
    let (tx, events) = std_mpsc::channel();
    let updater = start(Events(Arc::new(Mutex::new(tx))), setup(server.endpoint(), test_key(), dir.path()));
    updater.send((WindowId::FIRST, UpdateCommand::Auto(false))).unwrap();
    assert!(events.recv_timeout(Duration::from_millis(300)).is_err(), "off: no check by itself");
    assert!(server.paths().is_empty());
    updater.send((WindowId::FIRST, UpdateCommand::Auto(true))).unwrap();
    assert_eq!(events.recv_timeout(WAIT).unwrap(), ServiceEvent::Update(UpdateEvent::Checking));
    assert!(matches!(events.recv_timeout(WAIT).unwrap(), ServiceEvent::Update(UpdateEvent::Available(_))));
    // On again (another window's restore, the switch): checked today, so not again.
    updater.send((WindowId::FIRST, UpdateCommand::Auto(true))).unwrap();
    assert!(events.recv_timeout(Duration::from_millis(300)).is_err());
    assert_eq!(server.paths().len(), 1);
}

#[test]
fn the_updater_tells_the_windows_how_this_copy_was_installed_as_it_starts() {
    let server = serve(|_, _| None);
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime();
    let _in = rt.enter();
    let (tx, events) = std_mpsc::channel();
    let unknown = Setup { install: None, ..setup(server.endpoint(), test_key(), dir.path()) };
    let _updater = start(Events(Arc::new(Mutex::new(tx))), unknown);
    // A test binary in the build folder is no package Dromon knows.
    assert_eq!(events.recv_timeout(WAIT).unwrap(), ServiceEvent::Update(UpdateEvent::Install(Install::Other)));
    assert!(events.recv_timeout(Duration::from_millis(300)).is_err(), "once, and no check by itself");
    assert!(server.paths().is_empty());

    let mut s = fixture::app(fixture::demo());
    s.apply(ServiceEvent::Update(UpdateEvent::Install(Install::Deb)));
    assert_eq!(s.update.install, Some(Install::Deb), "the window keeps it for its bug reports");
    assert_eq!(s.update.phase, Phase::Idle, "and nothing else changes");
}
