//! The server matrix for the SFTP layer (`.plans/dromon-plan.md` §9, M5): the same checks against
//! every server of `dromon_testkit::matrix::variants()`: with `DROMON_SERVER_MATRIX=1` Debian
//! trixie (OpenSSH 10), bullseye (OpenSSH 8.4p1: no `copy-data`, no `limits@openssh.com`),
//! Alpine (musl) and trixie behind toxiproxy (latency, bandwidth caps, sliced replies); without
//! it only trixie, the suite's usual server. Each check prints one `matrix:` line per server
//! (ok with a note, skip with the reason, or FAIL) and fails after all ran if any failed.
//!
//! ```sh
//! DROMON_SERVER_MATRIX=1 cargo test -p dromon-sftp --test matrix
//! ```
//!
//! Capabilities are asked from the fixture (`Fixture::has`), never assumed. The 100k-entry
//! folder is made on the server in one command and listed in the matrix run only.

use std::time::{Duration, Instant};

use dromon_core::{Entry, EntryKind, Error, LinkTarget, RemotePath};
use dromon_sftp::{DEFAULT_CHUNK, MAX_CHUNK, Sftp, WriteMode};
use dromon_testkit::matrix::{self, Outcome};
use dromon_testkit::{Feature, Fixture, Role};
use tokio::process::{Child, Command};

struct Conn {
    sftp: Sftp,
    _ssh: Child,
}

/// An `ssh -s … sftp` child as the key user, with the ssh the app runs.
async fn connect(fx: &Fixture) -> Conn {
    let mut ssh = Command::new(dromon_ssh::ssh_program())
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

fn seed(fx: &Fixture, rel: &str) -> RemotePath {
    fx.path(Role::Key, rel)
}

/// A fresh, empty scratch folder for one check.
async fn scratch(fx: &Fixture, sftp: &Sftp, name: &str) -> RemotePath {
    let dir = fx.path(Role::Key, &format!("scratch/matrix-{name}-{}", std::process::id()));
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

/// Runs `script` on the server as root (`sh -c`).
fn root_sh(fx: &Fixture, script: &str) {
    fx.exec(&["sh", "-c", script]).unwrap_or_else(|e| panic!("{script}: {e}"));
}

fn find<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
    entries.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("{name} not listed"))
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed ^ (i >> 13) as u8)).collect()
}

async fn put(sftp: &Sftp, path: &RemotePath, mode: WriteMode, offset: u64, data: &[u8]) {
    let file = sftp.open_write(path, mode).await.unwrap();
    let chunk = sftp.chunk_size() as usize;
    let acks: Vec<_> = data.chunks(chunk).enumerate().map(|(i, c)| file.write_at(offset + (i * chunk) as u64, c)).collect();
    for ack in acks {
        ack.await.unwrap();
    }
    file.close().await.unwrap();
}

async fn get(sftp: &Sftp, path: &RemotePath, len: u64) -> Vec<u8> {
    let file = sftp.open_read(path).await.unwrap();
    let chunk = u64::from(sftp.chunk_size());
    let reads: Vec<_> = (0..len.div_ceil(chunk)).map(|i| file.read_at(i * chunk, chunk.min(len - i * chunk) as u32)).collect();
    let mut out = Vec::new();
    for read in reads {
        out.extend(read.await.unwrap());
    }
    file.close().await.unwrap();
    out
}

fn secs(d: Duration) -> String {
    format!("{:.2} s", d.as_secs_f64())
}

/// Streams `dir` in pages of 512: the page sizes, when the first arrived, and the total time.
async fn paged(sftp: &Sftp, dir: &RemotePath) -> (Vec<usize>, Duration, Duration) {
    let start = Instant::now();
    let mut rx = sftp.list_paged(dir, 512);
    let (mut pages, mut first) = (Vec::new(), None);
    while let Some(page) = rx.recv().await {
        first.get_or_insert_with(|| start.elapsed());
        pages.push(page.unwrap().len());
    }
    (pages, first.unwrap_or_default(), start.elapsed())
}

/// `total` entries in pages of 512: all full but the last.
fn assert_pages(pages: &[usize], total: usize) {
    assert_eq!(pages.iter().sum::<usize>(), total);
    assert_eq!(pages.len(), total.div_ceil(512));
    let (last, full) = pages.split_last().unwrap();
    assert!(full.iter().all(|&n| n == 512), "{pages:?}");
    assert_eq!(*last, total - full.len() * 512);
}

#[tokio::test(flavor = "multi_thread")]
async fn capabilities() {
    matrix::run("sftp capabilities", |fx| async move {
        let c = connect(fx).await;
        assert_eq!(c.sftp.home().await.unwrap(), fx.home(Role::Key));
        let caps = c.sftp.capabilities();
        let exts: Vec<&str> = caps.extensions.keys().map(String::as_str).collect();
        assert_eq!(caps.posix_rename, fx.has(Feature::PosixRename), "posix-rename; advertised: {exts:?}");
        assert_eq!(caps.copy_data, fx.has(Feature::CopyData), "copy-data; advertised: {exts:?}");
        assert_eq!(caps.limits.is_some(), fx.has(Feature::Limits), "limits; advertised: {exts:?}");
        let chunk = c.sftp.chunk_size();
        match caps.limits {
            Some(l) => {
                assert!(l.max_read > 0 && l.max_write > 0, "{l:?}");
                assert_eq!(u64::from(chunk), l.max_read.min(l.max_write).min(u64::from(MAX_CHUNK)));
            }
            None => assert_eq!(chunk, DEFAULT_CHUNK, "no limits: the 32 KiB default"),
        }
        Outcome::Ok(format!("{}; chunk {chunk}; {}", fx.banner(), exts.join(" ")))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn listing_100_mixed() {
    matrix::run("sftp listing 100 (mixed)", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let mixed = seed(fx, "fixture/mixed");
        let start = Instant::now();
        let entries = s.list(&mixed).await.unwrap();
        let took = start.elapsed();
        assert_eq!(entries.len(), 100, "80 files, 10 folders and 10 specials");
        let f = find(&entries, "file-001.txt");
        assert_eq!((f.kind, f.size, f.mtime), (EntryKind::File, 100, Some(1_577_934_245)));
        assert_eq!(find(&entries, "dir-01").kind, EntryKind::Dir);
        assert_eq!(find(&entries, "link-dir").kind, EntryKind::Symlink { target: Some(LinkTarget::Dir) });
        assert_eq!(find(&entries, "link-file").kind, EntryKind::Symlink { target: Some(LinkTarget::File) });
        assert_eq!(find(&entries, "dangling").kind, EntryKind::Symlink { target: None });
        assert_eq!(find(&entries, "pipe").kind, EntryKind::Other);
        assert_eq!(find(&entries, "unicode-ğüş.txt").size, 8);
        assert_eq!(find(&entries, "space name.txt").size, 7);
        let missing = mixed.join("nope");
        assert_eq!(s.list(&missing).await, Err(Error::NotFound(missing.clone())));
        assert_eq!(s.stat(&missing).await, Err(Error::NotFound(missing)));
        assert_eq!(s.stat(&mixed.join("link-dir")).await.unwrap().kind, EntryKind::Dir);
        assert_eq!(s.realpath(&mixed.join("link-dir")).await.unwrap(), mixed.join("dir-01"));
        Outcome::Ok(format!("list() {}", secs(took)))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn listing_10k_paged() {
    matrix::run("sftp listing 10k (paged)", |fx| async move {
        let c = connect(fx).await;
        let many = seed(fx, "fixture/many");
        let (pages, first, total) = paged(&c.sftp, &many).await;
        assert_pages(&pages, 10_000);
        let start = Instant::now();
        let all = c.sftp.list(&many).await.unwrap();
        let whole = start.elapsed();
        assert_eq!(all.len(), 10_000);
        assert!(all.iter().all(|e| e.kind == EntryKind::File && e.size == 0));
        // Dropping the receiver early stops the listing; the session stays usable.
        let mut rx = c.sftp.list_paged(&many, 100);
        assert_eq!(rx.recv().await.unwrap().unwrap().len(), 100);
        drop(rx);
        assert_eq!(c.sftp.list(&seed(fx, "fixture/mixed")).await.unwrap().len(), 100);
        Outcome::Ok(format!("first page {}, {} pages {}, list() {}", secs(first), pages.len(), secs(total), secs(whole)))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn listing_100k() {
    matrix::run("sftp listing 100k", |fx| async move {
        if let Some(skip) = Outcome::matrix_only() {
            return skip;
        }
        let c = connect(fx).await;
        let huge = seed(fx, "fixture/huge");
        let start = Instant::now();
        root_sh(
            fx,
            &format!(
                "set -e; d='{huge}'; rm -rf \"$d\"; mkdir -p \"$d\"; cd \"$d\"; \
                 seq -w 1 100000 | sed 's/^/f/' | xargs touch; chown -R {} \"$d\"",
                fx.owner(Role::Key)
            ),
        );
        let made = start.elapsed();
        let (pages, first, total) = paged(&c.sftp, &huge).await;
        assert_pages(&pages, 100_000);
        let start = Instant::now();
        let all = c.sftp.list(&huge).await.unwrap();
        let whole = start.elapsed();
        assert_eq!(all.len(), 100_000);
        assert!(all.iter().all(|e| e.kind == EntryKind::File));
        root_sh(fx, &format!("rm -rf '{huge}'"));
        Outcome::Ok(format!(
            "made in {}; first page {}, {} pages {}, list() {}",
            secs(made),
            secs(first),
            pages.len(),
            secs(total),
            secs(whole)
        ))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stat_mkdir_rename_remove() {
    matrix::run("sftp stat/mkdir/rename/remove_all", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let dir = scratch(fx, s, "ops").await;
        let (a, b) = (dir.join("a"), dir.join("b"));
        s.mkdir(&a).await.unwrap();
        assert_eq!(s.mkdir(&a).await, Err(Error::AlreadyExists(a.clone())));
        s.rename(&a, &b).await.unwrap();
        assert!(!s.exists(&a).await.unwrap());
        assert_eq!(s.stat(&b).await.unwrap().kind, EntryKind::Dir);

        let (f1, f2) = (dir.join("one.txt"), dir.join("two.txt"));
        put(s, &f1, WriteMode::Truncate, 0, b"one").await;
        put(s, &f2, WriteMode::Truncate, 0, b"second").await;
        assert_eq!(s.rename(&f1, &f2).await, Err(Error::AlreadyExists(f2.clone())));
        let replaced = if fx.has(Feature::PosixRename) {
            s.posix_rename(&f1, &f2).await.unwrap();
            assert!(!s.exists(&f1).await.unwrap());
            assert_eq!(s.stat(&f2).await.unwrap().size, 3);
            "posix-rename replaces"
        } else {
            assert_eq!(s.posix_rename(&f1, &f2).await, Err(Error::Unsupported("posix-rename")));
            "no posix-rename"
        };

        // A tree with a link out of it: removed whole, the link's target stays.
        let tree = dir.join("tree");
        for d in ["", "sub", "sub/deeper", "empty"] {
            s.mkdir(&tree.join(d)).await.unwrap();
        }
        for f in ["a.txt", "sub/b.txt", "sub/deeper/c.txt"] {
            put(s, &tree.join(f), WriteMode::Truncate, 0, b"x").await;
        }
        root_sh(fx, &format!("ln -s ../b '{tree}/link-out'"));
        assert!(matches!(s.remove_dir(&tree).await, Err(Error::Io(_))), "the folder isn't empty");
        let mut last = None;
        let removed = s.remove_all(&tree, |done, total| last = Some((done, total))).await.unwrap();
        assert_eq!(removed, 8, "tree, a.txt, sub, b.txt, deeper, c.txt, empty, link-out");
        assert_eq!(last, Some((8, Some(8))));
        assert!(!s.exists(&tree).await.unwrap());
        assert_eq!(s.stat(&b).await.unwrap().kind, EntryKind::Dir, "the link's target stays");
        let missing = dir.join("missing");
        assert_eq!(s.remove_all(&missing, |_, _| {}).await, Err(Error::NotFound(missing.clone())));
        assert_eq!(s.remove_file(&missing).await, Err(Error::NotFound(missing)));
        rm_rf(s, &dir).await;
        Outcome::Ok(replaced.into())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn symlink_loops_are_never_followed() {
    matrix::run("sftp symlink loops", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let dir = scratch(fx, s, "loops").await;
        let loops = make_loops(fx, s, &dir).await;
        let check = async {
            let entries = s.list(&loops).await.unwrap();
            assert_eq!(entries.len(), 5, "{entries:?}");
            assert_eq!(find(&entries, "self").kind, EntryKind::Symlink { target: Some(LinkTarget::Dir) });
            assert_eq!(find(&entries, "inner").kind, EntryKind::Dir);
            // a → b → a: stat fails (too many levels of links), so no target kind.
            assert_eq!(find(&entries, "a").kind, EntryKind::Symlink { target: None });
            assert_eq!(find(&entries, "b").kind, EntryKind::Symlink { target: None });
            let inner = s.list(&loops.join("inner")).await.unwrap();
            assert_eq!(find(&inner, "parent").kind, EntryKind::Symlink { target: Some(LinkTarget::Dir) });
            assert_eq!(find(&inner, "top").kind, EntryKind::Symlink { target: Some(LinkTarget::Dir) });
            assert!(s.exists(&loops.join("a")).await.unwrap());
            // OpenSSH's sftp-server reports ELOOP as "no such file".
            let a = loops.join("a");
            assert_eq!(s.stat(&a).await, Err(Error::NotFound(a.clone())), "a looping link has no target");

            // Deleting the tree removes the links themselves and never follows them: the folder
            // `top` points at (the scratch folder) and its keep.txt stay.
            let removed = s.remove_all(&loops, |_, _| {}).await.unwrap();
            assert_eq!(removed, 8, "loops, file.txt, self, inner, parent, top, a, b");
            assert!(!s.exists(&loops).await.unwrap());
            assert_eq!(s.stat(&dir.join("keep.txt")).await.unwrap().size, 4);
        };
        tokio::time::timeout(Duration::from_secs(120), check).await.expect("a link loop was followed (timed out)");
        rm_rf(s, &dir).await;
        Outcome::ok()
    })
    .await;
}

/// `<dir>/loops`: a file, `self -> .`, `inner/parent -> ..` (its own parent), `inner/top -> ../..`
/// (out of the tree, to `<dir>`, which holds `keep.txt`), and the cycle `a -> b`, `b -> a`.
async fn make_loops(fx: &Fixture, s: &Sftp, dir: &RemotePath) -> RemotePath {
    let loops = dir.join("loops");
    s.mkdir(&loops).await.unwrap();
    s.mkdir(&loops.join("inner")).await.unwrap();
    put(s, &loops.join("file.txt"), WriteMode::Truncate, 0, b"file").await;
    put(s, &dir.join("keep.txt"), WriteMode::Truncate, 0, b"keep").await;
    root_sh(
        fx,
        &format!(
            "cd '{loops}' && ln -s . self && ln -s b a && ln -s a b && ln -s .. inner/parent && ln -s ../.. inner/top"
        ),
    );
    loops
}

#[tokio::test(flavor = "multi_thread")]
async fn unreadable_folders() {
    matrix::run("sftp unreadable folders", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let locked = seed(fx, "fixture/mixed/locked");
        assert_eq!(s.list(&locked).await, Err(Error::PermissionDenied(locked.clone())));
        let mut rx = s.list_paged(&locked, 512);
        assert_eq!(rx.recv().await, Some(Err(Error::PermissionDenied(locked.clone()))));
        assert_eq!(rx.recv().await, None);
        let inside = locked.join("x");
        assert_eq!(s.mkdir(&inside).await, Err(Error::PermissionDenied(inside.clone())));
        assert_eq!(s.open_write(&inside, WriteMode::Truncate).await.err(), Some(Error::PermissionDenied(inside)));

        // A delete stops at an unreadable folder before removing anything.
        let dir = scratch(fx, s, "rmlocked").await;
        let tree = dir.join("tree");
        let inner = tree.join("locked");
        s.mkdir(&tree).await.unwrap();
        s.mkdir(&inner).await.unwrap();
        put(s, &tree.join("f.txt"), WriteMode::Truncate, 0, b"f").await;
        put(s, &inner.join("inner.txt"), WriteMode::Truncate, 0, b"inner").await;
        root_sh(fx, &format!("chmod 000 '{inner}'"));
        let mut removals = 0;
        let res = s.remove_all(&tree, |_, total| removals += u32::from(total.is_some())).await;
        assert_eq!(res, Err(Error::PermissionDenied(inner.clone())));
        assert_eq!(removals, 0, "the count stops first");
        assert_eq!(s.list(&tree).await.unwrap().len(), 2, "nothing was removed");
        root_sh(fx, &format!("chmod 755 '{inner}'"));
        assert_eq!(s.remove_all(&tree, |_, _| {}).await, Ok(4));
        rm_rf(s, &dir).await;
        Outcome::ok()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn write_read_round_trip() {
    matrix::run("sftp write/read/resume/mtime", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let dir = scratch(fx, s, "io").await;
        let f = dir.join("data.bin");
        let data = pattern(3 * 1024 * 1024 + 512 * 1024 + 7, 1);
        let start = Instant::now();
        put(s, &f, WriteMode::Truncate, 0, &data).await;
        let up = start.elapsed();
        assert_eq!(s.stat(&f).await.unwrap().size, data.len() as u64);
        let start = Instant::now();
        assert!(get(s, &f, data.len() as u64 + 1000).await == data, "round trip differs");
        let down = start.elapsed();

        // Resume writes at an offset and keeps what is there; CreateNew refuses a taken name.
        let r = dir.join("resume.bin");
        put(s, &r, WriteMode::Truncate, 0, &data[..100_000]).await;
        put(s, &r, WriteMode::Resume, 100_000, &data[100_000..150_000]).await;
        assert!(get(s, &r, 150_000).await == data[..150_000]);
        assert_eq!(s.open_write(&r, WriteMode::CreateNew).await.err(), Some(Error::AlreadyExists(r.clone())));

        s.set_mtime(&f, 1_000_000_000).await.unwrap();
        assert_eq!(s.stat(&f).await.unwrap().mtime, Some(1_000_000_000));
        rm_rf(s, &dir).await;
        Outcome::Ok(format!("3.5 MiB up {}, down {} in {} B chunks", secs(up), secs(down), s.chunk_size()))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_data_or_streaming() {
    matrix::run("sftp copy-data or streaming", |fx| async move {
        let c = connect(fx).await;
        let s = &c.sftp;
        let dir = scratch(fx, s, "copy").await;
        let src = dir.join("src.bin");
        let data = pattern(777_777, 9);
        put(s, &src, WriteMode::Truncate, 0, &data).await;

        let dst = dir.join("dst.bin");
        let how = if fx.has(Feature::CopyData) {
            s.copy_data(&src, &dst).await.unwrap();
            assert!(get(s, &dst, data.len() as u64).await == data);
            "copy-data"
        } else {
            assert_eq!(s.copy_data(&src, &dst).await, Err(Error::Unsupported("copy-data")));
            assert!(!s.exists(&dst).await.unwrap(), "nothing written");
            "no copy-data: streamed"
        };
        let streamed = dir.join("streamed.bin");
        s.copy_streaming(&src, &streamed).await.unwrap();
        assert!(get(s, &streamed, data.len() as u64).await == data);
        rm_rf(s, &dir).await;
        Outcome::Ok(how.into())
    })
    .await;
}
