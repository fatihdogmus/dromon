//! The types every part of the transfer engine shares: what the user asks for
//! ([`JobRequest`]), one file's copy ([`FilePlan`]), what a scan finds ([`Scan`]), the answer to
//! a name clash at a remote destination ([`ConflictChoice`]) and what the UI shows of a job
//! ([`JobView`]).
//!
//! The engine (`.plans/dromon-plan.md` §4.6) is three parts:
//! - `queue`: the pure model of every job and file, with the per-host scheduler;
//! - `io`: scans, file copies and moves over SFTP and the local disk (part files, resume, mtime);
//! - the engine task that feeds the queue's decisions to `io` and `io`'s results back.

use std::path::PathBuf;
use std::time::Duration;

use dromon_core::{EntryKind, HostId, RemotePath};

use crate::{JobKind, Progress, TaskState};

/// A job: one user action ("download these 3 items"). Ids are never reused in a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

/// One file of a job. Unique within its job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// A remote item the user picked, as the listing showed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteItem {
    pub path: RemotePath,
    pub kind: EntryKind,
    pub size: u64,
}

/// What the user asked for (`.plans/dromon-plan.md` §3.5–§3.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobRequest {
    /// Download remote files and folders (recursively) into a local folder.
    Download { host: HostId, items: Vec<RemoteItem>, into: PathBuf },
    /// Upload local files and folders (recursively) into a remote folder.
    Upload { host: HostId, items: Vec<PathBuf>, into: RemotePath },
    /// Copy remote items into a folder on the same host: server-side `copy-data` when the server offers it, else streamed through the app.
    RemoteCopy { host: HostId, items: Vec<RemoteItem>, into: RemotePath },
    /// Move remote items into a folder on the same host with `posix-rename` (nothing is copied).
    Move { host: HostId, items: Vec<RemoteItem>, into: RemotePath },
    /// Copy remote items to a folder on another host, streamed through the app.
    CrossHostCopy { from: HostId, items: Vec<RemoteItem>, to: HostId, into: RemotePath },
    /// As CrossHostCopy, then each file's size is checked on `to` and the source deleted (folders once empty).
    CrossHostMove { from: HostId, items: Vec<RemoteItem>, to: HostId, into: RemotePath },
}

impl JobRequest {
    pub fn kind(&self) -> JobKind {
        match self {
            JobRequest::Download { .. } => JobKind::Download,
            JobRequest::Upload { .. } => JobKind::Upload,
            JobRequest::RemoteCopy { .. } => JobKind::RemoteCopy,
            JobRequest::Move { .. } => JobKind::Move,
            JobRequest::CrossHostCopy { .. } => JobKind::CrossHostCopy,
            JobRequest::CrossHostMove { .. } => JobKind::CrossHostMove,
        }
    }

    /// The host the job is listed under: its only host, or the destination (`to`) of a
    /// cross-host job.
    pub fn host(&self) -> &HostId {
        match self {
            JobRequest::Download { host, .. }
            | JobRequest::Upload { host, .. }
            | JobRequest::RemoteCopy { host, .. }
            | JobRequest::Move { host, .. } => host,
            JobRequest::CrossHostCopy { to, .. } | JobRequest::CrossHostMove { to, .. } => to,
        }
    }

    /// Every host the job talks to, the source first: one for most jobs, `[from, to]` for a
    /// cross-host one (one when both are the same host).
    pub fn hosts(&self) -> Vec<&HostId> {
        match self {
            JobRequest::CrossHostCopy { from, to, .. } | JobRequest::CrossHostMove { from, to, .. } if from != to => {
                vec![from, to]
            }
            _ => vec![self.host()],
        }
    }

    /// The host the items are read from (the only host, or `from`).
    pub fn source_host(&self) -> &HostId {
        match self {
            JobRequest::CrossHostCopy { from, .. } | JobRequest::CrossHostMove { from, .. } => from,
            _ => self.host(),
        }
    }

    /// How many top-level items the user picked.
    pub fn item_count(&self) -> usize {
        match self {
            JobRequest::Upload { items, .. } => items.len(),
            _ => self.remote_items().map_or(0, <[RemoteItem]>::len),
        }
    }

    /// The picked remote items: everything but an upload's.
    pub fn remote_items(&self) -> Option<&[RemoteItem]> {
        match self {
            JobRequest::Download { items, .. }
            | JobRequest::RemoteCopy { items, .. }
            | JobRequest::Move { items, .. }
            | JobRequest::CrossHostCopy { items, .. }
            | JobRequest::CrossHostMove { items, .. } => Some(items),
            JobRequest::Upload { .. } => None,
        }
    }

    /// The remote folder the items go into: everything but a download's.
    pub fn remote_into(&self) -> Option<&RemotePath> {
        match self {
            JobRequest::Upload { into, .. }
            | JobRequest::RemoteCopy { into, .. }
            | JobRequest::Move { into, .. }
            | JobRequest::CrossHostCopy { into, .. }
            | JobRequest::CrossHostMove { into, .. } => Some(into),
            JobRequest::Download { .. } => None,
        }
    }

    /// The remote folders the job changes, by host, without repeats: the folder the items go
    /// into, and for a move the folders they leave. A download changes none.
    pub fn changed_folders(&self) -> Vec<(HostId, RemotePath)> {
        let mut out: Vec<(HostId, RemotePath)> = Vec::new();
        if let Some(into) = self.remote_into() {
            out.push((self.host().clone(), into.clone()));
        }
        if let JobRequest::Move { items, .. } | JobRequest::CrossHostMove { items, .. } = self {
            for parent in items.iter().filter_map(|i| i.path.parent()) {
                let at = (self.source_host().clone(), parent);
                if !out.contains(&at) {
                    out.push(at);
                }
            }
        }
        out
    }
}

/// One file to copy, decided by a scan. `src` and `dst` are final paths: conflicts have been
/// settled before the plan exists (download: a Chrome-style free name; upload: the user's
/// [`ConflictChoice`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePlan {
    Download {
        src: RemotePath,
        /// Where the file ends up; the part file is next to it ([`crate::download_part_name`]).
        dst: PathBuf,
        /// Size and mtime when scanned: a resume only continues a part file if the source still
        /// has them.
        size: u64,
        mtime: Option<i64>,
    },
    Upload {
        src: PathBuf,
        /// Where the file ends up; the part file is next to it ([`crate::upload_part_name`]).
        dst: RemotePath,
        size: u64,
        mtime: Option<i64>,
        /// The user chose Replace: an existing `dst` is overwritten. Otherwise a file that
        /// appeared at `dst` since the scan gets a free name instead.
        overwrite: bool,
    },
    /// RemoteCopy, CrossHostCopy and CrossHostMove: one remote file to a remote path (on the
    /// same or the destination host), through the part file next to `dst`
    /// ([`crate::upload_part_name`]). `overwrite` as for uploads.
    Copy { src: RemotePath, dst: RemotePath, size: u64, mtime: Option<i64>, overwrite: bool },
    /// Move (same host): one picked item, file or folder, renamed to `dst` as a whole.
    /// `overwrite` = the user chose Replace: a file is replaced, a folder merged.
    Move { src: RemotePath, dst: RemotePath, size: u64, overwrite: bool },
}

impl FilePlan {
    pub fn size(&self) -> u64 {
        match self {
            FilePlan::Download { size, .. }
            | FilePlan::Upload { size, .. }
            | FilePlan::Copy { size, .. }
            | FilePlan::Move { size, .. } => *size,
        }
    }

    /// The file's name, for the UI and error messages.
    pub fn name(&self) -> String {
        match self {
            FilePlan::Download { src, .. } | FilePlan::Copy { src, .. } | FilePlan::Move { src, .. } => {
                src.file_name().unwrap_or("/").to_owned()
            }
            FilePlan::Upload { src, .. } => src.file_name().map_or_else(|| src.display().to_string(), |n| n.to_string_lossy().into_owned()),
        }
    }
}

/// A folder a job creates (empty folders too), parents before children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirPlan {
    Local(PathBuf),
    Remote(RemotePath),
}

/// What scanning one top-level item found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Folders to create first, parents before children (the item itself when it is a folder).
    pub dirs: Vec<DirPlan>,
    /// Files in a stable order (depth first, names in natural order).
    pub files: Vec<FilePlan>,
    /// What was left out and why ("logs/current: a link to a folder"), for the job's details.
    pub skipped: Vec<String>,
}

/// The user's answer to "`name` already exists in `folder`" for an upload, copy or move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Overwrite (a folder is merged: same-named files inside are overwritten).
    Replace,
    /// Upload under a free name: `name (1).ext`.
    KeepBoth,
    /// Leave this item out.
    Skip,
}

/// What is known of one of the two clashing items. Both parts are `None` when unknown, and the
/// size is always `None` for a folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Side {
    pub size: Option<u64>,
    /// Seconds since the Unix epoch.
    pub mtime: Option<i64>,
}

/// What is already at a destination name: what a clash is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Existing {
    /// A folder (or a link to one); anything else, even a dangling link, is `false`.
    pub is_dir: bool,
    pub side: Side,
}

/// A question a job is waiting on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictView {
    /// The clashing item's name.
    pub name: String,
    /// The folder it clashes in.
    pub folder: RemotePath,
    /// Whether the existing item is a folder.
    pub existing_is_dir: bool,
    /// The item already at the destination.
    pub existing: Side,
    /// The item being transferred.
    pub incoming: Side,
    /// How many more clashes this job has after this one (for "Apply to all").
    pub more: usize,
}

/// What the UI shows of a job: one row of the transfers popover.
#[derive(Clone, Debug, PartialEq)]
pub struct JobView {
    pub id: JobId,
    pub kind: JobKind,
    /// [`JobRequest::host`]: the destination host of a cross-host job.
    pub host: HostId,
    /// "report.pdf", "photos", or "3 items".
    pub title: String,
    pub state: TaskState,
    /// Bytes over every file found so far.
    pub progress: Progress,
    pub files_done: u64,
    pub files_total: u64,
    /// Bytes per second, once known.
    pub speed: Option<f64>,
    /// Time left, shown only after 2 s of data (`crate::eta`).
    pub eta: Option<Duration>,
    /// Paused because the connection dropped (not by the user): resumes by itself.
    pub waiting_for_connection: bool,
    /// Set while the job waits for the user to settle a name clash.
    pub conflict: Option<ConflictView>,
    /// Where the result is, for "Show in Finder" (download) or the folder (upload, copy, move:
    /// on the destination host): the first top-level item's final path.
    pub reveal: Option<Reveal>,
    /// Files that failed, with their errors (the job is `Failed` once nothing else is left).
    pub errors: Vec<String>,
    pub skipped: Vec<String>,
    /// [`JobRequest::changed_folders`]: the folders to list again once the job is over.
    pub folders: Vec<(HostId, RemotePath)>,
}

/// Something to show when a finished row is clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reveal {
    Local(PathBuf),
    Remote(HostId, RemotePath),
}
