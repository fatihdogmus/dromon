//! Transfer performance gates (plan §4.6, §9, §5 M5) against the sshd fixture
//! (`dromon-testkit`), all `#[ignore]`; `scripts/perf.sh` runs them and tabulates their
//! `METRIC gate=.. metric=.. value=..` lines:
//!
//! - `perf_large_file_vs_scp`: one large random file (`DROMON_PERF_MIB`, default 1024) down and
//!   up with `dromon_transfer::io` and with `scp`, `DROMON_PERF_RUNS` times (default 3), the
//!   order alternating; medians, and the median of the paired ratios.
//! - `perf_many_files_vs_scp_r`: a folder of `DROMON_PERF_FILES` files (default 8,192) of
//!   128 KiB, 1 GiB in all, down and up as the engine runs a folder job (scan, folders, then
//!   three files at a time on one session, `CopyOptions::default()`), against `scp -r`.
//! - `soak_large_file`: one `DROMON_SOAK_GIB` file (default 10) down, then up, SHA-1 checked
//!   both ways, while the resident memory of this process (the engine's side) and of its `ssh`
//!   child is sampled every 200 ms. Fails when either grows by more than 256 MiB. Checks free
//!   space (local and on the server) first.
//!
//! The copies run on the engine's own I/O (`io::download`, `io::upload`: part files, pipelined
//! requests, mtime, `fsync` and rename at the end) but without the engine's queue and its
//! progress snapshots. Local files go to `CARGO_TARGET_TMPDIR` (on disk, not a tmpfs) unless
//! `DROMON_PERF_DIR` names another folder. `scp` is the system's OpenSSH (9.0 and later speak
//! SFTP underneath by default), with the same key and options as our `ssh`.
//!
//! ```sh
//! cargo test --release -p dromon-transfer --test perf -- --ignored --nocapture --test-threads 1
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dromon_core::RemotePath;
use dromon_sftp::Sftp;
use dromon_testkit::{Fixture, Role, file_sha1, openssh};
use dromon_transfer::io::{CopyOptions, create_dirs, download, scan_local, scan_remote, upload};
use dromon_transfer::{FilePlan, Scan};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const MIB: u64 = 1024 * 1024;
/// The engine's default "Transfers at once" per host (§4.6, Settings).
const FILES_AT_ONCE: usize = 3;
/// How much the soak lets either process's resident memory grow over its start.
const SOAK_RSS_LIMIT_MIB: f64 = 256.0;

struct Conn {
    sftp: Sftp,
    fixture: &'static Fixture,
    ssh: Child,
}

async fn connect(test: &str) -> Option<Conn> {
    let fixture = dromon_testkit::fixture(test).await?;
    let mut ssh = Command::new(openssh("ssh"))
        .args(fixture.ssh_options())
        .args(["-s", &fixture.destination(Role::Key), "sftp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ssh");
    let stdout = ssh.stdout.take().unwrap();
    let stdin = ssh.stdin.take().unwrap();
    let sftp = Sftp::new(tokio::io::join(stdout, stdin)).await.expect("SFTP session");
    Some(Conn { sftp, fixture, ssh })
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().map_or(default, |v| v.parse().unwrap_or_else(|_| panic!("{name}={v}: not a number")))
}

/// Where local copies go: on disk, so writes and the final `fsync` cost what they cost.
fn local_dir(name: &str) -> tempfile::TempDir {
    let base =
        std::env::var_os("DROMON_PERF_DIR").map_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")), PathBuf::from);
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new().prefix(&format!("perf-{name}-")).tempdir_in(base).unwrap()
}

/// A fresh, empty folder for one test in the key user's home.
async fn scratch(c: &Conn, name: &str) -> RemotePath {
    let dir = c.fixture.path(Role::Key, &format!("scratch/perf-{name}-{}", std::process::id()));
    rm_remote(c.fixture, &dir);
    c.sftp.mkdir(&dir).await.unwrap();
    dir
}

/// `rm -rf` on the server (quicker than over SFTP for thousands of files).
fn rm_remote(fixture: &Fixture, path: &RemotePath) {
    fixture.exec(&["rm", "-rf", path.as_str()]).unwrap();
}

/// Runs `script` on the server as root, then gives the key user what it made.
fn on_server(fixture: &Fixture, script: &str, made: &RemotePath) {
    let script = format!("set -e; {script}; chown -R {} '{made}'", fixture.owner(Role::Key));
    fixture.exec(&["sh", "-c", &script]).unwrap();
}

/// Free bytes where `path` lives (`df -Pk`); `None` when `df` doesn't answer.
fn free_local(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").arg("-Pk").arg(path).output().ok()?;
    df_free(&String::from_utf8_lossy(&out.stdout))
}

fn free_remote(fixture: &Fixture, path: &RemotePath) -> Option<u64> {
    df_free(&fixture.exec(&["df", "-Pk", path.as_str()]).ok()?)
}

fn df_free(out: &str) -> Option<u64> {
    let line = out.lines().nth(1)?;
    line.split_whitespace().nth(3)?.parse::<u64>().ok().map(|k| k * 1024)
}

/// scp's arguments for the fixture, from `Fixture::ssh_options` (scp takes `-P` for the port
/// and has no `-T`, `-x` or `-e`).
fn scp_args(fixture: &Fixture) -> Vec<String> {
    let mut out = Vec::new();
    let mut opts = fixture.ssh_options().into_iter();
    while let Some(o) = opts.next() {
        match o.as_str() {
            "-T" | "-x" => {}
            "-e" => {
                opts.next();
            }
            "-p" => out.push("-P".into()),
            _ => out.push(o),
        }
    }
    out
}

async fn scp(fixture: &Fixture, extra: &[&str], from: &str, to: &str) -> Duration {
    let start = Instant::now();
    let status =
        Command::new(openssh("scp")).args(scp_args(fixture)).args(extra).args(["-q", from, to]).status().await.unwrap();
    assert!(status.success(), "scp {from} {to}");
    start.elapsed()
}

fn remote_at(fixture: &Fixture, path: &RemotePath) -> String {
    format!("{}:{path}", fixture.destination(Role::Key))
}

fn mbps(bytes: u64, took: Duration) -> f64 {
    bytes as f64 / 1e6 / took.as_secs_f64()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn metric(gate: &str, name: &str, value: f64) {
    println!("METRIC gate={gate} metric={name} value={value:.3}");
}

/// Runs a scan's files the way the engine runs a folder job: its folders first, then
/// [`FILES_AT_ONCE`] files at a time on the one session.
async fn run_scan(sftp: &Sftp, scan: Scan) {
    create_dirs(sftp, &scan.dirs).await.unwrap();
    let queue = Arc::new(Mutex::new(scan.files));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..FILES_AT_ONCE {
        let (sftp, queue) = (sftp.clone(), queue.clone());
        workers.spawn(async move {
            loop {
                let Some(plan) = queue.lock().await.pop() else { return };
                match &plan {
                    FilePlan::Download { .. } => {
                        drop(download(&sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap())
                    }
                    FilePlan::Upload { .. } => {
                        drop(upload(&sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap())
                    }
                    other => panic!("not a download or upload: {other:?}"),
                }
            }
        });
    }
    while let Some(done) = workers.join_next().await {
        done.unwrap();
    }
}

async fn dromon_down(sftp: &Sftp, src: &RemotePath, dst: &Path) -> Duration {
    let start = Instant::now();
    let scan = scan_remote(sftp, src, dst, |_| {}).await.unwrap();
    run_scan(sftp, scan).await;
    start.elapsed()
}

async fn dromon_up(sftp: &Sftp, src: &Path, dst: &RemotePath) -> Duration {
    let start = Instant::now();
    let scan = scan_local(src, dst, false, |_| {}).await.unwrap();
    run_scan(sftp, scan).await;
    start.elapsed()
}

fn remove_local(path: &Path) {
    if path.is_dir() {
        std::fs::remove_dir_all(path).unwrap();
    } else if path.exists() {
        std::fs::remove_file(path).unwrap();
    }
}

/// One paired measurement: dromon and scp, down and up, in an order that alternates by `run`.
struct Pair {
    down: (Duration, Duration),
    up: (Duration, Duration),
}

/// Copies `src` (a remote file or folder) down and back up with both tools; `check` verifies
/// each copy (the local one, then the remote one).
async fn pair(
    c: &Conn,
    run: usize,
    src: &RemotePath,
    dir: &RemotePath,
    local: &Path,
    recursive: bool,
    check: impl Fn(&Path, &RemotePath),
) -> Pair {
    let ours = local.join("dromon");
    let theirs = local.join("scp");
    let (ours_up, theirs_up) = (dir.join("up-dromon"), dir.join("up-scp"));
    let extra: &[&str] = if recursive { &["-r"] } else { &[] };
    let flip = run % 2 == 1;
    // Downloads.
    let mut d = (Duration::ZERO, Duration::ZERO);
    for first in [!flip, flip] {
        if first {
            d.0 = dromon_down(&c.sftp, src, &ours).await;
        } else {
            d.1 = scp(c.fixture, extra, &remote_at(c.fixture, src), theirs.to_str().unwrap()).await;
        }
    }
    // Uploads, both from our download.
    let mut u = (Duration::ZERO, Duration::ZERO);
    for first in [flip, !flip] {
        if first {
            u.0 = dromon_up(&c.sftp, &ours, &ours_up).await;
        } else {
            u.1 = scp(c.fixture, extra, ours.to_str().unwrap(), &remote_at(c.fixture, &theirs_up)).await;
        }
    }
    check(&ours, &ours_up);
    remove_local(&ours);
    remove_local(&theirs);
    rm_remote(c.fixture, &ours_up);
    rm_remote(c.fixture, &theirs_up);
    Pair { down: d, up: u }
}

fn report(gate: &str, bytes: u64, pairs: &[Pair]) {
    for (dir, pick) in [("down", (|p: &Pair| p.down) as fn(&Pair) -> (Duration, Duration)), ("up", |p: &Pair| p.up)] {
        let ours = median(pairs.iter().map(|p| mbps(bytes, pick(p).0)).collect());
        let theirs = median(pairs.iter().map(|p| mbps(bytes, pick(p).1)).collect());
        let ratio = median(pairs.iter().map(|p| 100.0 * pick(p).1.as_secs_f64() / pick(p).0.as_secs_f64()).collect());
        let all: Vec<String> =
            pairs.iter().map(|p| format!("{:.0}/{:.0}", mbps(bytes, pick(p).0), mbps(bytes, pick(p).1))).collect();
        println!("{gate} {dir}: dromon/scp MB/s per run {}", all.join(" "));
        metric(&format!("{gate}-{dir}"), "dromon_mbps", ours);
        metric(&format!("{gate}-{dir}"), "scp_mbps", theirs);
        metric(&format!("{gate}-{dir}"), "ratio_pct", ratio);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: copies a large file down and up with dromon and scp; `scripts/perf.sh --gate throughput`"]
async fn perf_large_file_vs_scp() {
    let Some(c) = connect("perf_large_file_vs_scp").await else { return };
    let size = env_u64("DROMON_PERF_MIB", 1024) * MIB;
    let runs = env_u64("DROMON_PERF_RUNS", 3) as usize;
    let dir = scratch(&c, "large").await;
    let src = dir.join("big.bin");
    on_server(c.fixture, &format!("head -c {size} /dev/urandom >'{src}'"), &src);
    let want = c.fixture.sha1(src.as_str()).unwrap();
    let local = local_dir("large");
    println!(
        "large file: {} MiB, chunk {} B, {} requests in flight",
        size / MIB,
        c.sftp.chunk_size(),
        CopyOptions::default().in_flight
    );
    let mut pairs = Vec::new();
    for run in 0..runs {
        let check = |ours: &Path, up: &RemotePath| {
            // Only the first run's bytes are hashed; later runs copy the same file the same way.
            if run == 0 {
                assert_eq!(file_sha1(ours).unwrap(), want, "the download's bytes");
                assert_eq!(c.fixture.sha1(up.as_str()).unwrap(), want, "the upload's bytes");
            }
        };
        pairs.push(pair(&c, run, &src, &dir, local.path(), false, check).await);
    }
    report("large", size, &pairs);
    rm_remote(c.fixture, &dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: copies 1 GiB of small files down and up with dromon and scp -r; `scripts/perf.sh --gate throughput`"]
async fn perf_many_files_vs_scp_r() {
    let Some(c) = connect("perf_many_files_vs_scp_r").await else { return };
    let files = env_u64("DROMON_PERF_FILES", 8192);
    let each = 128 * 1024;
    let runs = env_u64("DROMON_PERF_RUNS", 3) as usize;
    let dir = scratch(&c, "many").await;
    let src = dir.join("files");
    // One random stream cut into files: f.aaaaa, f.aaaab, ...
    on_server(
        c.fixture,
        &format!("mkdir '{src}'; head -c {} /dev/urandom | split -b {each} -a 5 - '{src}/f.'", files * each),
        &src,
    );
    let names = c.fixture.exec(&["sh", "-c", &format!("ls '{src}' | sort")]).unwrap();
    let names: Vec<&str> = names.lines().collect();
    assert_eq!(names.len() as u64, files);
    let sample: Vec<(String, String)> = [0, names.len() / 2, names.len() - 1]
        .iter()
        .map(|&i| (names[i].to_owned(), c.fixture.sha1(src.join(names[i]).as_str()).unwrap()))
        .collect();
    let local = local_dir("many");
    println!("many files: {files} × {} KiB", each / 1024);
    let mut pairs = Vec::new();
    for run in 0..runs {
        let check = |ours: &Path, up: &RemotePath| {
            assert_eq!(std::fs::read_dir(ours).unwrap().count() as u64, files, "files downloaded");
            for (name, sha) in &sample {
                assert_eq!(&file_sha1(&ours.join(name)).unwrap(), sha, "{name} downloaded");
                assert_eq!(&c.fixture.sha1(up.join(name).as_str()).unwrap(), sha, "{name} uploaded");
            }
            let count = c.fixture.exec(&["sh", "-c", &format!("ls '{up}' | wc -l")]).unwrap();
            assert_eq!(count.trim().parse::<u64>().unwrap(), files, "files uploaded");
        };
        pairs.push(pair(&c, run, &src, &dir, local.path(), true, check).await);
    }
    report("many", files * each, &pairs);
    rm_remote(c.fixture, &dir);
}

/// Resident memory of process `pid` in MiB (Linux: `VmRSS` in `/proc/<pid>/status`).
fn rss_mib(pid: u32) -> Option<f64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// Samples this process's and the ssh child's resident memory every 200 ms, until stopped.
struct Sampler {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Vec<(f64, f64)>>,
}

impl Sampler {
    fn start(ssh: u32) -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let me = std::process::id();
        let thread = std::thread::spawn(move || {
            let mut out = Vec::new();
            while !flag.load(Ordering::Relaxed) {
                if let (Some(a), Some(b)) = (rss_mib(me), rss_mib(ssh)) {
                    out.push((a, b));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            out
        });
        Sampler { stop, thread }
    }

    fn finish(self) -> Vec<(f64, f64)> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap()
    }
}

/// Peak and drift (mean of the last quarter minus mean of the first quarter) of one series.
fn shape(v: &[f64]) -> (f64, f64) {
    let q = (v.len() / 4).max(1);
    let mean = |s: &[f64]| s.iter().sum::<f64>() / s.len() as f64;
    (v.iter().copied().fold(0.0, f64::max), mean(&v[v.len() - q..]) - mean(&v[..q]))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: a 10 GiB file down and up (DROMON_SOAK_GIB); `scripts/perf.sh --gate soak`"]
async fn soak_large_file() {
    let Some(mut c) = connect("soak_large_file").await else { return };
    let gib = env_u64("DROMON_SOAK_GIB", 10);
    let size = gib * 1024 * MIB;
    let local = local_dir("soak");
    let dir = scratch(&c, "soak").await;
    // The server holds the source, then (once the source is gone) the upload; here the download.
    let (here, there) = (free_local(local.path()), free_remote(c.fixture, &dir));
    let margin = size + 2048 * MIB;
    if here.is_some_and(|f| f < margin) || there.is_some_and(|f| f < margin) {
        panic!("soak: {gib} GiB needs {} GiB free here and on the server: {here:?} / {there:?} B", margin / 1024 / MIB);
    }
    let src = dir.join("soak.bin");
    let t = Instant::now();
    on_server(c.fixture, &format!("head -c {size} /dev/urandom >'{src}'"), &src);
    let want = c.fixture.sha1(src.as_str()).unwrap();
    println!("soak: {gib} GiB made and hashed on the server in {:.1} s", t.elapsed().as_secs_f64());

    let ssh = c.ssh.id().expect("ssh runs");
    let base = (rss_mib(std::process::id()), rss_mib(ssh));
    let dst = local.path().join("soak.bin");
    let sampler = Sampler::start(ssh);
    let down = dromon_down(&c.sftp, &src, &dst).await;
    let down_rss = sampler.finish();
    assert_eq!(file_sha1(&dst).unwrap(), want, "the download's bytes");
    rm_remote(c.fixture, &src);

    let up_dst = dir.join("soak-up.bin");
    let sampler = Sampler::start(ssh);
    let up = dromon_up(&c.sftp, &dst, &up_dst).await;
    let up_rss = sampler.finish();
    assert_eq!(c.fixture.sha1(up_dst.as_str()).unwrap(), want, "the upload's bytes");
    rm_remote(c.fixture, &dir);
    drop(local);
    let _ = c.ssh.kill().await;

    metric("soak", "size_gib", gib as f64);
    metric("soak-down", "dromon_mbps", mbps(size, down));
    metric("soak-up", "dromon_mbps", mbps(size, up));
    metric("soak", "sha1_ok", 1.0);
    let Some(base) = base.0.zip(base.1) else {
        println!("soak: no /proc here, resident memory not sampled");
        return;
    };
    println!("soak: resident memory at start: this process {:.1} MiB, ssh {:.1} MiB", base.0, base.1);
    let mut grew: f64 = 0.0;
    for (phase, samples) in [("down", &down_rss), ("up", &up_rss)] {
        let ours: Vec<f64> = samples.iter().map(|s| s.0).collect();
        let theirs: Vec<f64> = samples.iter().map(|s| s.1).collect();
        let ((peak, drift), (ssh_peak, ssh_drift)) = (shape(&ours), shape(&theirs));
        println!(
            "soak {phase}: {} samples; this process peak {peak:.1} MiB (drift {drift:+.1}), ssh peak {ssh_peak:.1} MiB (drift {ssh_drift:+.1})",
            samples.len()
        );
        metric(&format!("soak-{phase}"), "rss_growth_mib", peak - base.0);
        metric(&format!("soak-{phase}"), "rss_drift_mib", drift);
        metric(&format!("soak-{phase}"), "ssh_rss_growth_mib", ssh_peak - base.1);
        grew = grew.max(peak - base.0).max(ssh_peak - base.1);
    }
    assert!(grew < SOAK_RSS_LIMIT_MIB, "resident memory grew by {grew:.0} MiB (limit {SOAK_RSS_LIMIT_MIB} MiB)");
}

#[test]
fn df_output_parses() {
    let out = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 100 40 60 40% /\n";
    assert_eq!(df_free(out), Some(60 * 1024));
    assert_eq!(df_free(""), None);
}
