//! Integration tests against the sshd fixture (`dromon-testkit`, one server for this test
//! binary: the Docker image of `tests/docker/`, or the native sshd). They run whenever the
//! backend can; otherwise each test prints a skip line and passes (fails with
//! `DROMON_REQUIRE_DOCKER=1`). All as the key user, in its home (`Fixture::home`).
//! Each test opens its own `ssh -s … sftp` child with the ssh the app runs (`ssh_program()`, on
//! Windows the inbox OpenSSH). The 1 GiB S4 test is `#[ignore]`:
//! `cargo test -p dromon-sftp -- --ignored`.

use std::time::{Duration, Instant};

use dromon_core::{Entry, EntryKind, Error, LinkTarget, RemotePath};
use dromon_sftp::{Sftp, WriteMode};
use dromon_testkit::{BIG_FILE_SIZE as BIG_SIZE, Fixture, Role};
use tokio::process::{Child, Command};

struct Conn {
    sftp: Sftp,
    fixture: &'static Fixture,
    _ssh: Child,
}

impl Conn {
    /// The key user's home (`/home/key` in Docker).
    fn home(&self) -> String {
        self.fixture.home(Role::Key).as_str().to_owned()
    }

    /// The seed's mixed folder.
    fn mixed(&self) -> String {
        self.fixture.path(Role::Key, "fixture/mixed").as_str().to_owned()
    }

    /// The seed's 10,000 files.
    fn many(&self) -> String {
        self.fixture.path(Role::Key, "fixture/many").as_str().to_owned()
    }
}

async fn connect(test: &str) -> Option<Conn> {
    let fixture = dromon_testkit::fixture(test).await?;
    let mut ssh = Command::new(dromon_ssh::ssh_program())
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
    // The SFTP calls, not the reply timeout, are under test: a loaded Windows CI (WSL port
    // forwarding) can take longer than the default 10 s for one reply.
    sftp.set_timeout(60);
    Some(Conn { sftp, fixture, _ssh: ssh })
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// A fresh, empty scratch folder for one test.
async fn scratch(c: &Conn, name: &str) -> RemotePath {
    let dir = p(&format!("{}/scratch/{name}-{}", c.home(), std::process::id()));
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

fn find<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
    entries.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("{name} not listed"))
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

/// Writes `data` with every chunk in flight at once, then waits for the acks.
async fn write_pipelined(sftp: &Sftp, path: &RemotePath, mode: WriteMode, offset: u64, data: &[u8]) {
    let file = sftp.open_write(path, mode).await.unwrap();
    let chunk = sftp.chunk_size() as usize;
    let acks: Vec<_> =
        data.chunks(chunk).enumerate().map(|(i, c)| file.write_at(offset + (i * chunk) as u64, c)).collect();
    for ack in acks {
        ack.await.unwrap();
    }
    file.close().await.unwrap();
}

/// Reads `len` bytes at `offset` with every chunk requested up front.
async fn read_pipelined(sftp: &Sftp, path: &RemotePath, offset: u64, len: u64) -> Vec<u8> {
    let file = sftp.open_read(path).await.unwrap();
    let chunk = u64::from(sftp.chunk_size());
    let reads: Vec<_> =
        (0..len.div_ceil(chunk)).map(|i| file.read_at(offset + i * chunk, chunk.min(len - i * chunk) as u32)).collect();
    let mut out = Vec::new();
    for read in reads {
        out.extend(read.await.unwrap());
    }
    file.close().await.unwrap();
    out
}

fn rate(bytes: u64, took: Duration) -> String {
    format!("{:.2} s, {:.0} MiB/s", took.as_secs_f64(), bytes as f64 / 1048576.0 / took.as_secs_f64())
}

#[tokio::test(flavor = "multi_thread")]
async fn home_and_capabilities() {
    let Some(c) = connect("home_and_capabilities").await else { return };
    assert_eq!(c.sftp.home().await.unwrap(), p(&c.home()));
    let caps = c.sftp.capabilities();
    println!("extensions: {:?}", caps.extensions);
    println!("limits: {:?}, chunk size {}", caps.limits, c.sftp.chunk_size());
    assert!(caps.posix_rename, "posix-rename@openssh.com");
    assert!(caps.copy_data, "copy-data");
    let limits = caps.limits.expect("limits@openssh.com");
    assert!(limits.max_read > 0 && limits.max_write > 0);
    assert_eq!(u64::from(c.sftp.chunk_size()), limits.max_read.min(limits.max_write));
}

#[tokio::test(flavor = "multi_thread")]
async fn list_mixed_folder() {
    let Some(c) = connect("list_mixed_folder").await else { return };
    let s = &c.sftp;
    let mixed = c.mixed();
    let entries = s.list(&p(&mixed)).await.unwrap();
    assert_eq!(entries.len(), 100, "80 files, 10 folders and 10 specials");
    assert!(entries.iter().all(|e| e.name != "." && e.name != ".."));

    let f = find(&entries, "file-001.txt");
    assert_eq!((f.kind, f.size, f.mtime), (EntryKind::File, 100, Some(1_577_934_245)));
    assert_eq!(f.mode.map(|m| m & 0o777), Some(0o644));
    assert_eq!(find(&entries, "file-080.txt").size, 8000);
    assert_eq!(find(&entries, "dir-01").kind, EntryKind::Dir);
    assert_eq!(find(&entries, "link-dir").kind, EntryKind::Symlink { target: Some(LinkTarget::Dir) });
    assert!(find(&entries, "link-dir").kind.is_dir_like());
    assert_eq!(find(&entries, "link-file").kind, EntryKind::Symlink { target: Some(LinkTarget::File) });
    assert_eq!(find(&entries, "dangling").kind, EntryKind::Symlink { target: None });
    assert_eq!(find(&entries, "pipe").kind, EntryKind::Other);
    assert_eq!(find(&entries, "locked").kind, EntryKind::Dir);
    assert!(find(&entries, ".hidden").hidden());
    assert_eq!(find(&entries, ".config").kind, EntryKind::Dir);
    assert_eq!(find(&entries, "unicode-ğüş.txt").size, 8);
    assert_eq!(find(&entries, "space name.txt").size, 7);

    let locked = p(&format!("{mixed}/locked"));
    assert_eq!(s.list(&locked).await, Err(Error::PermissionDenied(locked.clone())));
    let missing = p(&format!("{mixed}/nope"));
    assert_eq!(s.list(&missing).await, Err(Error::NotFound(missing.clone())));
    assert_eq!(s.stat(&missing).await, Err(Error::NotFound(missing.clone())));
    assert!(!s.exists(&missing).await.unwrap());
    assert!(s.exists(&p(&format!("{mixed}/dangling"))).await.unwrap());

    let link = p(&format!("{mixed}/link-dir"));
    assert_eq!(s.stat(&link).await.unwrap().kind, EntryKind::Dir);
    let l = s.lstat(&link).await.unwrap();
    assert_eq!((l.name.as_str(), l.kind), ("link-dir", EntryKind::Symlink { target: Some(LinkTarget::Dir) }));
    assert_eq!(s.realpath(&link).await.unwrap(), p(&format!("{mixed}/dir-01")));
    let inside = s.list(&link).await.unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].name, "inside.txt");
    assert_eq!(s.stat(&RemotePath::root()).await.unwrap().name, "/");
}

#[tokio::test(flavor = "multi_thread")]
async fn paged_listing_of_10k_entries() {
    let Some(c) = connect("paged_listing_of_10k_entries").await else { return };
    let s = &c.sftp;
    let (mixed, many) = (c.mixed(), c.many());

    let start = Instant::now();
    let mut rx = s.list_paged(&p(&many), 512);
    let mut pages = Vec::new();
    let mut first_page = None;
    while let Some(page) = rx.recv().await {
        first_page.get_or_insert_with(|| start.elapsed());
        pages.push(page.unwrap().len());
    }
    let paged = start.elapsed();
    assert_eq!(pages.iter().sum::<usize>(), 10_000);
    assert_eq!(pages.len(), 20);
    assert!(pages[..19].iter().all(|&n| n == 512));
    assert_eq!(pages[19], 10_000 - 19 * 512);

    let start = Instant::now();
    let all = s.list(&p(&many)).await.unwrap();
    let whole = start.elapsed();
    assert_eq!(all.len(), 10_000);
    assert!(all.iter().all(|e| e.kind == EntryKind::File && e.size == 0));
    println!("10k listing: first page after {first_page:?}, paged total {paged:?}, list() {whole:?}");

    // An empty folder yields one empty page; an unreadable one a single error.
    let dir = scratch(&c, "paged").await;
    let mut rx = s.list_paged(&dir, 512);
    assert_eq!(rx.recv().await, Some(Ok(Vec::new())));
    assert_eq!(rx.recv().await, None);
    let locked = p(&format!("{mixed}/locked"));
    let mut rx = s.list_paged(&locked, 512);
    assert_eq!(rx.recv().await, Some(Err(Error::PermissionDenied(locked))));
    assert_eq!(rx.recv().await, None);

    // Dropping the receiver early stops the listing; the session stays usable.
    let mut rx = s.list_paged(&p(&many), 100);
    assert_eq!(rx.recv().await.unwrap().unwrap().len(), 100);
    drop(rx);
    assert_eq!(s.list(&p(&mixed)).await.unwrap().len(), 100);
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mkdir_rename_remove() {
    let Some(c) = connect("mkdir_rename_remove").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "ops").await;
    let a = dir.join("a");
    let b = dir.join("b");

    s.mkdir(&a).await.unwrap();
    assert_eq!(s.mkdir(&a).await, Err(Error::AlreadyExists(a.clone())));
    s.rename(&a, &b).await.unwrap();
    assert!(!s.exists(&a).await.unwrap());
    assert_eq!(s.stat(&b).await.unwrap().kind, EntryKind::Dir);

    let f1 = dir.join("one.txt");
    let f2 = dir.join("two.txt");
    write_pipelined(s, &f1, WriteMode::Truncate, 0, b"one").await;
    write_pipelined(s, &f2, WriteMode::Truncate, 0, b"second").await;
    assert_eq!(s.rename(&f1, &f2).await, Err(Error::AlreadyExists(f2.clone())));
    s.posix_rename(&f1, &f2).await.unwrap();
    assert!(!s.exists(&f1).await.unwrap());
    assert_eq!(s.stat(&f2).await.unwrap().size, 3);
    let missing = dir.join("missing");
    assert_eq!(s.rename(&missing, &f1).await, Err(Error::NotFound(missing.clone())));

    s.rename(&f2, &b.join("moved.txt")).await.unwrap();
    assert!(matches!(s.remove_dir(&b).await, Err(Error::Io(_))), "folder isn't empty");
    s.remove_file(&b.join("moved.txt")).await.unwrap();
    s.remove_dir(&b).await.unwrap();
    assert_eq!(s.remove_file(&missing).await, Err(Error::NotFound(missing.clone())));
    assert_eq!(s.remove_dir(&missing).await, Err(Error::NotFound(missing.clone())));

    let locked = p(&format!("{}/locked/x", c.mixed()));
    assert_eq!(s.mkdir(&locked).await, Err(Error::PermissionDenied(locked.clone())));
    assert!(s.list(&dir).await.unwrap().is_empty());
    rm_rf(s, &dir).await;
}

/// Runs `script` on the server as root (`sh -c`).
fn root_sh(c: &Conn, script: &str) {
    c.fixture.exec(&["sh", "-c", script]).unwrap_or_else(|e| panic!("{script}: {e}"));
}

async fn put(s: &Sftp, path: &RemotePath, data: &[u8]) {
    write_pipelined(s, path, WriteMode::Truncate, 0, data).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_all_deletes_nested_folders_but_never_follows_links() {
    let Some(c) = connect("remove_all_deletes_nested_folders_but_never_follows_links").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "rmall").await;
    let target = dir.join("target");
    s.mkdir(&target).await.unwrap();
    put(s, &target.join("keep.txt"), b"keep").await;
    let tree = dir.join("tree");
    for d in ["", "sub", "sub/deeper", "empty"] {
        s.mkdir(&tree.join(d)).await.unwrap();
    }
    for f in ["a.txt", "sub/b.txt", "sub/deeper/c.txt"] {
        put(s, &tree.join(f), b"x").await;
    }
    // Links (made as root; removing one only needs the folder to be writable): to a folder
    // outside the tree, to a file inside it, and to nothing.
    let t = tree.as_str();
    root_sh(
        &c,
        &format!("ln -s ../target '{t}/link-dir' && ln -s a.txt '{t}/link-file' && ln -s nowhere '{t}/dangling'"),
    );

    let mut calls = Vec::new();
    let removed = s.remove_all(&tree, |done, total| calls.push((done, total))).await.unwrap();
    // tree, a.txt, sub, empty, 3 links, b.txt, deeper, c.txt.
    assert_eq!(removed, 10);
    assert!(!s.exists(&tree).await.unwrap());
    assert_eq!(s.list(&target).await.unwrap().len(), 1, "the link's target stays");
    assert_eq!(s.stat(&target.join("keep.txt")).await.unwrap().size, 4);

    // Counting first (found so far, no total), then one call per removal up to the total.
    let split = calls.iter().position(|(_, t)| t.is_some()).expect("removal progress");
    let (counting, removing) = calls.split_at(split);
    assert!(!counting.is_empty() && counting.iter().all(|(_, t)| t.is_none()));
    assert!(counting.windows(2).all(|w| w[0].0 <= w[1].0), "{counting:?}");
    assert_eq!(counting.last(), Some(&(10, None)));
    let want: Vec<_> = (1..=10).map(|n| (n, Some(10))).collect();
    assert_eq!(removing, &want[..]);

    // Several paths at once: a file, a link to a folder, and one listed twice (inside another).
    let other = dir.join("other");
    s.mkdir(&other).await.unwrap();
    put(s, &other.join("f.txt"), b"f").await;
    let lone = dir.join("lone.txt");
    put(s, &lone, b"lone").await;
    root_sh(&c, &format!("ln -s target '{}/link-again'", dir.as_str()));
    let paths = [other.join("f.txt"), lone, dir.join("link-again"), other];
    assert_eq!(s.remove_all_of(&paths, |_, _| {}).await, Ok(4));
    assert_eq!(s.list(&dir).await.unwrap().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["target"]);
    assert_eq!(s.list(&target).await.unwrap().len(), 1);

    let missing = dir.join("missing");
    assert_eq!(s.remove_all(&missing, |_, _| {}).await, Err(Error::NotFound(missing)));
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_all_stops_at_an_unreadable_folder_before_removing_anything() {
    let Some(c) = connect("remove_all_stops_at_an_unreadable_folder_before_removing_anything").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "rmlocked").await;
    let tree = dir.join("tree");
    let locked = tree.join("locked");
    s.mkdir(&tree).await.unwrap();
    s.mkdir(&locked).await.unwrap();
    put(s, &tree.join("f.txt"), b"f").await;
    put(s, &locked.join("inner.txt"), b"inner").await;
    root_sh(&c, &format!("chmod 000 '{}'", locked.as_str()));

    let mut removals = 0;
    let res = s.remove_all(&tree, |_, total| removals += u32::from(total.is_some())).await;
    assert_eq!(res, Err(Error::PermissionDenied(locked.clone())));
    assert_eq!(removals, 0, "the count stops first");
    assert_eq!(s.list(&tree).await.unwrap().len(), 2, "nothing was removed");

    root_sh(&c, &format!("chmod 755 '{}'", locked.as_str()));
    assert_eq!(s.remove_all(&tree, |_, _| {}).await, Ok(4));
    assert!(!s.exists(&tree).await.unwrap());
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn write_read_round_trip_and_mtime() {
    let Some(c) = connect("write_read_round_trip_and_mtime").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "io").await;
    let f = dir.join("data.bin");

    // 3.5 MiB: many chunks in flight, and a short final chunk.
    let data = pattern(3 * 1024 * 1024 + 512 * 1024 + 7, 1);
    write_pipelined(s, &f, WriteMode::Truncate, 0, &data).await;
    assert_eq!(s.stat(&f).await.unwrap().size, data.len() as u64);
    let back = read_pipelined(s, &f, 0, data.len() as u64 + 1000).await;
    assert!(back == data, "round trip differs");

    // Reading at and past the end gives empty buffers.
    let file = s.open_read(&f).await.unwrap();
    assert!(file.read_at(data.len() as u64, 10).await.unwrap().is_empty());
    assert!(file.read_at(data.len() as u64 + (1 << 20), 10).await.unwrap().is_empty());
    assert_eq!(file.stat().await.unwrap().size, data.len() as u64);
    file.close().await.unwrap();

    // Resume keeps the contents and writes at the given offset (append-at-offset).
    let head = pattern(100_000, 2);
    let tail = pattern(50_000, 3);
    let r = dir.join("resume.bin");
    write_pipelined(s, &r, WriteMode::Truncate, 0, &head).await;
    write_pipelined(s, &r, WriteMode::Resume, head.len() as u64, &tail).await;
    let back = read_pipelined(s, &r, 0, 150_000).await;
    assert!(back[..100_000] == head[..] && back[100_000..] == tail[..]);
    // Truncate empties it again.
    s.open_write(&r, WriteMode::Truncate).await.unwrap().close().await.unwrap();
    assert_eq!(s.stat(&r).await.unwrap().size, 0);

    assert_eq!(s.open_write(&r, WriteMode::CreateNew).await.err(), Some(Error::AlreadyExists(r.clone())));
    let fresh = dir.join("fresh.bin");
    s.open_write(&fresh, WriteMode::CreateNew).await.unwrap().close().await.unwrap();
    let missing = dir.join("missing.bin");
    assert_eq!(s.open_read(&missing).await.err(), Some(Error::NotFound(missing)));
    let locked = p(&format!("{}/locked/f", c.mixed()));
    assert_eq!(s.open_write(&locked, WriteMode::Truncate).await.err(), Some(Error::PermissionDenied(locked)));

    s.set_mtime(&f, 1_000_000_000).await.unwrap();
    assert_eq!(s.stat(&f).await.unwrap().mtime, Some(1_000_000_000));
    s.set_mtime(&dir, 1_234_567_890).await.unwrap();
    assert_eq!(s.stat(&dir).await.unwrap().mtime, Some(1_234_567_890));
    rm_rf(s, &dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_data_small_file_is_exact() {
    let Some(c) = connect("copy_data_small_file_is_exact").await else { return };
    let s = &c.sftp;
    let dir = scratch(&c, "copysmall").await;
    let src = dir.join("src.bin");
    let data = pattern(777_777, 9);
    write_pipelined(s, &src, WriteMode::Truncate, 0, &data).await;

    let dst = dir.join("dst.bin");
    s.copy_data(&src, &dst).await.unwrap();
    assert!(read_pipelined(s, &dst, 0, data.len() as u64).await == data);

    let streamed = dir.join("streamed.bin");
    s.copy_streaming(&src, &streamed).await.unwrap();
    assert!(read_pipelined(s, &streamed, 0, data.len() as u64).await == data);

    let empty = dir.join("empty");
    write_pipelined(s, &empty, WriteMode::Truncate, 0, b"").await;
    s.copy_data(&empty, &dir.join("empty2")).await.unwrap();
    s.copy_streaming(&empty, &dir.join("empty3")).await.unwrap();
    assert_eq!(s.stat(&dir.join("empty2")).await.unwrap().size, 0);
    assert_eq!(s.stat(&dir.join("empty3")).await.unwrap().size, 0);

    let missing = dir.join("missing");
    assert_eq!(s.copy_data(&missing, &dst).await, Err(Error::NotFound(missing.clone())));
    assert_eq!(s.copy_streaming(&missing, &dst).await, Err(Error::NotFound(missing)));
    rm_rf(s, &dir).await;
}

/// Spike S4: server-side copy of a 1 GiB file, against the streaming fallback. Slow (the file is
/// made on first use, then copied twice): run with `-- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: creates and copies 1 GiB; run with `cargo test -p dromon-sftp -- --ignored`"]
async fn copy_data_big_file() {
    let Some(c) = connect("copy_data_big_file").await else { return };
    let s = &c.sftp;
    let big = p(&c.fixture.big_file().await);
    assert_eq!(s.stat(&big).await.unwrap().size, BIG_SIZE);
    let dir = scratch(&c, "copybig").await;

    let dst = dir.join("copy-data.bin");
    let mut steps = 0;
    let start = Instant::now();
    s.copy_data_with_progress(&big, &dst, |_| steps += 1).await.unwrap();
    let server_side = start.elapsed();
    assert_eq!(s.stat(&dst).await.unwrap().size, BIG_SIZE);
    assert_eq!(steps, BIG_SIZE / dromon_sftp::COPY_DATA_CHUNK);

    let streamed = dir.join("streamed.bin");
    let start = Instant::now();
    s.copy_streaming(&big, &streamed).await.unwrap();
    let through_app = start.elapsed();
    assert_eq!(s.stat(&streamed).await.unwrap().size, BIG_SIZE);

    println!("S4 copy-data 1 GiB: {}", rate(BIG_SIZE, server_side));
    println!("S4 streaming 1 GiB: {}", rate(BIG_SIZE, through_app));

    // Spot-check contents: the first and last MiB and one in the middle of each copy.
    for offset in [0, BIG_SIZE / 2 + 12_345, BIG_SIZE - (1 << 20)] {
        let want = read_pipelined(s, &big, offset, 1 << 20).await;
        assert!(read_pipelined(s, &dst, offset, 1 << 20).await == want, "copy-data @ {offset}");
        assert!(read_pipelined(s, &streamed, offset, 1 << 20).await == want, "stream @ {offset}");
    }
    rm_rf(s, &dir).await;
}
