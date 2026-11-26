//! Windows' own OpenSSH server (Win32-OpenSSH's `sshd` and `sftp-server.exe`), whose paths are
//! drive paths in SFTP form: `/C:/Users/<user>`. Checks that `RemotePath`, the SFTP layer and the
//! transfer I/O handle them: the home folder and the drive root resolve and list, a file and a
//! folder tree round-trip (upload, download, mtime), renames, a same-host copy, and deletes.
//!
//! Opt-in, like the native sshd backend: the `windows` CI job installs and starts the server,
//! authorizes a fresh key for the runner's user and sets
//!
//! - `DROMON_WINDOWS_SSHD=1`
//! - `DROMON_WINDOWS_SSHD_KEY`: the private key's path (no passphrase)
//! - `DROMON_WINDOWS_SSHD_USER`: the user (default: `USERNAME`)
//! - `DROMON_WINDOWS_SSHD_PORT`: the port (default 22), on `DROMON_WINDOWS_SSHD_HOST`
//!   (default 127.0.0.1)
//!
//! Without `DROMON_WINDOWS_SSHD=1` every test prints a skip line and passes. See the
//! `Windows OpenSSH server` step in `.github/workflows/ci.yml`.

use std::path::Path;
use std::time::UNIX_EPOCH;

use dromon_core::{EntryKind, Error, RemotePath};
use dromon_sftp::{Sftp, WriteMode};
use dromon_testkit::{NULL_DEVICE, openssh};
use dromon_transfer::FilePlan;
use dromon_transfer::io::{
    CopyOptions, copy_remote, create_dirs, download, scan_local, scan_remote, scan_remote_copy, upload,
};
use tokio::process::{Child, Command};

const ENV: &str = "DROMON_WINDOWS_SSHD";

/// The server named by the environment.
struct Server {
    host: String,
    port: String,
    user: String,
    key: String,
}

impl Server {
    /// `None` (after a skip line) unless `DROMON_WINDOWS_SSHD=1`; panics when it is set but the
    /// key isn't.
    fn from_env(test: &str) -> Option<Server> {
        if std::env::var(ENV).ok().as_deref() != Some("1") {
            eprintln!(
                "skipping {test}: set {ENV}=1 and {ENV}_KEY to test against a Windows OpenSSH server (CI: windows job)"
            );
            return None;
        }
        let var = |name: &str, default: Option<&str>| {
            std::env::var(format!("{ENV}_{name}"))
                .ok()
                .filter(|v| !v.is_empty())
                .or_else(|| default.map(str::to_owned))
                .unwrap_or_else(|| panic!("{ENV}=1 but {ENV}_{name} is not set"))
        };
        let user_default = std::env::var("USERNAME").ok();
        Some(Server {
            host: var("HOST", Some("127.0.0.1")),
            port: var("PORT", Some("22")),
            user: var("USER", user_default.as_deref()),
            key: var("KEY", None),
        })
    }

    async fn connect(&self) -> Conn {
        let mut args: Vec<String> = ["-T", "-x", "-e", "none", "-F", NULL_DEVICE].map(String::from).into();
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=no",
            &format!("UserKnownHostsFile={NULL_DEVICE}"),
            &format!("GlobalKnownHostsFile={NULL_DEVICE}"),
            "LogLevel=ERROR",
            "IdentitiesOnly=yes",
            "ConnectTimeout=10",
        ] {
            args.extend(["-o".into(), option.into()]);
        }
        args.extend(["-i".into(), self.key.clone(), "-p".into(), self.port.clone()]);
        let mut ssh = Command::new(openssh("ssh"))
            .args(args)
            .args(["-s", &format!("{}@{}", self.user, self.host), "sftp"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn ssh");
        let stdout = ssh.stdout.take().unwrap();
        let stdin = ssh.stdin.take().unwrap();
        let sftp = Sftp::new(tokio::io::join(stdout, stdin)).await.expect("SFTP session to the Windows sshd");
        Conn { sftp, _ssh: ssh }
    }
}

struct Conn {
    sftp: Sftp,
    _ssh: Child,
}

/// `/X:/…`: a drive path in Win32-OpenSSH's SFTP form.
fn is_drive_path(path: &RemotePath) -> bool {
    let b = path.as_str().as_bytes();
    b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' && (b.len() == 3 || b[3] == b'/')
}

fn names(entries: &[dromon_core::Entry]) -> Vec<String> {
    entries.iter().map(|e| e.name.to_ascii_lowercase()).collect()
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed ^ (i >> 13) as u8)).collect()
}

fn mtime_local(path: &Path) -> i64 {
    std::fs::metadata(path).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
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
    // In pieces the server takes: one write over its limit is refused ("write limit reached").
    let chunk = sftp.chunk_size() as usize;
    for (i, piece) in data.chunks(chunk).enumerate() {
        file.write_at((i * chunk) as u64, piece).await.unwrap();
    }
    file.close().await.unwrap();
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

fn the_plan(scan: dromon_transfer::Scan) -> FilePlan {
    assert_eq!(scan.files.len(), 1, "{scan:?}");
    scan.files.into_iter().next().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn drive_paths_resolve_and_list() {
    let Some(server) = Server::from_env("drive_paths_resolve_and_list") else { return };
    let c = server.connect().await;
    let s = &c.sftp;
    let caps = s.capabilities();
    println!("extensions: {:?}; chunk {}", caps.extensions, s.chunk_size());

    let home = s.home().await.unwrap();
    println!("home: {home}");
    assert!(is_drive_path(&home), "the home folder is a drive path: {home}");
    assert_eq!(RemotePath::new(home.as_str()), home, "normalizing keeps it");
    assert_eq!(s.stat(&home).await.unwrap().kind, EntryKind::Dir);

    // The breadcrumbs: `/`, `/C:`, `/C:/Users`, …; the drive itself is a folder that lists the
    // drive's root (the server reads `/C:` as `C:\`).
    let crumbs = home.ancestors_from_root();
    let drive = crumbs[1].clone();
    assert_eq!(drive.as_str().len(), 3, "{crumbs:?}");
    assert!(is_drive_path(&drive), "{drive}");
    assert_eq!(s.stat(&drive).await.unwrap().kind, EntryKind::Dir, "stat {drive}");
    let root = s.list(&drive).await.unwrap();
    for want in ["windows", "users"] {
        assert!(names(&root).contains(&want.to_owned()), "{drive} lists {want}: {:?}", names(&root));
    }
    let with_slash = RemotePath::new(&format!("{drive}/"));
    assert_eq!(with_slash, drive, "`/C:/` and `/C:` are one path");

    // The drive resolves to itself (the server answers `/C:/`), the parent lists the home folder.
    let resolved = s.realpath(&drive).await.unwrap();
    assert!(resolved.as_str().eq_ignore_ascii_case(drive.as_str()), "realpath {drive}: {resolved}");
    let parent = home.parent().unwrap();
    let up = s.list(&parent).await.unwrap();
    let name = home.file_name().unwrap();
    assert!(up.iter().any(|e| e.name.eq_ignore_ascii_case(name) && e.kind.is_dir_like()), "{parent} lists {name}");
    let missing = home.join("dromon-does-not-exist");
    assert_eq!(s.stat(&missing).await, Err(Error::NotFound(missing.clone())));

    // `/` itself: Win32-OpenSSH lists the drives there (or refuses); shown, not asserted.
    match s.list(&RemotePath::root()).await {
        Ok(entries) => println!("/ lists {:?}", entries.iter().map(|e| (&e.name, e.kind)).collect::<Vec<_>>()),
        Err(e) => println!("/ doesn't list: {e}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn round_trip_rename_copy_delete() {
    let Some(server) = Server::from_env("round_trip_rename_copy_delete") else { return };
    let c = server.connect().await;
    let s = &c.sftp;
    let home = s.home().await.unwrap();
    let dir = home.join(&format!("dromon-windows-sshd-{}", std::process::id()));
    rm_rf(s, &dir).await;
    s.mkdir(&dir).await.unwrap();
    assert!(is_drive_path(&dir), "{dir}");

    // Upload one file (part file, then renamed into place, with its mtime).
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("data.bin");
    let data = pattern(1_500_000, 3);
    std::fs::write(&local, &data).unwrap();
    let remote = dir.join("data.bin");
    let plan = the_plan(scan_local(&local, &remote, false, |_| {}).await.unwrap());
    assert_eq!(upload(s, &plan, CopyOptions::default(), |_| {}).await.unwrap(), remote);
    let entry = s.stat(&remote).await.unwrap();
    assert_eq!((entry.kind, entry.size), (EntryKind::File, data.len() as u64));
    assert_eq!(entry.mtime, Some(mtime_local(&local)), "mtime kept on upload");
    assert!(!s.exists(&dir.join(".data.bin.dromon-part")).await.unwrap());

    // A folder tree up, then everything down again.
    let site = tmp.path().join("site");
    std::fs::create_dir_all(site.join("sub")).unwrap();
    std::fs::write(site.join("a.txt"), b"a").unwrap();
    std::fs::write(site.join("sub/b.txt"), pattern(70_000, 4)).unwrap();
    let remote_site = dir.join("site");
    let scan = scan_local(&site, &remote_site, false, |_| {}).await.unwrap();
    create_dirs(s, &scan.dirs).await.unwrap();
    for plan in &scan.files {
        upload(s, plan, CopyOptions::default(), |_| {}).await.unwrap();
    }
    let back = tmp.path().join("back");
    std::fs::create_dir(&back).unwrap();
    let scan = scan_remote(s, &dir, &back.join("dir"), |_| {}).await.unwrap();
    assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);
    create_dirs(s, &scan.dirs).await.unwrap();
    for plan in &scan.files {
        download(s, plan, CopyOptions::default(), |_| {}).await.unwrap();
    }
    assert!(std::fs::read(back.join("dir/data.bin")).unwrap() == data, "download differs");
    assert_eq!(mtime_local(&back.join("dir/data.bin")), mtime_local(&local), "mtime kept on download");
    assert_eq!(std::fs::read(back.join("dir/site/sub/b.txt")).unwrap(), pattern(70_000, 4));

    // Renames: to a free name; onto a taken one (refused or replaced, shown); posix-rename
    // replaces when offered.
    let renamed = dir.join("renamed.bin");
    s.rename(&remote, &renamed).await.unwrap();
    assert!(!s.exists(&remote).await.unwrap() && s.exists(&renamed).await.unwrap());
    let other = dir.join("other.txt");
    write_remote(s, &other, b"other").await;
    let onto = s.rename(&other, &renamed).await;
    println!("plain rename onto a taken name: {onto:?}");
    assert!(matches!(onto, Ok(()) | Err(Error::AlreadyExists(_))), "{onto:?}");
    if onto.is_err() && s.capabilities().posix_rename {
        s.posix_rename(&other, &renamed).await.unwrap();
        assert_eq!(read_remote(s, &renamed).await, b"other");
    }

    // Replace on upload, and a same-host copy (copy-data when offered, else streamed).
    let plan = FilePlan::Upload {
        src: local.clone(),
        dst: renamed.clone(),
        size: data.len() as u64,
        mtime: Some(mtime_local(&local)),
        overwrite: true,
    };
    assert_eq!(upload(s, &plan, CopyOptions::default(), |_| {}).await.unwrap(), renamed);
    assert!(read_remote(s, &renamed).await == data);
    let copy = dir.join("copy.bin");
    let plan = the_plan(scan_remote_copy(s, &renamed, &copy, false, |_| {}).await.unwrap());
    let mut progress = Vec::new();
    copy_remote(s, &plan, CopyOptions::default(), |n| progress.push(n)).await.unwrap();
    println!(
        "same-host copy: {} progress reports ({})",
        progress.len(),
        if s.capabilities().copy_data { "copy-data" } else { "streamed" }
    );
    assert!(read_remote(s, &copy).await == data);

    // A copy streamed through one session (servers without copy-data): reads and writes in
    // flight both ways on one ssh.exe, with the app's own 10 s reply timeout. Without WSL in
    // between, unlike the Docker fixture on Windows CI.
    let big = dir.join("big.bin");
    let big_data = pattern(16 << 20, 9);
    write_remote(s, &big, &big_data).await;
    let streamed = CopyOptions { server_copy: false, ..CopyOptions::default() };
    for round in 0..5 {
        let dst = dir.join(&format!("streamed-{round}.bin"));
        let plan = the_plan(scan_remote_copy(s, &big, &dst, false, |_| {}).await.unwrap());
        let started = std::time::Instant::now();
        copy_remote(s, &plan, streamed, |_| {}).await.unwrap_or_else(|e| panic!("streamed copy {round}: {e:?}"));
        println!("streamed same-session copy {round}: 16 MiB in {:.2?}", started.elapsed());
        assert_eq!(s.stat(&dst).await.unwrap().size, 16 << 20);
    }
    assert!(read_remote(s, &dir.join("streamed-4.bin")).await == big_data);

    // Delete it all.
    let removed = s.remove_all(&dir, |_, _| {}).await.unwrap();
    println!("removed {removed} entries");
    assert!(removed >= 7, "{removed}");
    assert!(!s.exists(&dir).await.unwrap());
}
