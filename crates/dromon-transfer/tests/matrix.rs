//! The server matrix for transfers (`.plans/dromon-plan.md` §9, M5): downloads, uploads,
//! same-host copies (`copy-data`, or streamed where the server has none), moves, symlink loops
//! in scans, and resuming after toxiproxy resets the connection mid-transfer, against every
//! server of `dromon_testkit::matrix::variants()` (with `DROMON_SERVER_MATRIX=1`: trixie,
//! bullseye, alpine and trixie behind toxiproxy; otherwise trixie only). Each check prints one
//! `matrix:` line per server and fails after all ran if any failed.
//!
//! ```sh
//! DROMON_SERVER_MATRIX=1 cargo test -p dromon-transfer --test matrix
//! ```

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use dromon_core::{EntryKind, Error, RemotePath};
use dromon_sftp::{Sftp, WriteMode};
use dromon_testkit::matrix::{self, Outcome};
use dromon_testkit::toxiproxy::Stream;
use dromon_testkit::{Feature, Fixture, Role, Toxiproxy, file_sha1, openssh};
use dromon_transfer::io::{
    CopyOptions, copy_remote, create_dirs, download, move_between, move_remote, scan_local, scan_remote,
    scan_remote_copy, upload,
};
use dromon_transfer::{DirPlan, FilePlan, Scan, is_part_file};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

const MIB: u64 = 1024 * 1024;
const MTIME: i64 = 1_600_000_000;
/// [`MTIME`] for `TZ=UTC0 touch -t` (no GNU `-d @…` in busybox or BSD touch).
const TOUCH_MTIME: &str = "202009131226.40";

struct Conn {
    sftp: Sftp,
    _ssh: Child,
}

async fn connect(fx: &Fixture) -> Conn {
    let mut ssh = Command::new(openssh("ssh"))
        .args(fx.ssh_options())
        .args(["-s", &fx.destination(Role::Key), "sftp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ssh");
    let stdout = ssh.stdout.take().unwrap();
    let stdin = ssh.stdin.take().unwrap();
    let sftp = Sftp::new(tokio::io::join(stdout, stdin)).await.expect("SFTP session");
    Conn { sftp, _ssh: ssh }
}

async fn scratch(fx: &Fixture, sftp: &Sftp, name: &str) -> RemotePath {
    let dir = fx.path(Role::Key, &format!("scratch/xmatrix-{name}-{}", std::process::id()));
    rm_rf(sftp, &dir).await;
    sftp.mkdir(&dir).await.unwrap();
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

/// `len` random bytes at `path` on the server, owned by the key user, with mtime [`MTIME`].
fn random_remote(fx: &Fixture, path: &RemotePath, len: u64) {
    let script = format!(
        "head -c {len} /dev/urandom >'{path}' && chown {} '{path}' && TZ=UTC0 touch -t {TOUCH_MTIME} '{path}'",
        fx.owner(Role::Key)
    );
    fx.exec(&["sh", "-c", &script]).unwrap();
}

fn sha(fx: &Fixture, path: &RemotePath) -> String {
    fx.sha1(path.as_str()).unwrap()
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed ^ (i >> 13) as u8)).collect()
}

fn mtime_local(path: &Path) -> i64 {
    let t = std::fs::metadata(path).unwrap().modified().unwrap();
    t.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn the_plan(scan: Scan) -> FilePlan {
    assert_eq!(scan.files.len(), 1, "{scan:?}");
    scan.files.into_iter().next().unwrap()
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

#[tokio::test(flavor = "multi_thread")]
async fn download_files_and_a_folder() {
    matrix::run("transfer download", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let dir = scratch(fx, s, "down").await;
        let src = dir.join("big.bin");
        random_remote(fx, &src, 3 * MIB + 5);
        let tmp = tempfile::tempdir().unwrap();
        let dst = tmp.path().join("big.bin");
        let plan = the_plan(scan_remote(s, &src, &dst, |_| {}).await.unwrap());
        let mut progress = Vec::new();
        assert_eq!(download(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap(), dst);
        assert_eq!(progress.last(), Some(&(3 * MIB + 5)));
        assert_eq!(file_sha1(&dst).unwrap(), sha(fx, &src));
        assert_eq!(mtime_local(&dst), MTIME);

        // The seed's mixed folder: what can be copied is, the rest is skipped with a reason.
        let mixed = fx.path(Role::Key, "fixture/mixed");
        let root = tmp.path().join("mixed");
        let scan = scan_remote(s, &mixed, &root, |_| {}).await.unwrap();
        assert_eq!(
            scan.skipped,
            [
                "mixed/dangling: a broken link",
                "mixed/link-dir: a link to a folder",
                "mixed/locked: permission denied",
                "mixed/pipe: a named pipe",
            ]
        );
        assert_eq!((scan.dirs.len(), scan.files.len()), (12, 86));
        create_dirs(s, &scan.dirs).await.unwrap();
        for plan in &scan.files {
            let FilePlan::Download { src, dst, mtime, .. } = plan else { unreachable!() };
            download(s, plan, CopyOptions::default(), |_| {}).await.unwrap();
            assert!(std::fs::read(dst).unwrap() == read_remote(s, src).await, "{src} differs");
            assert_eq!(Some(mtime_local(dst)), *mtime, "{src} mtime");
        }
        assert_eq!(std::fs::read(root.join("unicode-ğüş.txt")).unwrap(), b"unicode\n");
        rm_rf(s, &dir).await;
        Outcome::ok()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_a_tree_replace_and_keep_both() {
    matrix::run("transfer upload", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let dir = scratch(fx, s, "up").await;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("site");
        std::fs::create_dir_all(root.join("assets/img")).unwrap();
        std::fs::create_dir(root.join("empty")).unwrap();
        std::fs::write(root.join("index.html"), b"<h1>hi</h1>").unwrap();
        std::fs::write(root.join("assets/app.js"), pattern(700_001, 1)).unwrap();
        std::fs::write(root.join("assets/img/logo.png"), pattern(3 * MIB as usize + 17, 2)).unwrap();
        std::fs::write(root.join("assets/zero"), b"").unwrap();

        let remote_root = dir.join("site");
        let scan = scan_local(&root, &remote_root, false, |_| {}).await.unwrap();
        assert_eq!((scan.dirs.len(), scan.files.len()), (4, 4));
        create_dirs(s, &scan.dirs).await.unwrap();
        for plan in &scan.files {
            let FilePlan::Upload { src, dst, .. } = plan else { unreachable!() };
            assert_eq!(&upload(s, plan, CopyOptions::default(), |_| {}).await.unwrap(), dst);
            assert!(read_remote(s, dst).await == std::fs::read(src).unwrap(), "{dst} differs");
            assert_eq!(s.stat(dst).await.unwrap().mtime, Some(mtime_local(src)), "{dst} mtime");
        }
        assert!(s.list(&remote_root.join("empty")).await.unwrap().is_empty());
        assert!(remote_parts(s, &dir).await.is_empty());

        // Replace overwrites (posix-rename, or remove and rename without it); Keep both lands a
        // file that appeared meanwhile beside it.
        let src = tmp.path().join("report.pdf");
        std::fs::write(&src, b"new contents").unwrap();
        let dst = dir.join("report.pdf");
        write_remote(s, &dst, b"old").await;
        let plan = the_plan(scan_local(&src, &dst, true, |_| {}).await.unwrap());
        assert_eq!(upload(s, &plan, CopyOptions::default(), |_| {}).await.unwrap(), dst);
        assert_eq!(read_remote(s, &dst).await, b"new contents");
        let other = dir.join("notes.txt");
        let plan = the_plan(scan_local(&src, &other, false, |_| {}).await.unwrap());
        write_remote(s, &other, b"theirs").await;
        assert_eq!(upload(s, &plan, CopyOptions::default(), |_| {}).await.unwrap(), dir.join("notes (1).txt"));
        assert_eq!(read_remote(s, &other).await, b"theirs");
        rm_rf(s, &dir).await;
        Outcome::ok()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn same_host_copy_uses_copy_data_or_streams() {
    matrix::run("transfer remote copy", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let dir = scratch(fx, s, "copy").await;
        let src = dir.join("big.bin");
        random_remote(fx, &src, 3 * MIB);
        s.mkdir(&dir.join("out")).await.unwrap();
        let dst = dir.join("out/big.bin");
        let plan = the_plan(scan_remote_copy(s, &src, &dst, false, |_| {}).await.unwrap());

        let mut progress = Vec::new();
        let got = copy_remote(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
        assert_eq!((got.dst.clone(), got.size, got.mtime), (dst.clone(), 3 * MIB, Some(MTIME)));
        let how = if fx.has(Feature::CopyData) {
            assert_eq!(progress, [3 * MIB], "one copy-data request, no streaming");
            "copy-data"
        } else {
            assert!(progress.len() > 3, "streamed in chunks: {progress:?}");
            assert_eq!(progress.last(), Some(&(3 * MIB)));
            "streamed (no copy-data)"
        };
        assert_eq!(sha(fx, &dst), sha(fx, &src));
        assert_eq!(s.stat(&dst).await.unwrap().mtime, Some(MTIME));

        // Replace overwrites; Keep both lands beside.
        write_remote(s, &dst, b"old").await;
        let replace = FilePlan::Copy { src: src.clone(), dst: dst.clone(), size: 3 * MIB, mtime: Some(MTIME), overwrite: true };
        assert_eq!(copy_remote(s, &replace, CopyOptions::default(), |_| {}).await.unwrap().dst, dst);
        assert_eq!(sha(fx, &dst), sha(fx, &src));
        assert_eq!(copy_remote(s, &plan, CopyOptions::default(), |_| {}).await.unwrap().dst, dir.join("out/big (1).bin"));
        assert!(remote_parts(s, &dir).await.is_empty());
        rm_rf(s, &dir).await;
        Outcome::Ok(how.into())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn moves_rename_merge_and_verify() {
    matrix::run("transfer move", |fx| async move {
        let (a, b) = (connect(fx).await, connect(fx).await);
        let s = &a.sftp;
        let dir = scratch(fx, s, "move").await;
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        for d in [&src, &dst, &src.join("d"), &src.join("d/sub"), &dst.join("d")] {
            s.mkdir(d).await.unwrap();
        }
        write_remote(s, &src.join("d/a.txt"), b"new a").await;
        write_remote(s, &src.join("d/sub/b.txt"), b"b").await;
        write_remote(s, &dst.join("d/a.txt"), b"old a").await;
        write_remote(s, &dst.join("d/keep.txt"), b"keep").await;

        // Replace on a folder: merged, same-named files replaced, the rest kept.
        let plan = FilePlan::Move { src: src.join("d"), dst: dst.join("d"), size: 0, overwrite: true };
        assert_eq!(move_remote(s, &plan).await.unwrap(), dst.join("d"));
        assert_eq!(read_remote(s, &dst.join("d/a.txt")).await, b"new a");
        assert_eq!(read_remote(s, &dst.join("d/sub/b.txt")).await, b"b");
        assert_eq!(read_remote(s, &dst.join("d/keep.txt")).await, b"keep");
        assert!(!s.exists(&src.join("d")).await.unwrap());

        // Between two sessions ("hosts"): copied, checked, and only then the source deleted.
        let one = dir.join("one.bin");
        random_remote(fx, &one, MIB + 7);
        let want = sha(fx, &one);
        let to = dir.join("dst/one.bin");
        let plan = the_plan(scan_remote_copy(s, &one, &to, false, |_| {}).await.unwrap());
        let got = move_between(s, &b.sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap();
        assert_eq!((got.dst, got.size), (to.clone(), MIB + 7));
        assert_eq!(sha(fx, &to), want);
        assert!(!s.exists(&one).await.unwrap(), "the source is deleted");
        rm_rf(s, &dir).await;
        Outcome::ok()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn scans_skip_symlink_loops() {
    matrix::run("transfer symlink loops", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let dir = scratch(fx, s, "loops").await;
        let loops = dir.join("loops");
        s.mkdir(&loops).await.unwrap();
        s.mkdir(&loops.join("inner")).await.unwrap();
        write_remote(s, &loops.join("file.txt"), b"file").await;
        // `self -> .`, `inner/parent -> ..` (its own parent), `inner/top -> ../..` (out of the
        // tree), and the cycle `a -> b -> a`.
        let script =
            format!("cd '{loops}' && ln -s . self && ln -s b a && ln -s a b && ln -s .. inner/parent && ln -s ../.. inner/top");
        fx.exec(&["sh", "-c", &script]).unwrap();
        let want_skipped = [
            "loops/a: a broken link",
            "loops/b: a broken link",
            "loops/inner/parent: a link to a folder",
            "loops/inner/top: a link to a folder",
            "loops/self: a link to a folder",
        ];
        let tmp = tempfile::tempdir().unwrap();
        let check = async {
            // A download: only the file and the two folders, the links skipped with reasons.
            let root = tmp.path().join("loops");
            let scan = scan_remote(s, &loops, &root, |_| {}).await.unwrap();
            assert_eq!(scan.skipped, want_skipped);
            assert_eq!(scan.dirs, [DirPlan::Local(root.clone()), DirPlan::Local(root.join("inner"))]);
            create_dirs(s, &scan.dirs).await.unwrap();
            download(s, &the_plan(scan), CopyOptions::default(), |_| {}).await.unwrap();
            assert_eq!(std::fs::read(root.join("file.txt")).unwrap(), b"file");

            // A copy on the server: the same plan, and it lands whole.
            let copy = dir.join("copy");
            let scan = scan_remote_copy(s, &loops, &copy, false, |_| {}).await.unwrap();
            assert_eq!(scan.skipped, want_skipped);
            create_dirs(s, &scan.dirs).await.unwrap();
            copy_remote(s, &the_plan(scan), CopyOptions::default(), |_| {}).await.unwrap();
            let mut names: Vec<String> = s.list(&copy).await.unwrap().into_iter().map(|e| e.name).collect();
            names.sort();
            assert_eq!(names, ["file.txt", "inner"]);
            assert!(s.list(&copy.join("inner")).await.unwrap().is_empty());
        };
        tokio::time::timeout(Duration::from_secs(120), check).await.expect("a link loop was followed (timed out)");
        rm_rf(s, &dir).await;
        Outcome::ok()
    })
    .await;
}

/// The resume check's toxics on top of the base ones: `(name, type, stream, attributes)`.
const TEST_TOXICS: [(&str, &str, Stream, &str); 3] = [
    ("test_bandwidth_down", "bandwidth", Stream::Downstream, r#"{"rate":1024}"#),
    ("test_bandwidth_up", "bandwidth", Stream::Upstream, r#"{"rate":1024}"#),
    ("test_latency_up", "latency", Stream::Upstream, r#"{"latency":30,"jitter":10}"#),
];

/// Removes [`TEST_TOXICS`] and any reset when dropped (errors ignored: some may not be there).
struct TestToxics<'a>(&'a Toxiproxy);

impl Drop for TestToxics<'_> {
    fn drop(&mut self) {
        let _ = self.0.stop_resets();
        for (name, ..) in TEST_TOXICS {
            let _ = self.0.remove(name);
        }
    }
}

/// Runs `transfer` (a download or upload) in its own task, resets every connection
/// through toxiproxy once it reported `at` bytes, and returns its result.
async fn reset_midway<F>(fx: &Fixture, at: u64, transfer: impl FnOnce(mpsc::UnboundedSender<u64>) -> F) -> dromon_core::Result<()>
where
    F: Future<Output = dromon_core::Result<()>> + Send + 'static,
{
    let toxi = fx.toxiproxy().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(transfer(tx));
    loop {
        match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.expect("no progress for 60 s") {
            Some(n) if n >= at => break,
            Some(_) => {}
            None => panic!("the transfer ended before {at} bytes"),
        }
    }
    toxi.reset_peers().unwrap();
    let res = tokio::time::timeout(Duration::from_secs(60), task).await.expect("still running 60 s after the reset").unwrap();
    toxi.stop_resets().unwrap();
    res
}

#[tokio::test(flavor = "multi_thread")]
async fn transfers_resume_after_a_connection_reset() {
    // Exclusive: the reset ends every connection through the proxy.
    matrix::run_exclusive("transfer resume after a reset", |fx| async move {
        if let Some(skip) = Outcome::unless(fx, Feature::Toxics) {
            return skip;
        }
        const SIZE: u64 = 8 * MIB;
        let toxi = fx.toxiproxy().unwrap();
        // Slower than the base 4 MB/s so the reset lands mid-way, plus more latency; removed
        // again at the end, also when an assertion fails.
        let _toxics = TestToxics(toxi);
        for (name, kind, stream, attributes) in TEST_TOXICS {
            toxi.add(name, kind, stream, attributes).unwrap();
        }
        let c = connect(fx).await;
        let dir = scratch(fx, &c.sftp, "reset").await;

        // A download cut at 2 MiB keeps its local part and resumes from it on a new connection.
        let src = dir.join("down.bin");
        random_remote(fx, &src, SIZE);
        let tmp = tempfile::tempdir().unwrap();
        let dst = tmp.path().join("down.bin");
        let part = tmp.path().join("down.bin.dromon-part");
        let plan = the_plan(scan_remote(&c.sftp, &src, &dst, |_| {}).await.unwrap());
        let (sftp, task_plan) = (c.sftp.clone(), plan.clone());
        let cut = reset_midway(fx, 2 * MIB, move |tx| async move {
            download(&sftp, &task_plan, CopyOptions::default(), move |n| {
                let _ = tx.send(n);
            })
            .await
            .map(drop)
        })
        .await;
        assert_eq!(cut, Err(Error::ConnectionLost), "a reset is a lost connection");
        drop(c);
        assert!(!dst.exists(), "nothing under the final name");
        let kept = std::fs::metadata(&part).expect("the part stays").len();
        assert!((2 * MIB..SIZE).contains(&kept), "{kept}");
        let c = connect(fx).await;
        let mut progress = Vec::new();
        download(&c.sftp, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
        assert_eq!(progress[0], kept, "resumed where the reset cut it");
        assert_eq!(file_sha1(&dst).unwrap(), sha(fx, &src), "the exact bytes");
        assert!(!part.exists());

        // An upload cut at 2 MiB resumes from the server's part.
        let local = tmp.path().join("up.bin");
        std::fs::write(&local, pattern(SIZE as usize, 7)).unwrap();
        let up = dir.join("up.bin");
        let up_part = dir.join(".up.bin.dromon-part");
        let plan = the_plan(scan_local(&local, &up, false, |_| {}).await.unwrap());
        let (sftp, task_plan) = (c.sftp.clone(), plan.clone());
        let cut = reset_midway(fx, 2 * MIB, move |tx| async move {
            upload(&sftp, &task_plan, CopyOptions::default(), move |n| {
                let _ = tx.send(n);
            })
            .await
            .map(drop)
        })
        .await;
        assert_eq!(cut, Err(Error::ConnectionLost), "a reset is a lost connection");
        drop(c);
        let c = connect(fx).await;
        assert!(!c.sftp.exists(&up).await.unwrap(), "nothing under the final name");
        let kept_up = c.sftp.stat(&up_part).await.expect("the part stays").size;
        assert!((2 * MIB..SIZE).contains(&kept_up), "{kept_up}");
        let mut progress = Vec::new();
        upload(&c.sftp, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
        assert_eq!(progress[0], kept_up, "resumed from the server's part");
        assert_eq!(sha(fx, &up), file_sha1(&local).unwrap(), "the exact bytes");

        // And listings work on the new connection.
        assert_eq!(c.sftp.list(&fx.path(Role::Key, "fixture/mixed")).await.unwrap().len(), 100);
        rm_rf(&c.sftp, &dir).await;
        Outcome::Ok(format!("download resumed at {kept} B, upload at {kept_up} B of {SIZE}"))
    })
    .await;
}
