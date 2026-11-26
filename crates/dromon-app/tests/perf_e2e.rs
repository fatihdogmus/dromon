//! The first-rows gate (plan §9: "first rows of a 10k folder painted under 150 ms after the
//! listing starts arriving") with the real service against the sshd fixture: the app in a
//! headless `Runtime`, `service::start` with the system `ssh` as the key user, and a folder of
//! 10,000 empty files made on the server. Each run navigates to the folder (`Action::GoTo`,
//! which sends `Command::List`) and times, from that moment:
//!
//! - `first_page_ms`: the first `ListingPage` of the folder arriving from the service thread
//!   (ssh, `sftp-server`'s READDIRs, the 512-entry page; no UI);
//! - `first_rows_ms`: that page applied and the next frame drawn with its rows mounted (what
//!   the gate asks, minus the GPU's part: headless frames aren't uploaded or presented);
//! - `whole_ms`: the last page applied.
//!
//! One warm-up run (the server's directory cache), then `DROMON_PERF_RUNS` (default 5),
//! medians. `#[ignore]`: `scripts/perf.sh --gate first-rows` runs it, or
//! `cargo test --release -p dromon-app --test perf_e2e -- --ignored --nocapture`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{Action, App, HostDirectory, Listing, ServiceEvent};
use dromon_core::{HostId, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key};
use rune::{Env, Runtime};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const SIZE: (f32, f32) = (1080.0, 680.0);
const WAIT: Duration = Duration::from_secs(30);
const FILES: usize = 10_000;

#[derive(Clone)]
struct TestSink(mpsc::Sender<ServiceEvent>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send(ev);
    }
}

/// An ssh config with one host, `fx-key`, for the fixture's key user. New host keys are
/// accepted (`accept-new`), so no sheet asks.
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
    for line in [
        format!("UserKnownHostsFile {}", dir.join("known_hosts").display()),
        format!("GlobalKnownHostsFile {NULL_DEVICE}"),
        "StrictHostKeyChecking accept-new".into(),
        "IdentitiesOnly yes".into(),
        "BatchMode no".into(),
    ] {
        writeln!(config, "    {line}").unwrap();
    }
    let path = dir.join("ssh_config");
    std::fs::write(&path, config).unwrap();
    path
}

struct Harness {
    rt: Runtime<App>,
    events: mpsc::Receiver<ServiceEvent>,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn apply(&mut self, ev: ServiceEvent) {
        self.rt.update(move |app, _| app.apply(ev));
    }

    /// Applies events as they come, a frame after each, until `done` holds.
    fn pump_until(&mut self, what: &str, mut done: impl FnMut(&Runtime<App>) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.rt) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            if let Ok(ev) = self.events.recv_timeout(left.min(Duration::from_millis(50))) {
                self.apply(ev);
                self.rt.frame(1.0 / 120.0);
            }
        }
    }
}

fn host() -> HostId {
    HostId("fx-key".into())
}

fn listed(rt: &Runtime<App>, path: &RemotePath) -> bool {
    rt.state.conn().is_some_and(|c| c.cwd == *path && matches!(c.listing, Listing::Ready(_)) && !c.loading_more())
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
#[ignore = "perf: run by `scripts/perf.sh --gate first-rows`"]
fn perf_first_rows_of_a_10k_folder() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let Some(fx) = rt.block_on(dromon_testkit::fixture("perf_first_rows_of_a_10k_folder")) else { return };
    let runs = std::env::var("DROMON_PERF_RUNS").ok().map_or(5, |v| v.parse().expect("DROMON_PERF_RUNS"));

    let big = fx.path(Role::Key, &format!("perf-{FILES}-{}", std::process::id()));
    let script = format!(
        "set -e; rm -rf '{big}'; mkdir '{big}'; cd '{big}'; seq -f 'IMG_%05g.jpg' 1 {FILES} | xargs touch; chown -R {} '{big}'",
        fx.owner(Role::Key)
    );
    fx.exec(&["sh", "-c", &script]).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let config = write_ssh_config(fx, dir.path());
    let (tx, events) = mpsc::channel();
    let (backend, commands) = service::channel();
    let cfg = Config {
        ssh: dromon_ssh::ssh_program(),
        helper_exe: PathBuf::from(HELPER),
        verbose: false,
        config_file: Some(config.clone()),
        discover: Some(dromon_ssh::ConfigSources {
            user: Some(config),
            system: None,
            user_ssh_dir: dir.path().to_path_buf(),
        }),
        hosts_file: None,
        hosts: Vec::new(),
        without_env: vec!["SSH_AUTH_SOCK".into()],
    };
    service::start(commands, TestSink(tx), cfg).expect("start the service thread");
    let mut runtime = dromon_app::app().headless_runtime(SIZE, Env::new(true, 2.0));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    runtime.update(move |s, _| *s = App::new(HostDirectory::default(), now, Box::new(backend)));
    runtime.frame(0.0);
    let mut h = Harness { rt: runtime, events, _dir: dir };
    h.pump_until("fx-key in the sidebar", |rt| rt.state.hosts.get(&host()).is_some());
    h.rt.update(|s, _| drop(s.act(Action::SelectHost(host()))));
    let home = fx.home(Role::Key);
    h.pump_until("the home folder listed", |rt| listed(rt, &home));

    let (mut first_page, mut first_rows, mut whole) = (Vec::new(), Vec::new(), Vec::new());
    for run in 0..=runs {
        let t0 = Instant::now();
        let to = big.clone();
        h.rt.update(move |s, _| drop(s.act(Action::GoTo(to))));
        h.rt.frame(1.0 / 120.0);
        // Events up to the folder's first page.
        let (page, rows) = loop {
            let ev = h.events.recv_timeout(WAIT).expect("the folder's first page");
            let first = matches!(&ev, ServiceEvent::ListingPage { path, first: true, .. } if *path == big);
            let arrived = t0.elapsed();
            h.apply(ev);
            h.rt.frame(1.0 / 120.0);
            if first {
                let name = h.rt.state.conn().map(|c| c.row(0).name.clone()).expect("rows");
                assert!(h.rt.find(&format!("rows/{name}")).is_some(), "the first page's rows are mounted");
                break (arrived, t0.elapsed());
            }
        };
        h.pump_until("the whole folder", |rt| listed(rt, &big));
        let all = t0.elapsed();
        assert_eq!(h.rt.state.conn().map(|c| c.row_count()), Some(FILES));
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        println!("run {run}: first page {:.1} ms, first rows {:.1} ms, whole {:.1} ms", ms(page), ms(rows), ms(all));
        if run > 0 {
            first_page.push(ms(page));
            first_rows.push(ms(rows));
            whole.push(ms(all));
        }
        let back = home.clone();
        h.rt.update(move |s, _| drop(s.act(Action::GoTo(back))));
        h.pump_until("back home", |rt| listed(rt, &home));
    }
    println!("METRIC gate=first-rows-10k-sshd metric=first_page_ms value={:.3}", median(first_page));
    println!("METRIC gate=first-rows-10k-sshd metric=first_rows_ms value={:.3}", median(first_rows));
    println!("METRIC gate=first-rows-10k-sshd metric=whole_ms value={:.3}", median(whole));
    fx.exec(&["rm", "-rf", big.as_str()]).unwrap();
}
