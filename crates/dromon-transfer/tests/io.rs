//! `dromon_transfer::io` against the sshd fixture (`dromon-testkit`: the Docker image of
//! `tests/docker/`, or the native sshd): scans, downloads and uploads with part files, resume,
//! cancel, mtimes and name clashes. Each test opens its own `ssh -s … sftp` child
//! (`openssh("ssh")`, on Windows the inbox OpenSSH) as the key user, uses its own scratch folder
//! in that user's home on the server and a temp folder locally. When the backend can't run each
//! test prints a skip line and passes (fails with `DROMON_REQUIRE_DOCKER=1`). The 256 MiB
//! throughput test is `#[ignore]`:
//! `cargo test -p dromon-transfer --test io -- --ignored --nocapture`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use dromon_core::{EntryKind, Error, RemotePath};
use dromon_sftp::{Sftp, WriteMode};
use dromon_testkit::{Fixture, Role, file_sha1, openssh};
use dromon_transfer::io::{
    CopyOptions, create_dirs, download, free_remote_name, local_side, remote_existing, remote_kind, remote_side,
    remove_part, scan_local, scan_remote, upload,
};
use dromon_transfer::{DirPlan, FilePlan, Scan, Side, is_part_file};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

const MIB: u64 = 1024 * 1024;

struct Conn {
    sftp: Sftp,
    fixture: &'static Fixture,
    _ssh: Child,
}

impl Conn {
    /// The seed's mixed folder in the key user's home.
    fn mixed(&self) -> String {
        self.fixture.path(Role::Key, "fixture/mixed").as_str().to_owned()
    }
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
    // The I/O, not the reply timeout, is under test: a loaded Windows CI (WSL port
    // forwarding) can take longer than the default 10 s for one reply.
    sftp.set_timeout(60);
    Some(Conn { sftp, fixture, _ssh: ssh })
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// A fresh, empty scratch folder for one test.
async fn scratch(c: &Conn, name: &str) -> RemotePath {
    let dir = c.fixture.path(Role::Key, &format!("scratch/xfer-{name}-{}", std::process::id()));
    rm_rf(&c.sftp, &dir).await;
    c.sftp.mkdir(&dir).await.unwrap();
    dir
}

async fn rm_rf(sftp: &Sftp, path: &RemotePath) {
    let Ok(entry) = sftp.lstat(path).await else { return };
    if entry.kind == EntryKind::Dir {
        for child in sftp.list(path).await.unwrap() {
            Box::pin(rm_rf(sftp, &path.join(&child.name))).await;
        }
        sftp.remove_dir(path).await.unwrap();
    } else {
        sftp.remove_file(path).await.unwrap();
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed ^ (i >> 13) as u8)).collect()
}

async fn read_remote(sftp: &Sftp, path: &RemotePath) -> Vec<u8> {
    let file = sftp.open_read(path).await.unwrap();
    let mut out = Vec::new();
    loop {
        let data = file.read_at(out.len() as u64, sftp.chunk_size()).await.unwrap();
        if data.is_empty() {
            break;
        }
        out.extend(data);
    }
    file.close().await.unwrap();
    out
}

async fn write_remote(sftp: &Sftp, path: &RemotePath, data: &[u8]) {
    let file = sftp.open_write(path, WriteMode::Truncate).await.unwrap();
    let chunk = sftp.chunk_size() as usize;
    let acks: Vec<_> = data.chunks(chunk).enumerate().map(|(i, c)| file.write_at((i * chunk) as u64, c)).collect();
    for ack in acks {
        ack.await.unwrap();
    }
    file.close().await.unwrap();
}

/// `len` random bytes at `path` on the server, owned by the key user.
fn random_remote(fixture: &Fixture, path: &RemotePath, len: u64) {
    let script = format!("head -c {len} /dev/urandom >'{path}' && chown {} '{path}'", fixture.owner(Role::Key));
    fixture.exec(&["sh", "-c", &script]).unwrap();
}

/// SHA-1 computed on the server (`Fixture::sha1`: `sha1sum` in the container), so the check
/// doesn't go through SFTP.
fn sha_remote(fixture: &Fixture, path: &RemotePath) -> String {
    fixture.sha1(path.as_str()).unwrap()
}

/// SHA-1 computed here (no hashing tool needed on the host, Windows included).
fn sha_local(path: &Path) -> String {
    file_sha1(path).unwrap()
}

/// The engine runs these in spawned tasks, so their futures must be `Send` (checked at compile
/// time; never called).
#[allow(dead_code)]
fn futures_are_send(sftp: &Sftp, plan: &FilePlan) {
    fn send<T: Send>(_: T) {}
    send(scan_remote(sftp, &p("/"), Path::new("x"), |_| {}));
    send(scan_local(Path::new("x"), &p("/x"), false, |_| {}));
    send(create_dirs(sftp, &[]));
    send(download(sftp, plan, CopyOptions::default(), |_| {}));
    send(upload(sftp, plan, CopyOptions::default(), |_| {}));
    send(remove_part(sftp, plan));
    send(remote_kind(sftp, &p("/")));
    send(free_remote_name(sftp, &p("/"), "a"));
}

fn mtime_local(path: &Path) -> i64 {
    let t = std::fs::metadata(path).unwrap().modified().unwrap();
    t.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn set_mtime_local(path: &Path, secs: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

/// Every name in a local tree that looks like a part file.
fn local_parts(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            out.extend(local_parts(&path));
        } else if is_part_file(&entry.file_name().to_string_lossy()) {
            out.push(path);
        }
    }
    out
}

/// Every name in a remote tree that looks like a part file.
async fn remote_parts(sftp: &Sftp, dir: &RemotePath) -> Vec<RemotePath> {
    let mut out = Vec::new();
    for entry in sftp.list(dir).await.unwrap() {
        let path = dir.join(&entry.name);
        if entry.kind == EntryKind::Dir {
            out.extend(Box::pin(remote_parts(sftp, &path)).await);
        } else if is_part_file(&entry.name) {
            out.push(path);
        }
    }
    out
}

fn the_plan(scan: Scan) -> FilePlan {
    assert_eq!(scan.files.len(), 1, "{scan:?}");
    scan.files.into_iter().next().unwrap()
}

fn rate(bytes: u64, took: Duration) -> String {
    format!("{:.2} s, {:.1} MB/s", took.as_secs_f64(), bytes as f64 / 1e6 / took.as_secs_f64())
}

/// Runs a copy in its own task, as the engine does, and aborts it after the first progress
/// report. Returns the progress values seen.
async fn abort_after_first_progress<F>(copy: impl FnOnce(mpsc::UnboundedSender<u64>) -> F) -> Vec<u64>
where
    F: Future<Output = ()> + Send + 'static,
{
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(copy(tx));
    let first = rx.recv().await.expect("a progress report before the end");
    task.abort();
    let joined = task.await;
    assert!(joined.is_err_and(|e| e.is_cancelled()), "the copy finished before it was aborted");
    let mut seen = vec![first];
    while let Ok(n) = rx.try_recv() {
        seen.push(n);
    }
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn download_a_file_keeps_bytes_and_mtime() {
    let Some(c) = connect("download_a_file_keeps_bytes_and_mtime").await else { return };
    let s = &c.sftp;
    let mixed = c.mixed();
    let tmp = tempfile::tempdir().unwrap();
    let src = p(&format!("{mixed}/file-001.txt"));
    let dst = tmp.path().join("file-001.txt");
    let mut counts = Vec::new();
    let plan = the_plan(scan_remote(s, &src, &dst, |n| counts.push(n)).await.unwrap());
    assert_eq!(counts, [1]);
    assert_eq!(plan, FilePlan::Download { src: src.clone(), dst: dst.clone(), size: 100, mtime: Some(1_577_934_245) });

    let mut progress = Vec::new();
    let got = download(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert_eq!(got, dst);
    assert_eq!(progress.last(), Some(&100));
    assert_eq!(std::fs::read(&dst).unwrap(), read_remote(s, &src).await);
    assert_eq!(mtime_local(&dst), 1_577_934_245);
    let names: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, ["file-001.txt"], "no part file left");

    // An empty file, and a link the user picked (followed: planned as its target).
    let empty = tmp.path().join("empty.txt");
    let plan = the_plan(scan_remote(s, &p(&format!("{mixed}/empty.txt")), &empty, |_| {}).await.unwrap());
    assert_eq!(download(s, &plan, CopyOptions::default(), |_| {}).await.unwrap(), empty);
    assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0);
    let link = tmp.path().join("link-file");
    let plan = the_plan(scan_remote(s, &p(&format!("{mixed}/link-file")), &link, |_| {}).await.unwrap());
    assert!(matches!(plan, FilePlan::Download { size: 100, mtime: Some(1_577_934_245), .. }));
    download(s, &plan, CopyOptions { in_flight: 1, ..CopyOptions::default() }, |_| {}).await.unwrap();
    assert_eq!(std::fs::read(&link).unwrap(), vec![0u8; 100]);

    // Wrong variant: a programming error, not a panic.
    let wrong = FilePlan::Upload { src: dst.clone(), dst: src.clone(), size: 1, mtime: None, overwrite: false };
    assert!(matches!(download(s, &wrong, CopyOptions::default(), |_| {}).await, Err(Error::Other(_))));
    assert!(matches!(upload(s, &plan, CopyOptions::default(), |_| {}).await, Err(Error::Other(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn download_mixed_folder_recursively() {
    let Some(c) = connect("download_mixed_folder_recursively").await else { return };
    let s = &c.sftp;
    let mixed = c.mixed();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("mixed");
    let mut counts = Vec::new();
    let scan = scan_remote(s, &p(&mixed), &root, |n| counts.push(n)).await.unwrap();

    assert_eq!(
        scan.skipped,
        [
            "mixed/dangling: a broken link",
            "mixed/link-dir: a link to a folder",
            "mixed/locked: permission denied",
            "mixed/pipe: a named pipe",
        ]
    );
    let mut want_dirs = vec![root.clone(), root.join(".config")];
    want_dirs.extend((1..=10).map(|i| root.join(format!("dir-{i:02}"))));
    assert_eq!(scan.dirs, want_dirs.into_iter().map(DirPlan::Local).collect::<Vec<_>>());

    let mut want_files: Vec<String> = vec![".hidden".into(), "dir-01/inside.txt".into(), "empty.txt".into()];
    want_files.extend((1..=80).map(|i| format!("file-{i:03}.txt")));
    want_files.extend(["link-file", "space name.txt", "unicode-ğüş.txt"].map(String::from));
    let srcs: Vec<String> = scan
        .files
        .iter()
        .map(|f| match f {
            FilePlan::Download { src, .. } => src.as_str().strip_prefix(&format!("{mixed}/")).unwrap().to_owned(),
            other => panic!("not a download plan in a remote scan: {other:?}"),
        })
        .collect();
    assert_eq!(srcs, want_files);
    assert_eq!(counts, [64, 86]);
    let FilePlan::Download { dst, size, .. } = &scan.files[1] else { unreachable!() };
    assert_eq!((dst, *size), (&root.join("dir-01").join("inside.txt"), 7));

    create_dirs(s, &scan.dirs).await.unwrap();
    create_dirs(s, &scan.dirs).await.unwrap(); // already there: fine
    for plan in &scan.files {
        let FilePlan::Download { src, dst, .. } = plan else { unreachable!() };
        let got = download(s, plan, CopyOptions::default(), |_| {}).await.unwrap();
        assert_eq!(&got, dst);
        assert!(std::fs::read(&got).unwrap() == read_remote(s, src).await, "{src} differs");
    }
    assert_eq!(std::fs::read(root.join("link-file")).unwrap(), vec![0u8; 100]);
    assert_eq!(mtime_local(&root.join("link-file")), 1_577_934_245);
    assert!(root.join("dir-10").is_dir() && root.join(".config").is_dir());
    assert!(std::fs::read_dir(root.join("dir-10")).unwrap().next().is_none());
    assert!(!root.join("locked").exists() && !root.join("link-dir").exists());
    assert!(local_parts(&root).is_empty());

    // Errors on the picked item itself are returned.
    let locked = p(&format!("{mixed}/locked"));
    assert_eq!(scan_remote(s, &locked, &root, |_| {}).await, Err(Error::PermissionDenied(locked)));
    let missing = p(&format!("{mixed}/nope"));
    assert_eq!(scan_remote(s, &missing, &root, |_| {}).await, Err(Error::NotFound(missing)));
    let dangling = p(&format!("{mixed}/dangling"));
    assert_eq!(scan_remote(s, &dangling, &root, |_| {}).await, Err(Error::NotFound(dangling)));
    // A picked link to a folder is followed.
    let via_link = scan_remote(s, &p(&format!("{mixed}/link-dir")), &tmp.path().join("ld"), |_| {}).await.unwrap();
    assert_eq!(via_link.dirs, [DirPlan::Local(tmp.path().join("ld"))]);
    assert_eq!(via_link.files.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn download_resumes_from_the_part_file() {
    let Some(c) = connect("download_resumes_from_the_part_file").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "dl-resume").await;
    let src = dir.join("big.bin");
    random_remote(c.fixture, &src, 8 * MIB);
    let data = read_remote(s, &src).await;
    let tmp = tempfile::tempdir().unwrap();
    let dst = tmp.path().join("big.bin");
    let part = tmp.path().join("big.bin.dromon-part");
    let plan = the_plan(scan_remote(s, &src, &dst, |_| {}).await.unwrap());

    std::fs::write(&part, &data[..3 * MIB as usize]).unwrap();
    let mut progress = Vec::new();
    download(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert_eq!(progress.first(), Some(&(3 * MIB)), "resumed at the part's length");
    assert!(progress.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(progress.last(), Some(&(8 * MIB)));
    assert_eq!(sha_local(&dst), sha_remote(c.fixture, &src));
    assert!(!part.exists());

    // The source changed since the scan (different mtime in the plan): start over, and the
    // garbage in the part file is gone.
    std::fs::remove_file(&dst).unwrap();
    std::fs::write(&part, vec![0xAA; 3 * MIB as usize]).unwrap();
    let FilePlan::Download { mtime, .. } = &plan else { unreachable!() };
    let stale = FilePlan::Download { src: src.clone(), dst: dst.clone(), size: 8 * MIB, mtime: mtime.map(|m| m - 10) };
    let mut progress = Vec::new();
    download(s, &stale, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert!(progress[0] <= u64::from(s.chunk_size()), "started over: {}", progress[0]);
    assert_eq!(sha_local(&dst), sha_remote(c.fixture, &src));

    // A part longer than the source isn't resumed either.
    std::fs::remove_file(&dst).unwrap();
    std::fs::write(&part, vec![0xBB; 9 * MIB as usize]).unwrap();
    download(s, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert_eq!(sha_local(&dst), sha_remote(c.fixture, &src));

    // A source smaller than planned (it changed since the scan, or a retry after "changed on
    // the server") is downloaded as it is now, not refused again.
    std::fs::remove_file(&dst).unwrap();
    let shrunk = FilePlan::Download { src: src.clone(), dst: dst.clone(), size: 9 * MIB, mtime: *mtime };
    download(s, &shrunk, CopyOptions::default(), |_| {}).await.unwrap();
    assert_eq!(sha_local(&dst), sha_remote(c.fixture, &src));
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_download_leaves_a_part_and_resumes() {
    let Some(c) = connect("cancelled_download_leaves_a_part_and_resumes").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "dl-cancel").await;
    let src = dir.join("big.bin");
    random_remote(c.fixture, &src, 32 * MIB);
    let tmp = tempfile::tempdir().unwrap();
    let dst = tmp.path().join("big.bin");
    let part = tmp.path().join("big.bin.dromon-part");
    let plan = the_plan(scan_remote(s, &src, &dst, |_| {}).await.unwrap());

    let (sftp, task_plan) = (s.clone(), plan.clone());
    let seen = abort_after_first_progress(move |tx| async move {
        let _ = download(&sftp, &task_plan, CopyOptions::default(), move |n| {
            let _ = tx.send(n);
        })
        .await;
    })
    .await;
    assert!(seen.iter().all(|&n| n < 32 * MIB));
    // Let the last local write that was in flight land.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!dst.exists(), "nothing under the final name");
    let kept = std::fs::metadata(&part).expect("the part file stays").len();
    assert!(kept > 0 && kept < 32 * MIB, "{kept}");

    let mut progress = Vec::new();
    let got = download(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert_eq!(got, dst);
    assert_eq!(progress[0], kept, "resumed where the cancelled copy stopped");
    assert_eq!(sha_local(&dst), sha_remote(c.fixture, &src));
    assert!(!part.exists());

    // Cancel, then discard the part.
    std::fs::write(&part, b"partial").unwrap();
    remove_part(s, &plan).await;
    assert!(!part.exists());
    remove_part(s, &plan).await; // nothing there: fine
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn download_to_a_name_taken_meanwhile_keeps_both() {
    let Some(c) = connect("download_to_a_name_taken_meanwhile_keeps_both").await else { return };
    let s = &c.sftp;
    let mixed = c.mixed();
    let tmp = tempfile::tempdir().unwrap();
    let dst = tmp.path().join("file-002.txt");
    let plan = the_plan(scan_remote(s, &p(&format!("{mixed}/file-002.txt")), &dst, |_| {}).await.unwrap());
    std::fs::write(&dst, b"mine").unwrap();
    let got = download(s, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert_eq!(got, tmp.path().join("file-002 (1).txt"));
    assert_eq!(std::fs::read(&dst).unwrap(), b"mine");
    assert_eq!(std::fs::read(&got).unwrap(), vec![0u8; 200]);
    assert!(!tmp.path().join("file-002.txt.dromon-part").exists());

    // Another download's part file also counts as taken.
    std::fs::write(tmp.path().join("file-002 (2).txt.dromon-part"), b"someone else's part").unwrap();
    let got = download(s, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert_eq!(got, tmp.path().join("file-002 (3).txt"));
    assert_eq!(std::fs::read(&got).unwrap(), vec![0u8; 200]);
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_a_file_and_a_folder_tree() {
    let Some(c) = connect("upload_a_file_and_a_folder_tree").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "up-tree").await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("site");
    std::fs::create_dir_all(root.join("assets/img")).unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    std::fs::write(root.join("index.html"), b"<h1>hi</h1>").unwrap();
    std::fs::write(root.join("assets/app.js"), pattern(700_001, 1)).unwrap();
    std::fs::write(root.join("assets/img/logo.png"), pattern(3 * MIB as usize + 17, 2)).unwrap();
    std::fs::write(root.join("assets/zero"), b"").unwrap();
    set_mtime_local(&root.join("index.html"), 1_600_000_000);

    // One file.
    let one = dir.join("index.html");
    let plan = the_plan(scan_local(&root.join("index.html"), &one, false, |_| {}).await.unwrap());
    let mut progress = Vec::new();
    assert_eq!(upload(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap(), one);
    assert_eq!(progress, [11]);
    assert_eq!(read_remote(s, &one).await, b"<h1>hi</h1>");
    assert_eq!(s.stat(&one).await.unwrap().mtime, Some(1_600_000_000));

    // The tree, empty folder included.
    let remote_root = dir.join("site");
    let scan = scan_local(&root, &remote_root, false, |_| {}).await.unwrap();
    let dirs: Vec<_> = scan
        .dirs
        .iter()
        .map(|d| match d {
            DirPlan::Remote(r) => r.as_str().strip_prefix(dir.as_str()).unwrap().to_owned(),
            DirPlan::Local(_) => panic!("a local folder in a local scan"),
        })
        .collect();
    assert_eq!(dirs, ["/site", "/site/assets", "/site/assets/img", "/site/empty"]);
    assert_eq!(scan.files.len(), 4);
    create_dirs(s, &scan.dirs).await.unwrap();
    for plan in &scan.files {
        let FilePlan::Upload { src, dst, .. } = plan else { unreachable!() };
        assert_eq!(&upload(s, plan, CopyOptions::default(), |_| {}).await.unwrap(), dst);
        assert!(read_remote(s, dst).await == std::fs::read(src).unwrap(), "{dst} differs");
        assert_eq!(s.stat(dst).await.unwrap().mtime, Some(mtime_local(src)), "{dst} mtime");
    }
    assert_eq!(remote_kind(s, &remote_root.join("empty")).await, Ok(Some(true)));
    assert!(s.list(&remote_root.join("empty")).await.unwrap().is_empty());
    assert!(remote_parts(s, &dir).await.is_empty());

    // A file in the way of a folder to create.
    let blocked = vec![DirPlan::Remote(one.clone())];
    assert_eq!(create_dirs(s, &blocked).await, Err(Error::AlreadyExists(one.clone())));
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_resumes_and_survives_a_cancel() {
    let Some(c) = connect("upload_resumes_and_survives_a_cancel").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "up-resume").await;
    let tmp = tempfile::tempdir().unwrap();

    // Resume from a part the server already has.
    let src = tmp.path().join("data.bin");
    let data = pattern(5 * MIB as usize + 123, 3);
    std::fs::write(&src, &data).unwrap();
    let dst = dir.join("data.bin");
    let part = dir.join(".data.bin.dromon-part");
    let plan = the_plan(scan_local(&src, &dst, false, |_| {}).await.unwrap());
    write_remote(s, &part, &data[..2 * MIB as usize]).await;
    let mut progress = Vec::new();
    upload(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert_eq!(progress.first(), Some(&(2 * MIB)));
    assert_eq!(progress.last(), Some(&(data.len() as u64)));
    assert!(read_remote(s, &dst).await == data);
    assert!(!s.exists(&part).await.unwrap());

    // Not resumed when the local file changed since the scan.
    rm_rf(s, &dst).await;
    write_remote(s, &part, &vec![0xEE; 2 * MIB as usize]).await;
    let FilePlan::Upload { mtime, .. } = &plan else { unreachable!() };
    let stale = FilePlan::Upload {
        src: src.clone(),
        dst: dst.clone(),
        size: data.len() as u64,
        mtime: mtime.map(|m| m + 5),
        overwrite: false,
    };
    let mut progress = Vec::new();
    upload(s, &stale, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert!(progress[0] <= u64::from(s.chunk_size()));
    assert!(read_remote(s, &dst).await == data);

    // Cancel mid-way, then finish.
    let big = tmp.path().join("big.bin");
    let big_data = pattern(32 * MIB as usize, 4);
    std::fs::write(&big, &big_data).unwrap();
    let big_dst = dir.join("big.bin");
    let big_part = dir.join(".big.bin.dromon-part");
    let plan = the_plan(scan_local(&big, &big_dst, false, |_| {}).await.unwrap());
    let (sftp, task_plan) = (s.clone(), plan.clone());
    abort_after_first_progress(move |tx| async move {
        let _ = upload(&sftp, &task_plan, CopyOptions::default(), move |n| {
            let _ = tx.send(n);
        })
        .await;
    })
    .await;
    assert!(!s.exists(&big_dst).await.unwrap(), "nothing under the final name");
    let kept = s.stat(&big_part).await.expect("the part stays").size;
    assert!(kept > 0 && kept < 32 * MIB, "{kept}");
    let mut progress = Vec::new();
    upload(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert!(progress[0] > 0, "resumed");
    assert_eq!(sha_remote(c.fixture, &big_dst), sha_local(&big));
    assert!(!s.exists(&big_part).await.unwrap());

    write_remote(s, &big_part, b"partial").await;
    remove_part(s, &plan).await;
    assert!(!s.exists(&big_part).await.unwrap());
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_replace_and_keep_both() {
    let Some(c) = connect("upload_replace_and_keep_both").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "up-clash").await;
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("report.pdf");
    std::fs::write(&src, b"new contents").unwrap();
    let dst = dir.join("report.pdf");

    // Replace: the existing file is overwritten.
    write_remote(s, &dst, b"old").await;
    let plan = the_plan(scan_local(&src, &dst, true, |_| {}).await.unwrap());
    assert_eq!(upload(s, &plan, CopyOptions::default(), |_| {}).await.unwrap(), dst);
    assert_eq!(read_remote(s, &dst).await, b"new contents");

    // Keep both: a file that appeared at dst after planning gets a free name.
    let other = dir.join("notes.txt");
    let plan = the_plan(scan_local(&src, &other, false, |_| {}).await.unwrap());
    write_remote(s, &other, b"theirs").await;
    let got = upload(s, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert_eq!(got, dir.join("notes (1).txt"));
    assert_eq!(read_remote(s, &other).await, b"theirs");
    assert_eq!(read_remote(s, &got).await, b"new contents");
    assert!(remote_parts(s, &dir).await.is_empty());
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_kinds_and_free_names() {
    let Some(c) = connect("remote_kinds_and_free_names").await else { return };
    let s = &c.sftp;
    let mixed = c.mixed();
    let m = |name: &str| p(&format!("{mixed}/{name}"));
    assert_eq!(remote_kind(s, &p(&mixed)).await, Ok(Some(true)));
    assert_eq!(remote_kind(s, &m("link-dir")).await, Ok(Some(true)));
    assert_eq!(remote_kind(s, &m("locked")).await, Ok(Some(true)));
    assert_eq!(remote_kind(s, &m("file-001.txt")).await, Ok(Some(false)));
    assert_eq!(remote_kind(s, &m("link-file")).await, Ok(Some(false)));
    assert_eq!(remote_kind(s, &m("dangling")).await, Ok(Some(false)));
    assert_eq!(remote_kind(s, &m("pipe")).await, Ok(Some(false)));
    assert_eq!(remote_kind(s, &m("nope")).await, Ok(None));

    assert_eq!(free_remote_name(s, &p(&mixed), "file-001.txt").await.unwrap(), "file-001 (1).txt");
    assert_eq!(free_remote_name(s, &p(&mixed), "dir-01").await.unwrap(), "dir-01 (1)");
    assert_eq!(free_remote_name(s, &p(&mixed), "new.txt").await.unwrap(), "new.txt");

    let dir = scratch(&c, "names").await;
    write_remote(s, &dir.join(".a.txt.dromon-part"), b"x").await;
    write_remote(s, &dir.join("a (1).txt"), b"x").await;
    assert_eq!(free_remote_name(s, &dir, "a.txt").await.unwrap(), "a (2).txt");
    let missing = dir.join("missing");
    assert_eq!(free_remote_name(s, &missing, "a").await, Err(Error::NotFound(missing)));
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clash_reports_the_size_and_date_of_both_sides() {
    let Some(c) = connect("a_clash_reports_the_size_and_date_of_both_sides").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "sides").await;
    write_remote(s, &dir.join("f.txt"), b"theirs").await;
    s.mkdir(&dir.join("d")).await.unwrap();

    let file = remote_existing(s, &dir.join("f.txt")).await.unwrap().unwrap();
    assert!(!file.is_dir);
    assert_eq!(file.side.size, Some(6));
    assert!(file.side.mtime.is_some_and(|t| t > 1_600_000_000));
    assert_eq!(remote_side(s, &dir.join("f.txt")).await, file.side);

    let folder = remote_existing(s, &dir.join("d")).await.unwrap().unwrap();
    assert!(folder.is_dir);
    assert_eq!((folder.side.size, folder.side.mtime.is_some()), (None, true));
    assert_eq!(remote_existing(s, &dir.join("nope")).await, Ok(None));
    assert_eq!(remote_side(s, &dir.join("nope")).await, Side::default());

    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("mine.txt");
    std::fs::write(&local, b"twelve bytes").unwrap();
    let side = local_side(&local).await;
    assert_eq!(side.size, Some(12));
    assert!(side.mtime.is_some_and(|t| t > 1_600_000_000));
    let folder = local_side(tmp.path()).await;
    assert_eq!((folder.size, folder.mtime.is_some()), (None, true));
    assert_eq!(local_side(&tmp.path().join("nope")).await, Side::default());
    rm_rf(s, &dir).await;
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

async fn scp(fixture: &Fixture, from: &str, to: &str) -> Duration {
    let start = Instant::now();
    let status = Command::new(openssh("scp")).args(scp_args(fixture)).args(["-q", from, to]).status().await.unwrap();
    assert!(status.success(), "scp {from} {to}");
    start.elapsed()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: copies 256 MiB four times; run with `-- --ignored --nocapture`"]
async fn throughput_256_mib_against_scp() {
    let Some(c) = connect("throughput_256_mib_against_scp").await else { return };
    let s = &c.sftp;
    const SIZE: u64 = 256 * MIB;
    let dir = scratch(&c, "speed").await;
    let src = dir.join("big.bin");
    random_remote(c.fixture, &src, SIZE);
    let want = sha_remote(c.fixture, &src);
    let tmp = tempfile::tempdir().unwrap();
    let dst = tmp.path().join("big.bin");
    let remote_at = |path: &RemotePath| format!("{}:{path}", c.fixture.destination(Role::Key));
    println!("chunk size {}, in flight {}", s.chunk_size(), CopyOptions::default().in_flight);

    let plan = the_plan(scan_remote(s, &src, &dst, |_| {}).await.unwrap());
    let start = Instant::now();
    download(s, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    let down = start.elapsed();
    assert_eq!(sha_local(&dst), want);

    let up_dst = dir.join("up.bin");
    let plan = the_plan(scan_local(&dst, &up_dst, false, |_| {}).await.unwrap());
    let start = Instant::now();
    upload(s, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    let up = start.elapsed();
    assert_eq!(sha_remote(c.fixture, &up_dst), want);

    let scp_local = tmp.path().join("scp.bin");
    let scp_down = scp(c.fixture, &remote_at(&src), scp_local.to_str().unwrap()).await;
    assert_eq!(sha_local(&scp_local), want);
    let scp_up = scp(c.fixture, scp_local.to_str().unwrap(), &remote_at(&dir.join("scp-up.bin"))).await;

    println!("download 256 MiB: dromon {} | scp {}", rate(SIZE, down), rate(SIZE, scp_down));
    println!("upload   256 MiB: dromon {} | scp {}", rate(SIZE, up), rate(SIZE, scp_up));
    println!(
        "dromon / scp: download {:.0} %, upload {:.0} %",
        100.0 * scp_down.as_secs_f64() / down.as_secs_f64(),
        100.0 * scp_up.as_secs_f64() / up.as_secs_f64()
    );
    rm_rf(s, &dir).await;
}
