//! One job of the [`Queue`](super::Queue): its items on their way from "picked" to "scanned",
//! its files, and the rules that move it on (the same-host shortcuts, name clashes, tidying a
//! cross-host move, finishing). Pure like the queue, which owns the jobs, takes the engine's
//! facts and schedules the files across hosts (`queue.rs`).

use std::path::Path;
use std::time::Duration;

use dromon_core::{EntryKind, HostId};

use super::{Action, ItemDst};
use crate::{
    ConflictChoice, ConflictView, Existing, FileId, FilePlan, JobId, JobKind, JobRequest, Reveal, Side, SpeedMeter,
    TaskEvent, TaskState, step,
};

/// Where one top-level item is on its way from "picked" to "scanned".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ItemState {
    /// `Name` was (or will be) emitted; waiting for `named` (download) or `exists` (the others).
    Naming,
    /// The name is taken at the destination; waiting for the user.
    Conflict {
        existing: Existing,
        incoming: Side,
    },
    /// KeepBoth: `FreeRemoteName` was emitted; waiting for `named`.
    Renaming,
    /// `Scan` was (or will be) emitted; waiting for `scanned`.
    Scanning,
    Scanned,
    /// Cross-host move: `Tidy` was (or will be) emitted; waiting for `tidied`. The job doesn't
    /// finish before it.
    Tidying,
    Skipped,
    Failed,
}

impl ItemState {
    /// Nothing more will be found for the item (until a retry).
    pub(super) fn is_settled(&self) -> bool {
        matches!(self, Self::Scanned | Self::Tidying | Self::Skipped | Self::Failed)
    }
}

#[derive(Debug)]
pub(super) struct Item {
    pub(super) state: ItemState,
    pub(super) dst: Option<ItemDst>,
    /// Files found so far (while scanning) or in total (once scanned).
    pub(super) found: u64,
    /// Cross-host move: the source folders were tidied since its files last settled.
    pub(super) tidied: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileState {
    Pending,
    Running,
    Done,
    /// The error text lives in the job's `errors`.
    Failed,
}

#[derive(Debug)]
pub(super) struct FileEntry {
    pub(super) plan: FilePlan,
    pub(super) state: FileState,
    /// Bytes done, absolute (includes a resumed offset); kept across pause and interruption.
    pub(super) done: u64,
    /// The top-level item the file belongs to.
    pub(super) item: usize,
}

/// Where a remote item goes relative to where it is, for same-host jobs.
enum OwnFolder {
    /// Into the folder it is already in.
    Same,
    /// Into itself or one of its own subfolders.
    Inside,
}

#[derive(Debug)]
pub(super) struct Job {
    pub(super) id: JobId,
    pub(super) req: JobRequest,
    /// [`JobRequest::hosts`]: every host the job needs up.
    pub(super) hosts: Vec<HostId>,
    /// [`JobRequest::host`]: the view's host and the one `Reveal::Remote` names.
    pub(super) dest: HostId,
    pub(super) state: TaskState,
    pub(super) items: Vec<Item>,
    /// `FileId(n)` is index `n`.
    pub(super) files: Vec<FileEntry>,
    pub(super) errors: Vec<String>,
    pub(super) skipped: Vec<String>,
    /// Set by "apply to all": answers every later name clash.
    pub(super) apply_all: Option<ConflictChoice>,
    /// Paused while Preparing: resume goes back to Preparing, not Running.
    pub(super) paused_preparing: bool,
    /// Work parked by facts, handed out by the next `poll` (while Preparing or Running, hosts up).
    pub(super) outbox: Vec<Action>,
    pub(super) meter: SpeedMeter,
    pub(super) first_progress: Option<Duration>,
}

impl Job {
    pub(super) fn item_name(&self, item: usize) -> String {
        match &self.req {
            JobRequest::Upload { items, .. } => items.get(item).map_or_else(String::new, |p| local_name(p)),
            req => req
                .remote_items()
                .and_then(|items| items.get(item))
                .and_then(|i| i.path.file_name())
                .unwrap_or("/")
                .to_owned(),
        }
    }

    pub(super) fn files_found(&self) -> u64 {
        self.items.iter().map(|i| i.found).sum()
    }

    pub(super) fn bytes_done(&self) -> u64 {
        self.files.iter().map(|f| f.done).sum()
    }

    pub(super) fn all_settled(&self) -> bool {
        self.items.iter().all(|i| i.state.is_settled())
    }

    fn tidying(&self) -> bool {
        self.items.iter().any(|i| i.state == ItemState::Tidying)
    }

    pub(super) fn reset_speed(&mut self) {
        self.meter.reset();
        self.first_progress = None;
    }

    pub(super) fn record_speed(&mut self, now: Duration) {
        self.first_progress.get_or_insert(now);
        let done = self.bytes_done();
        self.meter.record(now, done);
    }

    /// Same-host jobs (and a cross-host job whose two hosts are one): is the item headed for
    /// its own folder, or into itself?
    fn own_folder(&self, item: usize) -> Option<OwnFolder> {
        if self.req.source_host() != self.req.host() {
            return None;
        }
        let into = self.req.remote_into()?;
        let it = self.req.remote_items()?.get(item)?;
        if into.starts_with(&it.path) {
            Some(OwnFolder::Inside)
        } else if it.path.parent().as_ref() == Some(into) {
            Some(OwnFolder::Same)
        } else {
            None
        }
    }

    /// What happens first to an item (on start, and again on retry): usually `Name`. Same-host
    /// rules settle some at once: a copy into its own folder keeps both without asking, a move
    /// there is skipped ("already there"), and a folder into itself fails.
    pub(super) fn first_step(&mut self, item: usize) -> Option<Action> {
        let is_move = matches!(self.req.kind(), JobKind::Move | JobKind::CrossHostMove);
        match self.own_folder(item) {
            None => Some(Action::Name { job: self.id, item }),
            Some(OwnFolder::Inside) => {
                let verb = if is_move { "move" } else { "copy" };
                self.fail_item(item, &format!("can't {verb} a folder into itself"));
                None
            }
            Some(OwnFolder::Same) if is_move => {
                self.items[item].state = ItemState::Skipped;
                let name = self.item_name(item);
                self.skipped.push(format!("{name}: already there"));
                None
            }
            Some(OwnFolder::Same) => {
                self.items[item].state = ItemState::Renaming;
                Some(Action::FreeRemoteName { job: self.id, item })
            }
        }
    }

    pub(super) fn fail_item(&mut self, item: usize, error: &str) {
        self.items[item].state = ItemState::Failed;
        let line = format!("{}: {error}", self.item_name(item));
        self.errors.push(line);
    }

    /// Item gets its destination and the scan is queued for the next poll. A same-host move
    /// needs no scan: its one plan (the whole item, renamed) is ready at once.
    pub(super) fn begin_scan(&mut self, item: usize, dst: ItemDst) {
        self.items[item].dst = Some(dst.clone());
        if let (JobRequest::Move { items, .. }, ItemDst::Remote { path, overwrite }) = (&self.req, &dst)
            && let Some(it) = items.get(item)
        {
            let size = if it.kind.is_dir_like() { 0 } else { it.size };
            let plan = FilePlan::Move { src: it.path.clone(), dst: path.clone(), size, overwrite: *overwrite };
            self.files.push(FileEntry { plan, state: FileState::Pending, done: 0, item });
            self.items[item].state = ItemState::Scanned;
            self.items[item].found = 1;
            return;
        }
        self.items[item].state = ItemState::Scanning;
        self.outbox.push(Action::Scan { job: self.id, item, dst });
    }

    /// Applies the user's answer to a name clash of an item headed for a remote folder.
    pub(super) fn apply_choice(&mut self, item: usize, choice: ConflictChoice) {
        let Some(into) = self.req.remote_into() else { return };
        let name = self.item_name(item);
        let path = into.join(&name);
        match choice {
            ConflictChoice::Replace => {
                self.begin_scan(item, ItemDst::Remote { path, overwrite: true });
            }
            ConflictChoice::KeepBoth => {
                self.items[item].state = ItemState::Renaming;
                self.outbox.push(Action::FreeRemoteName { job: self.id, item });
            }
            ConflictChoice::Skip => {
                self.items[item].state = ItemState::Skipped;
                self.skipped.push(name);
            }
        }
    }

    /// Cross-host move: once every file of a scanned folder item is settled, its source folders
    /// are tidied (the job waits for it).
    pub(super) fn maybe_tidy(&mut self, item: usize) {
        if self.req.kind() != JobKind::CrossHostMove {
            return;
        }
        let is_dir = self.req.remote_items().and_then(|i| i.get(item)).is_some_and(|i| i.kind == EntryKind::Dir);
        if !is_dir || !self.items.get(item).is_some_and(|i| i.state == ItemState::Scanned && !i.tidied) {
            return;
        }
        if self.files.iter().any(|f| f.item == item && matches!(f.state, FileState::Pending | FileState::Running)) {
            return;
        }
        self.items[item].state = ItemState::Tidying;
        self.outbox.push(Action::Tidy { job: self.id, item });
    }

    /// While Preparing and every item is settled: start the files, or finish a job with none.
    pub(super) fn advance(&mut self) {
        if !matches!(self.state, TaskState::Preparing { .. }) || !self.all_settled() {
            return;
        }
        if !self.files.is_empty() {
            if let Some(s) = step(&self.state, TaskEvent::ScanDone) {
                self.state = s;
            }
        } else if self.tidying() {
            // Finishes once the source folders are tidied.
        } else if let Some(error) = self.errors.first().cloned() {
            if let Some(s) = step(&self.state, TaskEvent::Fail(error)) {
                self.state = s;
            }
        } else {
            // `step` has no Preparing → Done edge (nothing was found, nothing to run).
            self.state = TaskState::Done;
        }
    }

    /// A Running job with no Pending or Running file and no item being tidied is over.
    pub(super) fn check_finish(&mut self) {
        if self.state != TaskState::Running || self.tidying() {
            return;
        }
        if self.files.iter().any(|f| matches!(f.state, FileState::Pending | FileState::Running)) {
            return;
        }
        let failed = self.files.iter().filter(|f| f.state == FileState::Failed).count();
        let ev = if failed > 0 {
            let error = if failed == 1 { "1 file failed".to_owned() } else { format!("{failed} files failed") };
            TaskEvent::Fail(error)
        } else if let Some(error) = self.errors.first() {
            // Only items failed (their files, if any, are done).
            TaskEvent::Fail(error.clone())
        } else {
            TaskEvent::Complete
        };
        if let Some(s) = step(&self.state, ev) {
            self.state = s;
        }
    }

    /// Running files go back to Pending (they keep `done`); returns an `Abort` for each.
    pub(super) fn abort_running(&mut self) -> Vec<Action> {
        let id = self.id;
        let mut out = Vec::new();
        for (n, f) in self.files.iter_mut().enumerate() {
            if f.state == FileState::Running {
                f.state = FileState::Pending;
                out.push(Action::Abort { job: id, file: FileId(n as u32) });
            }
        }
        out
    }

    pub(super) fn conflict_view(&self) -> Option<ConflictView> {
        let into = self.req.remote_into()?;
        let mut clashes = self.items.iter().enumerate().filter_map(|(n, i)| match i.state {
            ItemState::Conflict { existing, incoming } => Some((n, existing, incoming)),
            _ => None,
        });
        let (first, existing, incoming) = clashes.next()?;
        Some(ConflictView {
            name: self.item_name(first),
            folder: into.clone(),
            existing_is_dir: existing.is_dir,
            existing: existing.side,
            incoming,
            more: clashes.count(),
        })
    }

    pub(super) fn reveal(&self) -> Option<Reveal> {
        match self.items.first()?.dst.as_ref()? {
            ItemDst::Local(p) => Some(Reveal::Local(p.clone())),
            ItemDst::Remote { path, .. } => Some(Reveal::Remote(self.dest.clone(), path.clone())),
        }
    }
}

fn local_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}
