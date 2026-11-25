use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dromon_core::{Error, RemotePath};

use super::*;
use crate::{DirPlan, FilePlan, Scan};

fn write(path: &Path, len: usize) {
    fs::write(path, vec![7u8; len]).unwrap();
}

fn set_mtime(path: &Path, secs: u64) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

async fn scan_with_counts(src: &Path, dst: &str) -> (Scan, Vec<u64>) {
    let mut counts = Vec::new();
    let scan = scan_local(src, &RemotePath::new(dst), false, |n| counts.push(n)).await.unwrap();
    (scan, counts)
}

fn uploads(scan: &Scan) -> Vec<(PathBuf, String, u64)> {
    scan.files
        .iter()
        .map(|f| match f {
            FilePlan::Upload { src, dst, size, .. } => (src.clone(), dst.to_string(), *size),
            FilePlan::Download { .. } => panic!("a download plan in a local scan"),
        })
        .collect()
}

#[tokio::test]
async fn local_file_is_one_plan_with_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("a.txt");
    write(&f, 42);
    set_mtime(&f, 1_577_934_245);
    let (scan, counts) = scan_with_counts(&f, "/up/a.txt").await;
    assert!(scan.dirs.is_empty() && scan.skipped.is_empty());
    assert_eq!(
        scan.files,
        [FilePlan::Upload {
            src: f.clone(),
            dst: RemotePath::new("/up/a.txt"),
            size: 42,
            mtime: Some(1_577_934_245),
            overwrite: false,
        }]
    );
    assert_eq!(counts, [1]);
    let replace = scan_local(&f, &RemotePath::new("/up/a.txt"), true, |_| {}).await.unwrap();
    assert!(matches!(replace.files[0], FilePlan::Upload { overwrite: true, .. }));
}

#[tokio::test]
async fn local_tree_is_depth_first_in_natural_order() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("site");
    fs::create_dir_all(root.join("b/deep")).unwrap();
    fs::create_dir(root.join("empty")).unwrap();
    write(&root.join("file10.txt"), 10);
    write(&root.join("file2.txt"), 2);
    write(&root.join("A.txt"), 1);
    write(&root.join("b/x.bin"), 3);
    write(&root.join("b/deep/y.bin"), 4);

    let (scan, counts) = scan_with_counts(&root, "/srv/site").await;
    let dirs: Vec<_> = scan.dirs.iter().map(|d| match d {
        DirPlan::Remote(p) => p.to_string(),
        DirPlan::Local(_) => panic!("a local folder in a local scan"),
    }).collect();
    assert_eq!(dirs, ["/srv/site", "/srv/site/b", "/srv/site/b/deep", "/srv/site/empty"]);
    let files = uploads(&scan);
    let dsts: Vec<_> = files.iter().map(|f| f.1.as_str()).collect();
    assert_eq!(
        dsts,
        ["/srv/site/A.txt", "/srv/site/b/deep/y.bin", "/srv/site/b/x.bin", "/srv/site/file2.txt", "/srv/site/file10.txt"]
    );
    assert_eq!(files[1].0, root.join("b").join("deep").join("y.bin"));
    assert_eq!(files[4].2, 10);
    assert!(scan.skipped.is_empty());
    assert_eq!(counts, [5]);
}

#[tokio::test]
async fn found_is_throttled() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..130 {
        write(&tmp.path().join(format!("f{i}")), 0);
    }
    let (scan, counts) = scan_with_counts(tmp.path(), "/x").await;
    assert_eq!(scan.files.len(), 130);
    assert_eq!(counts, [64, 128, 130]);
}

#[cfg(unix)]
#[tokio::test]
async fn local_links_and_specials_follow_the_rules() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("mixed");
    fs::create_dir_all(root.join("dir")).unwrap();
    write(&root.join("dir/inside.txt"), 6);
    write(&root.join("target.txt"), 9);
    set_mtime(&root.join("target.txt"), 1_000_000_000);
    symlink("dir", root.join("link-dir")).unwrap();
    symlink("target.txt", root.join("link-file")).unwrap();
    symlink("nowhere", root.join("dangling")).unwrap();
    let fifo = std::process::Command::new("mkfifo").arg(root.join("pipe")).status().unwrap();
    assert!(fifo.success());

    let (scan, _) = scan_with_counts(&root, "/r/mixed").await;
    let files = uploads(&scan);
    let names: Vec<_> = files.iter().map(|f| f.1.as_str()).collect();
    assert_eq!(names, ["/r/mixed/dir/inside.txt", "/r/mixed/link-file", "/r/mixed/target.txt"]);
    // A link to a file is planned with the target's size and mtime, read through the link.
    assert_eq!(
        scan.files[1],
        FilePlan::Upload {
            src: root.join("link-file"),
            dst: RemotePath::new("/r/mixed/link-file"),
            size: 9,
            mtime: Some(1_000_000_000),
            overwrite: false,
        }
    );
    assert_eq!(scan.skipped, ["mixed/dangling: a broken link", "mixed/link-dir: a link to a folder", "mixed/pipe: a named pipe"]);
    assert_eq!(scan.dirs.len(), 2);

    // The picked item itself is followed when it is a link.
    let (via_link, _) = scan_with_counts(&root.join("link-dir"), "/r/link-dir").await;
    assert_eq!(via_link.dirs, [DirPlan::Remote(RemotePath::new("/r/link-dir"))]);
    assert_eq!(uploads(&via_link)[0].1, "/r/link-dir/inside.txt");
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_local_folder_is_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("top");
    fs::create_dir_all(root.join("locked")).unwrap();
    write(&root.join("locked/secret"), 1);
    write(&root.join("ok.txt"), 1);
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = fs::read_dir(root.join("locked")).is_ok(); // running as root
    let (scan, _) = scan_with_counts(&root, "/t").await;
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        return;
    }
    assert_eq!(scan.skipped, ["top/locked: permission denied"]);
    assert_eq!(scan.dirs, [DirPlan::Remote(RemotePath::new("/t"))]);
    assert_eq!(uploads(&scan).len(), 1);
}

#[tokio::test]
async fn missing_local_source_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("nope");
    let err = scan_local(&missing, &RemotePath::new("/x"), false, |_| {}).await.unwrap_err();
    let Error::Io(msg) = err else { panic!("{err:?}") };
    assert!(msg.starts_with("Couldn't read ") && msg.contains("nope"), "{msg}");
    assert!(!msg.contains("os error"), "{msg}");
}

#[test]
fn name_taken_counts_part_files_and_dangling_links() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert!(!local_name_taken(dir, "a.zip"));
    write(&dir.join("a.zip"), 1);
    assert!(local_name_taken(dir, "a.zip"));
    write(&dir.join("b.zip.dromon-part"), 1);
    assert!(local_name_taken(dir, "b.zip"));
    assert!(!local_name_taken(dir, "b (1).zip"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("nowhere", dir.join("c")).unwrap();
        assert!(local_name_taken(dir, "c"));
    }
    assert_eq!(unique_name("a.zip", |n| local_name_taken(dir, n)), "a (1).zip");
}

#[test]
fn part_paths() {
    let dst = Path::new("downloads").join("a.zip");
    assert_eq!(download::part_path(&dst), Some(Path::new("downloads").join("a.zip.dromon-part")));
    assert_eq!(upload::part_path(&RemotePath::new("/srv/a.zip")), Some(RemotePath::new("/srv/.a.zip.dromon-part")));
    assert_eq!(upload::part_path(&RemotePath::root()), None);
}

#[test]
fn times_round_trip() {
    assert_eq!(secs_of(UNIX_EPOCH + Duration::from_millis(1_577_934_245_900)), 1_577_934_245);
    assert_eq!(secs_of(UNIX_EPOCH - Duration::from_secs(5)), -5);
    for s in [0, 1_577_934_245, -86_400] {
        assert_eq!(time_of(s).map(secs_of), Some(s));
    }
    let now = SystemTime::now();
    assert!(time_of(secs_of(now)).is_some_and(|t| t <= now));
}

#[test]
fn local_errors_read_like_the_ui_wants() {
    let err = std::io::Error::from_raw_os_error(28);
    let Error::Io(msg) = local_err("write", Path::new("/x/y"), &err) else { panic!() };
    #[cfg(target_os = "linux")]
    assert_eq!(msg, "Couldn't write /x/y: No space left on device");
    assert!(msg.starts_with("Couldn't write ") && !msg.contains("os error"), "{msg}");
    let other = std::io::Error::other("boom");
    assert_eq!(os_message(&other), "boom");
}

#[test]
fn default_options() {
    assert_eq!(CopyOptions::default().in_flight, 16);
}
