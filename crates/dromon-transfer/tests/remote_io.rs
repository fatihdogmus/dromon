//! `dromon_transfer::io`'s remote-to-remote paths against the sshd fixture (`dromon-testkit`):
//! same-host copies (server-side `copy-data`, and streamed when told to), copies and moves
//! between two hosts, moves with a merge on Replace, and the tidying after a move. The "two
//! hosts" are two SFTP sessions (two `ssh` children) to the same fixture as the key user; each
//! test uses its own scratch folder in that user's home. When the backend can't run each test
//! prints a skip line and passes (fails with `DROMON_REQUIRE_DOCKER=1`).

use std::time::Duration;

use dromon_core::{EntryKind, Error, RemotePath};
use dromon_sftp::{Sftp, WriteMode};
use dromon_testkit::{Fixture, Role, openssh};
use dromon_transfer::io::{
    Copied, CopyOptions, copy_between, copy_remote, create_dirs, delete_verified, move_between, move_remote,
    remove_empty_dirs, remove_part, scan_remote_copy,
};
use dromon_transfer::{DirPlan, FilePlan, Scan, is_part_file};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

const MIB: u64 = 1024 * 1024;
const MTIME: i64 = 1_600_000_000;
/// [`MTIME`] for `TZ=UTC0 touch -t` (GNU's `-d @…` isn't in macOS's BSD touch).
const TOUCH_MTIME: &str = "202009131226.40";

/// One SFTP session to the fixture, standing in for a host.
struct Conn {
    sftp: Sftp,
    _ssh: Child,
}

async fn session(fixture: &Fixture) -> Conn {
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
    Conn { sftp, _ssh: ssh }
}

/// The fixture and two sessions to it ("host a" and "host b").
async fn two_hosts(test: &str) -> Option<(&'static Fixture, Conn, Conn)> {
    let fixture = dromon_testkit::fixture(test).await?;
    Some((fixture, session(fixture).await, session(fixture).await))
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

async fn scratch(fixture: &Fixture, sftp: &Sftp, name: &str) -> RemotePath {
    let dir = fixture.path(Role::Key, &format!("scratch/remote-{name}-{}", std::process::id()));
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

/// `len` random bytes at `path`, owned by the key user, with mtime [`MTIME`].
fn random_remote(fixture: &Fixture, path: &RemotePath, len: u64) {
    let script =
        format!("head -c {len} /dev/urandom >'{path}' && chown {} '{path}' && TZ=UTC0 touch -m -t {TOUCH_MTIME} '{path}'", fixture.owner(Role::Key));
    fixture.exec(&["sh", "-c", &script]).unwrap();
}

fn sha(fixture: &Fixture, path: &RemotePath) -> String {
    fixture.sha1(path.as_str()).unwrap()
}

async fn mtime(sftp: &Sftp, path: &RemotePath) -> Option<i64> {
    sftp.stat(path).await.unwrap().mtime
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

fn streamed() -> CopyOptions {
    CopyOptions { server_copy: false, ..CopyOptions::default() }
}

/// The engine runs these in spawned tasks, so their futures must be `Send` (checked at compile
/// time; never called).
#[allow(dead_code)]
fn futures_are_send(a: &Sftp, b: &Sftp, plan: &FilePlan, copied: &Copied) {
    fn send<T: Send>(_: T) {}
    send(scan_remote_copy(a, &p("/"), &p("/x"), false, |_| {}));
    send(copy_remote(a, plan, CopyOptions::default(), |_| {}));
    send(copy_between(a, b, plan, CopyOptions::default(), |_| {}));
    send(move_between(a, b, plan, CopyOptions::default(), |_| {}));
    send(delete_verified(a, b, &p("/x"), copied));
    send(move_remote(a, plan));
    send(remove_empty_dirs(a, &p("/x")));
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
async fn same_host_copy_uses_copy_data_and_keeps_bytes_and_mtime() {
    let Some((fx, a, _b)) = two_hosts("same_host_copy_uses_copy_data_and_keeps_bytes_and_mtime").await else { return };
    let s = &a.sftp;
    assert!(s.capabilities().copy_data, "the fixture's OpenSSH offers copy-data");
    let dir = scratch(fx, s, "copy-data").await;
    let src = dir.join("big.bin");
    random_remote(fx, &src, 3 * MIB);
    s.mkdir(&dir.join("out")).await.unwrap();
    let dst = dir.join("out/big.bin");
    let plan = the_plan(scan_remote_copy(s, &src, &dst, false, |_| {}).await.unwrap());
    assert_eq!(plan, FilePlan::Copy { src: src.clone(), dst: dst.clone(), size: 3 * MIB, mtime: Some(MTIME), overwrite: false });

    let mut progress = Vec::new();
    let got = copy_remote(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert_eq!(got, Copied { dst: dst.clone(), size: 3 * MIB, mtime: Some(MTIME) });
    assert_eq!(progress, [3 * MIB], "one copy-data request, no streaming");
    assert_eq!(sha(fx, &dst), sha(fx, &src));
    assert_eq!(mtime(s, &dst).await, Some(MTIME));

    // Again with a file in the way: Keep both lands it beside; Replace overwrites.
    let got = copy_remote(s, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert_eq!(got.dst, dir.join("out/big (1).bin"));
    write_remote(s, &dst, b"old").await;
    let replace = FilePlan::Copy { src: src.clone(), dst: dst.clone(), size: 3 * MIB, mtime: Some(MTIME), overwrite: true };
    assert_eq!(copy_remote(s, &replace, CopyOptions::default(), |_| {}).await.unwrap().dst, dst);
    assert_eq!(sha(fx, &dst), sha(fx, &src));
    assert!(remote_parts(s, &dir).await.is_empty());

    // An empty file, and the wrong kind of plan.
    let empty = dir.join("empty");
    write_remote(s, &empty, b"").await;
    let plan = the_plan(scan_remote_copy(s, &empty, &dir.join("out/empty"), false, |_| {}).await.unwrap());
    assert_eq!(copy_remote(s, &plan, CopyOptions::default(), |_| {}).await.unwrap().size, 0);
    let wrong = FilePlan::Move { src: src.clone(), dst: dst.clone(), size: 1, overwrite: false };
    assert!(matches!(copy_remote(s, &wrong, CopyOptions::default(), |_| {}).await, Err(Error::Other(_))));
    assert!(matches!(move_remote(s, &plan).await, Err(Error::Other(_))));
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn same_host_copy_streams_when_told_and_resumes_from_the_part() {
    let Some((fx, a, _b)) = two_hosts("same_host_copy_streams_when_told_and_resumes_from_the_part").await else { return };
    let s = &a.sftp;
    let dir = scratch(fx, s, "stream").await;
    let src = dir.join("data.bin");
    random_remote(fx, &src, 5 * MIB + 123);
    let data = read_remote(s, &src).await;
    let dst = dir.join("copy.bin");
    let plan = the_plan(scan_remote_copy(s, &src, &dst, false, |_| {}).await.unwrap());

    let mut progress = Vec::new();
    copy_remote(s, &plan, streamed(), |n| progress.push(n)).await.unwrap();
    assert!(progress.len() > 3, "streamed in chunks: {progress:?}");
    assert!(progress.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(progress.last(), Some(&(5 * MIB + 123)));
    assert!(read_remote(s, &dst).await == data);
    assert_eq!(mtime(s, &dst).await, Some(MTIME));

    // A part the server already has is continued.
    rm_rf(s, &dst).await;
    let part = dir.join(".copy.bin.dromon-part");
    write_remote(s, &part, &data[..2 * MIB as usize]).await;
    let mut progress = Vec::new();
    copy_remote(s, &plan, streamed(), |n| progress.push(n)).await.unwrap();
    assert_eq!(progress.first(), Some(&(2 * MIB)), "resumed at the part's length");
    assert!(read_remote(s, &dst).await == data);
    assert!(!s.exists(&part).await.unwrap());

    // copy-data never resumes: a part is overwritten from the start.
    rm_rf(s, &dst).await;
    write_remote(s, &part, &vec![0xEE; 2 * MIB as usize]).await;
    copy_remote(s, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert!(read_remote(s, &dst).await == data);
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_copy_scans_like_a_download_and_lands_whole() {
    let Some((fx, a, b)) = two_hosts("a_folder_copy_scans_like_a_download_and_lands_whole").await else { return };
    let mixed = fx.path(Role::Key, "fixture/mixed");
    let dir = scratch(fx, &b.sftp, "tree").await;
    let root = dir.join("mixed");
    let mut counts = Vec::new();
    let scan = scan_remote_copy(&a.sftp, &mixed, &root, true, |n| counts.push(n)).await.unwrap();
    assert_eq!(
        scan.skipped,
        [
            "mixed/dangling: a broken link",
            "mixed/link-dir: a link to a folder",
            "mixed/locked: permission denied",
            "mixed/pipe: a named pipe",
        ]
    );
    assert_eq!(counts, [64, 86]);
    assert_eq!(scan.dirs[..2], [DirPlan::Remote(root.clone()), DirPlan::Remote(root.join(".config"))]);
    assert!(scan.files.iter().all(|f| matches!(f, FilePlan::Copy { overwrite: true, .. })));

    // Between the two hosts: folders first, then every file streamed.
    create_dirs(&b.sftp, &scan.dirs).await.unwrap();
    for plan in &scan.files {
        let FilePlan::Copy { src, dst, mtime: want, .. } = plan else { unreachable!() };
        let got = copy_between(&a.sftp, &b.sftp, plan, CopyOptions::default(), |_| {}).await.unwrap();
        assert_eq!(&got.dst, dst);
        assert!(read_remote(&b.sftp, dst).await == read_remote(&a.sftp, src).await, "{dst} differs");
        assert_eq!(mtime(&b.sftp, dst).await, *want, "{dst} mtime");
    }
    assert_eq!(b.sftp.list(&root.join("dir-10")).await.unwrap(), []);
    assert_eq!(read_remote(&b.sftp, &root.join("link-file")).await, vec![0u8; 100]);
    assert!(remote_parts(&b.sftp, &dir).await.is_empty());
    rm_rf(&b.sftp, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cross_host_copy_resumes_after_a_cancel() {
    let Some((fx, a, b)) = two_hosts("a_cross_host_copy_resumes_after_a_cancel").await else { return };
    let dir = scratch(fx, &a.sftp, "cross-resume").await;
    let src = dir.join("big.bin");
    random_remote(fx, &src, 32 * MIB);
    b.sftp.mkdir(&dir.join("b")).await.unwrap();
    let dst = dir.join("b/big.bin");
    let part = dir.join("b/.big.bin.dromon-part");
    let plan = the_plan(scan_remote_copy(&a.sftp, &src, &dst, false, |_| {}).await.unwrap());

    let (from, to, task_plan) = (a.sftp.clone(), b.sftp.clone(), plan.clone());
    let seen = abort_after_first_progress(move |tx| async move {
        let _ = copy_between(&from, &to, &task_plan, CopyOptions::default(), move |n| {
            let _ = tx.send(n);
        })
        .await;
    })
    .await;
    assert!(seen.iter().all(|&n| n < 32 * MIB));
    // Let the writes that were in flight land.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!b.sftp.exists(&dst).await.unwrap(), "nothing under the final name");
    let kept = b.sftp.stat(&part).await.expect("the part stays").size;
    assert!(kept > 0 && kept < 32 * MIB, "{kept}");

    let mut progress = Vec::new();
    let got = copy_between(&a.sftp, &b.sftp, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert_eq!(got.dst, dst);
    assert_eq!(progress[0], kept, "resumed where the cancelled copy stopped");
    assert_eq!(sha(fx, &dst), sha(fx, &src));
    assert_eq!(mtime(&b.sftp, &dst).await, Some(MTIME));
    assert!(!b.sftp.exists(&part).await.unwrap());

    // Cancelled for good: the part goes.
    write_remote(&b.sftp, &part, b"partial").await;
    remove_part(&b.sftp, &plan).await;
    assert!(!b.sftp.exists(&part).await.unwrap());
    rm_rf(&a.sftp, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cross_host_move_deletes_only_verified_sources() {
    let Some((fx, a, b)) = two_hosts("a_cross_host_move_deletes_only_verified_sources").await else { return };
    let dir = scratch(fx, &a.sftp, "cross-move").await;
    b.sftp.mkdir(&dir.join("b")).await.unwrap();

    // A whole move: copied, checked, source gone.
    let src = dir.join("one.bin");
    random_remote(fx, &src, MIB + 7);
    let want = sha(fx, &src);
    let dst = dir.join("b/one.bin");
    let plan = the_plan(scan_remote_copy(&a.sftp, &src, &dst, false, |_| {}).await.unwrap());
    let got = move_between(&a.sftp, &b.sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert_eq!((got.dst, got.size), (dst.clone(), MIB + 7));
    assert_eq!(sha(fx, &dst), want);
    assert!(!a.sftp.exists(&src).await.unwrap(), "the source is deleted");

    // A copy whose size doesn't match: the source stays.
    let src = dir.join("two.txt");
    write_remote(&a.sftp, &src, b"two").await;
    let copied = Copied { dst: dst.clone(), size: 3, mtime: mtime(&a.sftp, &src).await };
    let err = delete_verified(&a.sftp, &b.sftp, &src, &copied).await.unwrap_err();
    assert_eq!(err.to_string(), format!("The copy of two.txt has {} bytes instead of 3; the original was kept", MIB + 7));
    assert!(a.sftp.exists(&src).await.unwrap());

    // A source that changed after it was copied: it stays.
    let dst2 = dir.join("b/two.txt");
    let plan = FilePlan::Copy { src: src.clone(), dst: dst2.clone(), size: 3, mtime: None, overwrite: false };
    let copied = copy_between(&a.sftp, &b.sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    write_remote(&a.sftp, &src, b"two, longer now").await;
    let err = delete_verified(&a.sftp, &b.sftp, &src, &copied).await.unwrap_err();
    assert_eq!(err.to_string(), "two.txt changed on the server while moving; the original was kept");
    assert_eq!(read_remote(&a.sftp, &src).await, b"two, longer now");

    // A move dropped mid-copy never touches its source.
    let big = dir.join("big.bin");
    random_remote(fx, &big, 32 * MIB);
    let plan = FilePlan::Copy { src: big.clone(), dst: dir.join("b/big.bin"), size: 32 * MIB, mtime: Some(MTIME), overwrite: false };
    let (from, to, task_plan) = (a.sftp.clone(), b.sftp.clone(), plan.clone());
    abort_after_first_progress(move |tx| async move {
        let _ = move_between(&from, &to, &task_plan, CopyOptions::default(), move |n| {
            let _ = tx.send(n);
        })
        .await;
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.sftp.stat(&big).await.unwrap().size, 32 * MIB, "the source stays whole");
    assert!(!b.sftp.exists(&dir.join("b/big.bin")).await.unwrap());
    // Run again: it resumes, and only then is the source deleted.
    let mut progress = Vec::new();
    move_between(&a.sftp, &b.sftp, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    assert!(progress[0] > 0, "resumed");
    assert!(!a.sftp.exists(&big).await.unwrap());
    rm_rf(&a.sftp, &dir).await;
}

/// The engine aborts a running file when its host goes down, even one that had just finished,
/// and runs it again: a move run twice must neither fail on its missing source nor leave a
/// second copy.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_run_again_after_it_finished_is_done_without_a_second_copy() {
    let Some((fx, a, b)) = two_hosts("a_move_run_again_after_it_finished_is_done_without_a_second_copy").await else { return };
    let dir = scratch(fx, &a.sftp, "move-again").await;
    b.sftp.mkdir(&dir.join("b")).await.unwrap();
    let listing = |s: &Sftp, d: RemotePath| {
        let s = s.clone();
        async move {
            let mut n: Vec<String> = s.list(&d).await.unwrap().into_iter().map(|e| e.name).collect();
            n.sort();
            n
        }
    };

    // Between hosts, finished whole: run again, it's done (no error, no "x (1).bin").
    let src = dir.join("x.bin");
    random_remote(fx, &src, 300_007);
    let want = sha(fx, &src);
    let dst = dir.join("b/x.bin");
    let plan = the_plan(scan_remote_copy(&a.sftp, &src, &dst, false, |_| {}).await.unwrap());
    move_between(&a.sftp, &b.sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    let again = move_between(&a.sftp, &b.sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert_eq!((again.dst, again.size), (dst.clone(), 300_007));
    assert_eq!(listing(&b.sftp, dir.join("b")).await, ["x.bin"]);
    assert_eq!(sha(fx, &dst), want);

    // Between hosts, copied but not yet deleted: run again, the source goes, still one copy.
    let src = dir.join("y.bin");
    random_remote(fx, &src, 4097);
    let dst = dir.join("b/y.bin");
    let plan = the_plan(scan_remote_copy(&a.sftp, &src, &dst, false, |_| {}).await.unwrap());
    copy_between(&a.sftp, &b.sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    move_between(&a.sftp, &b.sftp, &plan, CopyOptions::default(), |_| {}).await.unwrap();
    assert!(!a.sftp.exists(&src).await.unwrap(), "the verified source is deleted");
    assert_eq!(listing(&b.sftp, dir.join("b")).await, ["x.bin", "y.bin"]);

    // On one host, renamed already: run again, it's done.
    let (src, dst) = (dir.join("z.txt"), dir.join("b/z.txt"));
    write_remote(&a.sftp, &src, b"zed").await;
    let plan = FilePlan::Move { src: src.clone(), dst: dst.clone(), size: 3, overwrite: false };
    assert_eq!(move_remote(&a.sftp, &plan).await.unwrap(), dst);
    assert_eq!(move_remote(&a.sftp, &plan).await.unwrap(), dst, "run again: done, not an error");
    assert_eq!(read_remote(&a.sftp, &dst).await, b"zed");

    // A missing source with nothing at the destination is still an error.
    let gone = FilePlan::Move { src: dir.join("never.txt"), dst: dir.join("b/never.txt"), size: 1, overwrite: false };
    assert!(move_remote(&a.sftp, &gone).await.is_err());
    rm_rf(&a.sftp, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tidying_removes_only_empty_source_folders() {
    let Some((fx, a, _b)) = two_hosts("tidying_removes_only_empty_source_folders").await else { return };
    let s = &a.sftp;
    let dir = scratch(fx, s, "tidy").await;
    let root = dir.join("moved");
    for d in ["", "/empty", "/deep", "/deep/er", "/deep/er/est", "/kept", "/kept/inner"] {
        s.mkdir(&p(&format!("{root}{d}"))).await.unwrap();
    }
    write_remote(s, &root.join("kept/inner/left.txt"), b"left behind").await;
    remove_empty_dirs(s, &root).await.unwrap();
    let names: Vec<String> = s.list(&root).await.unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["kept"]);
    assert_eq!(read_remote(s, &root.join("kept/inner/left.txt")).await, b"left behind");

    s.remove_file(&root.join("kept/inner/left.txt")).await.unwrap();
    remove_empty_dirs(s, &root).await.unwrap();
    assert!(!s.exists(&root).await.unwrap(), "all empty now: the root goes too");
    remove_empty_dirs(s, &root).await.unwrap(); // gone already: fine
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_renames_and_replace_merges_folders() {
    let Some((fx, a, _b)) = two_hosts("a_move_renames_and_replace_merges_folders").await else { return };
    let s = &a.sftp;
    let dir = scratch(fx, s, "move").await;
    let (src, dst) = (dir.join("src"), dir.join("dst"));
    for d in [&src, &dst, &src.join("d"), &src.join("d/sub"), &src.join("d/new"), &dst.join("d"), &dst.join("d/sub")] {
        s.mkdir(d).await.unwrap();
    }
    write_remote(s, &src.join("d/a.txt"), b"new a").await;
    s.set_mtime(&src.join("d/a.txt"), MTIME).await.unwrap();
    write_remote(s, &src.join("d/sub/b.txt"), b"b").await;
    write_remote(s, &src.join("d/new/c.txt"), b"c").await;
    write_remote(s, &dst.join("d/a.txt"), b"old a").await;
    write_remote(s, &dst.join("d/sub/other.txt"), b"other").await;
    write_remote(s, &dst.join("d/keep.txt"), b"keep").await;

    // Replace on a folder: merged, same-named files replaced, the rest kept; the source goes.
    let plan = FilePlan::Move { src: src.join("d"), dst: dst.join("d"), size: 0, overwrite: true };
    assert_eq!(move_remote(s, &plan).await.unwrap(), dst.join("d"));
    assert_eq!(read_remote(s, &dst.join("d/a.txt")).await, b"new a");
    assert_eq!(mtime(s, &dst.join("d/a.txt")).await, Some(MTIME), "a rename keeps the mtime");
    for (f, want) in [("d/sub/b.txt", "b"), ("d/sub/other.txt", "other"), ("d/new/c.txt", "c"), ("d/keep.txt", "keep")] {
        assert_eq!(read_remote(s, &dst.join(f)).await, want.as_bytes(), "{f}");
    }
    assert!(!s.exists(&src.join("d")).await.unwrap());

    // A plain move to a free name; one that was taken since lands beside it.
    write_remote(s, &src.join("f.txt"), b"f").await;
    let plan = FilePlan::Move { src: src.join("f.txt"), dst: dst.join("f.txt"), size: 1, overwrite: false };
    assert_eq!(move_remote(s, &plan).await.unwrap(), dst.join("f.txt"));
    write_remote(s, &src.join("f.txt"), b"f2").await;
    assert_eq!(move_remote(s, &plan).await.unwrap(), dst.join("f (1).txt"));
    assert_eq!(read_remote(s, &dst.join("f.txt")).await, b"f");

    // Replace on a file replaces it; a file and a folder never replace each other.
    write_remote(s, &src.join("f.txt"), b"f3").await;
    let plan = FilePlan::Move { src: src.join("f.txt"), dst: dst.join("f.txt"), size: 2, overwrite: true };
    move_remote(s, &plan).await.unwrap();
    assert_eq!(read_remote(s, &dst.join("f.txt")).await, b"f3");
    write_remote(s, &src.join("g"), b"g").await;
    let plan = FilePlan::Move { src: src.join("g"), dst: dst.join("d"), size: 1, overwrite: true };
    assert_eq!(move_remote(s, &plan).await, Err(Error::AlreadyExists(dst.join("d"))));
    assert!(s.exists(&src.join("g")).await.unwrap());
    rm_rf(s, &dir).await;
}
