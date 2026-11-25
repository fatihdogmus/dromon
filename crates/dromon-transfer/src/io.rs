//! Scans and single-file copies between the local disk and an SFTP server: what the transfer
//! engine runs inside its tokio tasks (`.plans/dromon-plan.md` §3.5, §3.6, §4.6).
//!
//! - Scans ([`scan_remote`], [`scan_local`]) walk one picked item depth first and turn it into
//!   folders to create, files to copy and what was left out (links to folders, special files,
//!   unreadable folders), see `scan`.
//! - Copies ([`download`], [`upload`]) write a part file next to the destination, keep
//!   [`CopyOptions::in_flight`] requests outstanding, preserve the mtime and rename the part
//!   into place once complete, see `download` and `upload`.
//!
//! **Cancel safety.** The engine pauses or cancels a copy by dropping its future. At every await
//! point the destination name holds either nothing of ours or the finished file: everything in
//! between lives in the part file. Bytes are appended to the part strictly in order, so its
//! length is always a verified prefix of the source and the next call resumes from it (when the
//! source still has the planned size and mtime). [`remove_part`] discards it.
//!
//! Local errors become [`Error::Io`] with the path and the OS message ("Couldn't write
//! /x/y: No space left on device"); remote errors keep `dromon-sftp`'s mapping, where
//! [`Error::ConnectionLost`] means "interrupted, resume later".

mod download;
mod scan;
mod upload;

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dromon_core::{EntryKind, Error, LinkTarget, RemotePath, Result};
use dromon_sftp::Sftp;

use crate::{DirPlan, FilePlan, download_part_name, unique_name, upload_part_name};

pub use self::download::download;
pub use self::scan::{scan_local, scan_remote};
pub use self::upload::upload;

/// How a single file is copied.
#[derive(Clone, Copy, Debug)]
pub struct CopyOptions {
    /// Read (download) or write (upload) requests kept outstanding per file.
    pub in_flight: usize,
}

impl Default for CopyOptions {
    fn default() -> Self {
        CopyOptions { in_flight: 16 }
    }
}

/// Creates the scan's folders, parents first. Local: `create_dir_all`. Remote: `mkdir`, fine if
/// a folder (or a link to one) is already there; anything else in the way is
/// [`Error::AlreadyExists`].
pub async fn create_dirs(sftp: &Sftp, dirs: &[DirPlan]) -> Result<()> {
    for dir in dirs {
        match dir {
            DirPlan::Local(path) => {
                if let Err(e) = tokio::fs::create_dir_all(path).await {
                    let in_the_way = tokio::fs::metadata(path).await.is_ok_and(|m| !m.is_dir());
                    return Err(if in_the_way {
                        Error::Io(format!(
                            "Couldn't create the folder {}: a file with that name is in the way",
                            path.display()
                        ))
                    } else {
                        local_err("create the folder", path, &e)
                    });
                }
            }
            DirPlan::Remote(path) => match sftp.mkdir(path).await {
                Ok(()) => {}
                Err(Error::AlreadyExists(_)) => {
                    if remote_kind(sftp, path).await? != Some(true) {
                        return Err(Error::AlreadyExists(path.clone()));
                    }
                }
                Err(e) => return Err(e),
            },
        }
    }
    Ok(())
}

/// Deletes a plan's part file if present (on cancel). Best effort: errors are ignored.
pub async fn remove_part(sftp: &Sftp, plan: &FilePlan) {
    match plan {
        FilePlan::Download { dst, .. } => {
            if let Some(part) = download::part_path(dst) {
                let _ = tokio::fs::remove_file(part).await;
            }
        }
        FilePlan::Upload { dst, .. } => {
            if let Some(part) = upload::part_path(dst) {
                let _ = sftp.remove_file(&part).await;
            }
        }
    }
}

/// Whether `name` (or its download part file) exists in local `dir`: the "taken" test for
/// picking a free local name. A dangling symlink counts as taken.
pub fn local_name_taken(dir: &Path, name: &str) -> bool {
    let exists = |n: &str| dir.join(n).symlink_metadata().is_ok();
    exists(name) || exists(&download_part_name(name))
}

/// What is at a remote path: `None` (nothing), `Some(true)` a folder (or a link to one),
/// `Some(false)` anything else (including a dangling link).
pub async fn remote_kind(sftp: &Sftp, path: &RemotePath) -> Result<Option<bool>> {
    match sftp.lstat(path).await {
        Ok(entry) => Ok(Some(matches!(
            entry.kind,
            EntryKind::Dir | EntryKind::Symlink { target: Some(LinkTarget::Dir) }
        ))),
        Err(Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A free name for `name` in remote `folder`: one listing, then [`unique_name`]. A name whose
/// upload part file exists also counts as taken.
pub async fn free_remote_name(sftp: &Sftp, folder: &RemotePath, name: &str) -> Result<String> {
    let taken: HashSet<String> = sftp.list(folder).await?.into_iter().map(|e| e.name).collect();
    Ok(unique_name(name, |n| taken.contains(n) || taken.contains(&upload_part_name(n))))
}

/// A local error as the UI shows it: "Couldn't `verb` `path`: `OS message`".
pub(crate) fn local_err(verb: &str, path: &Path, err: &std::io::Error) -> Error {
    Error::Io(format!("Couldn't {verb} {}: {}", path.display(), os_message(err)))
}

/// The OS's description of `err` without Rust's " (os error N)" suffix.
fn os_message(err: &std::io::Error) -> String {
    let text = err.to_string();
    match err.raw_os_error() {
        Some(code) => text.strip_suffix(&format!(" (os error {code})")).unwrap_or(&text).to_owned(),
        None => text,
    }
}

/// The error for a remote source that shrank under a running download.
fn changed_on_server(name: &str) -> Error {
    Error::Io(format!("{name} changed on the server while downloading"))
}

/// The error for a local source that shrank under a running upload.
fn changed_locally(name: &str) -> Error {
    Error::Io(format!("{name} changed while uploading"))
}

/// Seconds since the Unix epoch (negative before it), as SFTP carries mtimes.
fn secs_of(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// The inverse of [`secs_of`]; `None` when the platform can't represent it.
fn time_of(secs: i64) -> Option<SystemTime> {
    if secs >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(secs.unsigned_abs()))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(secs.unsigned_abs()))
    }
}

#[cfg(test)]
#[path = "io_tests.rs"]
mod tests;
