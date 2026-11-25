//! Recursive scans: one picked item becomes folders to create, files to copy and what was left
//! out, with the same rules on both sides (and for remote-to-remote copies):
//!
//! - depth first, children in natural name order, a folder listed in `dirs` before its
//!   contents (the item itself first when it is a folder);
//! - regular files are planned with their size and mtime; a link to a file is planned as a file
//!   with its target's size and mtime;
//! - links to folders are never followed (no loops), and are skipped like dangling links,
//!   FIFOs, sockets, devices and unreadable folders, each with a reason
//!   ("photos/current: a link to a folder"); the scan goes on;
//! - the picked item itself is followed when it is a link (the user chose it), and errors on it
//!   (not found, permission denied) are returned.
//!
//! Reasons name the path relative to the folder the item was picked from, so they start with
//! the item's own name.

use std::ffi::OsString;
use std::fs::{FileType, Metadata};
use std::path::{Path, PathBuf};

use dromon_core::{Entry, EntryKind, Error, LinkTarget, RemotePath, Result, natural_cmp};
use dromon_sftp::Sftp;

use super::{local_err, os_message, secs_of};
use crate::{DirPlan, FilePlan, Scan};

/// `found` is called at most once per this many files planned, plus once at the end.
const FOUND_EVERY: u64 = 64;

const LINK_TO_DIR: &str = "a link to a folder";
const BROKEN_LINK: &str = "a broken link";
const LINK_TO_SPECIAL: &str = "a link to something that isn't a file or folder";
const DENIED: &str = "permission denied";
const GONE: &str = "disappeared during the scan";
const NOT_UTF8: &str = "the name isn't valid UTF-8";

/// Recursively plans downloading `src` (a file, a folder, or a link the user picked, which is
/// followed) into `dst`, the item's final local root (already free). `found` gets the running
/// number of files planned.
pub async fn scan_remote(
    sftp: &Sftp,
    src: &RemotePath,
    dst: &Path,
    found: impl FnMut(u64) + Send,
) -> Result<Scan> {
    walk_remote(sftp, src, Target::Local(dst.to_owned()), false, found).await
}

/// Recursively plans copying `src` (as [`scan_remote`]: same order, same skip rules) to `dst`,
/// the item's final remote root on the same or another host; `overwrite` goes into each
/// [`FilePlan::Copy`]. Only `sftp`, the source's session, is used: the folders in
/// [`Scan::dirs`] are [`DirPlan::Remote`] paths on the destination, created afterwards.
pub async fn scan_remote_copy(
    sftp: &Sftp,
    src: &RemotePath,
    dst: &RemotePath,
    overwrite: bool,
    found: impl FnMut(u64) + Send,
) -> Result<Scan> {
    walk_remote(sftp, src, Target::Remote(dst.clone()), overwrite, found).await
}

async fn walk_remote(
    sftp: &Sftp,
    src: &RemotePath,
    dst: Target,
    overwrite: bool,
    mut found: impl FnMut(u64) + Send,
) -> Result<Scan> {
    let top = sftp.stat(src).await?;
    let mut notify = |n| {
        found(n);
        true
    };
    let mut walk = Walk::new(&mut notify);
    walk.overwrite = overwrite;
    let root_rel = src.file_name().unwrap_or("/").to_owned();
    match top.kind {
        EntryKind::File => walk.plan_remote(src.clone(), dst, &top),
        EntryKind::Dir => {
            let entries = sftp.list(src).await?;
            walk.scan.dirs.push(dst.dir());
            let mut stack = Vec::new();
            push_remote(&mut stack, entries, src, &dst, &root_rel);
            while let Some(item) = stack.pop() {
                walk_remote_item(sftp, &mut walk, &mut stack, item).await?;
            }
        }
        _ => walk.skip(&root_rel, special_kind(top.mode)),
    }
    Ok(walk.finish())
}

/// Where a remote scan plans its files: a download's local root or a copy's remote one.
#[derive(Clone)]
enum Target {
    Local(PathBuf),
    Remote(RemotePath),
}

impl Target {
    fn join(&self, name: &str) -> Target {
        match self {
            Target::Local(p) => Target::Local(p.join(name)),
            Target::Remote(p) => Target::Remote(p.join(name)),
        }
    }

    fn dir(&self) -> DirPlan {
        match self {
            Target::Local(p) => DirPlan::Local(p.clone()),
            Target::Remote(p) => DirPlan::Remote(p.clone()),
        }
    }
}

/// A remote child waiting on the walk's stack.
struct RemoteChild {
    entry: Entry,
    src: RemotePath,
    dst: Target,
    rel: String,
}

async fn walk_remote_item(
    sftp: &Sftp,
    walk: &mut Walk<'_>,
    stack: &mut Vec<RemoteChild>,
    item: RemoteChild,
) -> Result<()> {
    match item.entry.kind {
        EntryKind::File => walk.plan_remote(item.src, item.dst, &item.entry),
        EntryKind::Dir => match sftp.list(&item.src).await {
            Ok(entries) => {
                walk.scan.dirs.push(item.dst.dir());
                push_remote(stack, entries, &item.src, &item.dst, &item.rel);
            }
            Err(Error::PermissionDenied(_)) => walk.skip(&item.rel, DENIED),
            Err(Error::NotFound(_)) => walk.skip(&item.rel, GONE),
            Err(e) => return Err(e),
        },
        EntryKind::Symlink { target: Some(LinkTarget::File) } => match sftp.stat(&item.src).await {
            Ok(target) if target.kind == EntryKind::File => walk.plan_remote(item.src, item.dst, &target),
            Ok(_) => walk.skip(&item.rel, LINK_TO_SPECIAL),
            Err(Error::NotFound(_) | Error::PermissionDenied(_)) => walk.skip(&item.rel, BROKEN_LINK),
            Err(e) => return Err(e),
        },
        EntryKind::Symlink { target: Some(LinkTarget::Dir) } => walk.skip(&item.rel, LINK_TO_DIR),
        EntryKind::Symlink { target: Some(LinkTarget::Other) } => walk.skip(&item.rel, LINK_TO_SPECIAL),
        EntryKind::Symlink { target: None } => walk.skip(&item.rel, BROKEN_LINK),
        EntryKind::Other => walk.skip(&item.rel, special_kind(item.entry.mode)),
    }
    Ok(())
}

/// Pushes a folder's children so they pop in natural name order.
fn push_remote(stack: &mut Vec<RemoteChild>, mut entries: Vec<Entry>, src: &RemotePath, dst: &Target, rel: &str) {
    entries.sort_by(|a, b| natural_cmp(&b.name, &a.name));
    for entry in entries {
        let rel = if rel.ends_with('/') { format!("{rel}{}", entry.name) } else { format!("{rel}/{}", entry.name) };
        stack.push(RemoteChild { src: src.join(&entry.name), dst: dst.join(&entry.name), rel, entry });
    }
}

/// What a non-file, non-folder entry is, from its `st_mode` type bits.
fn special_kind(mode: Option<u32>) -> &'static str {
    match mode.map(|m| m & 0o170_000) {
        Some(0o010_000) => "a named pipe",
        Some(0o140_000) => "a socket",
        Some(0o020_000 | 0o060_000) => "a device",
        _ => "not a regular file",
    }
}

/// Recursively plans uploading local `src` (followed when it is a link) into remote `dst`, the
/// item's final remote root; `overwrite` goes into each [`FilePlan::Upload`]. The walk runs on
/// a blocking thread and stops soon after this future is dropped.
pub async fn scan_local(
    src: &Path,
    dst: &RemotePath,
    overwrite: bool,
    mut found: impl FnMut(u64) + Send,
) -> Result<Scan> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (src_owned, dst_owned) = (src.to_owned(), dst.clone());
    let task = tokio::task::spawn_blocking(move || {
        walk_local(&src_owned, &dst_owned, overwrite, &mut |n| tx.send(n).is_ok())
    });
    while let Some(n) = rx.recv().await {
        found(n);
    }
    task.await.map_err(|e| Error::Other(format!("The scan of {} stopped: {e}", src.display())))?
}

/// A local child waiting on the walk's stack.
struct LocalChild {
    path: PathBuf,
    dst: RemotePath,
    rel: PathBuf,
}

/// The blocking local walk; `found` returning false (nobody listens any more) stops it.
fn walk_local(
    src: &Path,
    dst: &RemotePath,
    overwrite: bool,
    found: &mut (dyn FnMut(u64) -> bool + Send),
) -> Result<Scan> {
    let meta = std::fs::metadata(src).map_err(|e| local_err("read", src, &e))?;
    let root_rel = PathBuf::from(src.file_name().unwrap_or(src.as_os_str()));
    let mut walk = Walk::new(found);
    if meta.is_file() {
        walk.plan_upload(src.to_owned(), dst.clone(), &meta, overwrite);
    } else if meta.is_dir() {
        let names = read_names(src).map_err(|e| local_err("list", src, &e))?;
        walk.scan.dirs.push(DirPlan::Remote(dst.clone()));
        let mut stack = Vec::new();
        push_local(&mut walk, &mut stack, names, src, dst, &root_rel);
        while let Some(item) = stack.pop() {
            walk_local_item(&mut walk, &mut stack, item, overwrite);
            if !walk.listening() {
                return Err(Error::Other("The scan was stopped".into()));
            }
        }
    } else {
        walk.skip(&root_rel.display().to_string(), special_local(meta.file_type()));
    }
    Ok(walk.finish())
}

fn walk_local_item(walk: &mut Walk<'_>, stack: &mut Vec<LocalChild>, item: LocalChild, overwrite: bool) {
    let rel = item.rel.display().to_string();
    let meta = match std::fs::symlink_metadata(&item.path) {
        Ok(m) => m,
        Err(e) => return walk.skip(&rel, &io_reason(&e)),
    };
    let kind = meta.file_type();
    if kind.is_symlink() {
        match std::fs::metadata(&item.path) {
            Ok(t) if t.is_dir() => walk.skip(&rel, LINK_TO_DIR),
            Ok(t) if t.is_file() => walk.plan_upload(item.path, item.dst, &t, overwrite),
            Ok(_) => walk.skip(&rel, LINK_TO_SPECIAL),
            Err(_) => walk.skip(&rel, BROKEN_LINK),
        }
    } else if kind.is_dir() {
        match read_names(&item.path) {
            Ok(names) => {
                walk.scan.dirs.push(DirPlan::Remote(item.dst.clone()));
                push_local(walk, stack, names, &item.path, &item.dst, &item.rel);
            }
            Err(e) => walk.skip(&rel, &io_reason(&e)),
        }
    } else if kind.is_file() {
        walk.plan_upload(item.path, item.dst, &meta, overwrite);
    } else {
        walk.skip(&rel, special_local(kind));
    }
}

/// The names in a local folder.
fn read_names(dir: &Path) -> std::io::Result<Vec<OsString>> {
    std::fs::read_dir(dir)?.map(|e| e.map(|e| e.file_name())).collect()
}

/// Pushes a local folder's children so they pop in natural name order; names that can't be
/// remote names (not UTF-8) are skipped here.
fn push_local(
    walk: &mut Walk<'_>,
    stack: &mut Vec<LocalChild>,
    names: Vec<OsString>,
    dir: &Path,
    dst: &RemotePath,
    rel: &Path,
) {
    let mut named: Vec<(String, OsString)> = Vec::with_capacity(names.len());
    for os in names {
        match os.to_str() {
            Some(s) => named.push((s.to_owned(), os)),
            None => walk.skip(&rel.join(&os).display().to_string(), NOT_UTF8),
        }
    }
    named.sort_by(|a, b| natural_cmp(&b.0, &a.0));
    for (name, os) in named {
        stack.push(LocalChild { path: dir.join(&os), dst: dst.join(&name), rel: rel.join(&os) });
    }
}

fn io_reason(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => DENIED.to_owned(),
        std::io::ErrorKind::NotFound => GONE.to_owned(),
        _ => os_message(err),
    }
}

#[cfg(unix)]
fn special_local(kind: FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    if kind.is_fifo() {
        "a named pipe"
    } else if kind.is_socket() {
        "a socket"
    } else if kind.is_block_device() || kind.is_char_device() {
        "a device"
    } else {
        "not a regular file"
    }
}

#[cfg(not(unix))]
fn special_local(_kind: FileType) -> &'static str {
    "not a regular file"
}

/// The scan being built, with the throttled `found` count. `found` returns false once nobody
/// listens any more.
struct Walk<'a> {
    scan: Scan,
    files: u64,
    found: &'a mut (dyn FnMut(u64) -> bool + Send),
    stopped: bool,
    /// A remote-to-remote copy's Replace, for its plans.
    overwrite: bool,
}

impl<'a> Walk<'a> {
    fn new(found: &'a mut (dyn FnMut(u64) -> bool + Send)) -> Self {
        Walk { scan: Scan::default(), files: 0, found, stopped: false, overwrite: false }
    }

    fn plan(&mut self, plan: FilePlan) {
        self.scan.files.push(plan);
        self.files += 1;
        if self.files.is_multiple_of(FOUND_EVERY) && !(self.found)(self.files) {
            self.stopped = true;
        }
    }

    fn plan_remote(&mut self, src: RemotePath, dst: Target, entry: &Entry) {
        let (size, mtime) = (entry.size, entry.mtime);
        self.plan(match dst {
            Target::Local(dst) => FilePlan::Download { src, dst, size, mtime },
            Target::Remote(dst) => FilePlan::Copy { src, dst, size, mtime, overwrite: self.overwrite },
        });
    }

    fn plan_upload(&mut self, src: PathBuf, dst: RemotePath, meta: &Metadata, overwrite: bool) {
        let mtime = meta.modified().ok().map(secs_of);
        self.plan(FilePlan::Upload { src, dst, size: meta.len(), mtime, overwrite });
    }

    fn skip(&mut self, rel: &str, reason: &str) {
        self.scan.skipped.push(format!("{rel}: {reason}"));
    }

    fn listening(&self) -> bool {
        !self.stopped
    }

    fn finish(self) -> Scan {
        let _ = (self.found)(self.files);
        self.scan
    }
}
