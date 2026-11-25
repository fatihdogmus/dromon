//! The pure model of every transfer job and file, with the per-host scheduler.
//!
//! No I/O and no clocks: the async engine reports facts (a name is free, a scan finished, a file
//! progressed, a host went down) and carries out the [`Action`]s the queue hands back. Actions
//! only come out of [`Queue::poll`], [`Queue::pause`] and [`Queue::cancel`]; facts that need
//! follow-up work (a scan after `named`) park it in the job's outbox for the next `poll`.
//! See `.plans/dromon-plan.md` §3.5, §3.6, §3.9 and §4.6.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dromon_core::{HostId, RemotePath};

use crate::{
    ConflictChoice, ConflictView, FileId, FilePlan, JobId, JobRequest, JobView, Progress, Reveal, Scan, SpeedMeter,
    TaskEvent, TaskState, download_part_name, eta, step,
};

/// The settled destination of one top-level item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemDst {
    /// Download: the item's final local path (file or folder root).
    Local(PathBuf),
    /// Upload: the final remote path; `overwrite` = the user chose Replace.
    Remote { path: RemotePath, overwrite: bool },
}

/// Work for the engine to carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Download: pick a free local name for item `item` (the engine uses `unique_name` with the
    /// disk and [`Queue::is_reserved_local`], then calls [`Queue::named`]). Upload: check whether
    /// `into/name` exists on the host, then call [`Queue::exists`].
    Name { job: JobId, item: usize },
    /// Upload, after KeepBoth: list the remote folder, pick a free name with `unique_name`, call
    /// [`Queue::named`].
    FreeRemoteName { job: JobId, item: usize },
    /// Scan the item into `dst` (and create its folders), then call `scanned` / `item_failed`.
    Scan { job: JobId, item: usize, dst: ItemDst },
    /// Copy one file (resuming from its part file when there is one); report
    /// progress/done/failed/interrupted.
    Start { job: JobId, file: FileId, plan: FilePlan },
    /// Stop a running file (pause or cancel): the engine drops its task; the part file stays.
    Abort { job: JobId, file: FileId },
    /// Delete these files' part files (cancel): best effort.
    Cleanup { job: JobId, plans: Vec<FilePlan> },
}

/// Where one top-level item is on its way from "picked" to "scanned".
#[derive(Clone, Debug, PartialEq, Eq)]
enum ItemState {
    /// `Name` was (or will be) emitted; waiting for `named` (download) or `exists` (upload).
    Naming,
    /// Upload: the name is taken; waiting for the user.
    Conflict { existing_is_dir: bool },
    /// Upload, KeepBoth: `FreeRemoteName` was emitted; waiting for `named`.
    Renaming,
    /// `Scan` was (or will be) emitted; waiting for `scanned`.
    Scanning,
    Scanned,
    Skipped,
    Failed,
}

impl ItemState {
    /// Nothing more will happen to the item (until a retry).
    fn is_settled(&self) -> bool {
        matches!(self, Self::Scanned | Self::Skipped | Self::Failed)
    }
}

#[derive(Debug)]
struct Item {
    state: ItemState,
    dst: Option<ItemDst>,
    /// Files found so far (while scanning) or in total (once scanned).
    found: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileState {
    Pending,
    Running,
    Done,
    /// The error text lives in the job's `errors`.
    Failed,
}

#[derive(Debug)]
struct FileEntry {
    plan: FilePlan,
    state: FileState,
    /// Bytes done, absolute (includes a resumed offset); kept across pause and interruption.
    done: u64,
}

#[derive(Debug)]
struct Job {
    id: JobId,
    req: JobRequest,
    host: HostId,
    state: TaskState,
    items: Vec<Item>,
    /// `FileId(n)` is index `n`.
    files: Vec<FileEntry>,
    errors: Vec<String>,
    skipped: Vec<String>,
    /// Set by "apply to all": answers every later name clash.
    apply_all: Option<ConflictChoice>,
    /// Paused while Preparing: resume goes back to Preparing, not Running.
    paused_preparing: bool,
    /// Work parked by facts, handed out by the next `poll` (only while Preparing, host up).
    outbox: Vec<Action>,
    meter: SpeedMeter,
    first_progress: Option<Duration>,
}

impl Job {
    fn item_name(&self, item: usize) -> String {
        match &self.req {
            JobRequest::Download { items, .. } => {
                items.get(item).and_then(|i| i.path.file_name()).unwrap_or("/").to_owned()
            }
            JobRequest::Upload { items, .. } => items.get(item).map_or_else(String::new, |p| local_name(p)),
        }
    }

    fn files_found(&self) -> u64 {
        self.items.iter().map(|i| i.found).sum()
    }

    fn bytes_done(&self) -> u64 {
        self.files.iter().map(|f| f.done).sum()
    }

    fn all_settled(&self) -> bool {
        self.items.iter().all(|i| i.state.is_settled())
    }

    fn reset_speed(&mut self) {
        self.meter.reset();
        self.first_progress = None;
    }

    fn record_speed(&mut self, now: Duration) {
        self.first_progress.get_or_insert(now);
        let done = self.bytes_done();
        self.meter.record(now, done);
    }

    /// Item gets its destination and the scan is queued for the next poll.
    fn begin_scan(&mut self, item: usize, dst: ItemDst) {
        self.items[item].dst = Some(dst.clone());
        self.items[item].state = ItemState::Scanning;
        self.outbox.push(Action::Scan { job: self.id, item, dst });
    }

    /// Applies the user's answer to a name clash of an upload item.
    fn apply_choice(&mut self, item: usize, choice: ConflictChoice) {
        let JobRequest::Upload { into, .. } = &self.req else { return };
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

    /// While Preparing and every item is settled: start the files, or finish a job with none.
    fn advance(&mut self) {
        if !matches!(self.state, TaskState::Preparing { .. }) || !self.all_settled() {
            return;
        }
        if !self.files.is_empty() {
            if let Some(s) = step(&self.state, TaskEvent::ScanDone) {
                self.state = s;
            }
        } else if let Some(error) = self.errors.first().cloned() {
            if let Some(s) = step(&self.state, TaskEvent::Fail(error)) {
                self.state = s;
            }
        } else {
            // `step` has no Preparing → Done edge (nothing was found, nothing to run).
            self.state = TaskState::Done;
        }
    }

    /// A Running job with no Pending or Running file left is over.
    fn check_finish(&mut self) {
        if self.state != TaskState::Running {
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
    fn abort_running(&mut self) -> Vec<Action> {
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

    fn conflict_view(&self) -> Option<ConflictView> {
        let JobRequest::Upload { into, .. } = &self.req else { return None };
        let mut clashes = self.items.iter().enumerate().filter_map(|(n, i)| match i.state {
            ItemState::Conflict { existing_is_dir } => Some((n, existing_is_dir)),
            _ => None,
        });
        let (first, existing_is_dir) = clashes.next()?;
        Some(ConflictView { name: self.item_name(first), folder: into.clone(), existing_is_dir, more: clashes.count() })
    }

    fn reveal(&self) -> Option<Reveal> {
        match self.items.first()?.dst.as_ref()? {
            ItemDst::Local(p) => Some(Reveal::Local(p.clone())),
            ItemDst::Remote { path, .. } => Some(Reveal::Remote(self.host.clone(), path.clone())),
        }
    }
}

fn local_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Every transfer job and file, and which of them may run now.
#[derive(Debug)]
pub struct Queue {
    /// In creation order (ids ascending).
    jobs: Vec<Job>,
    up: HashSet<HostId>,
    per_host: usize,
    next_id: u64,
}

impl Queue {
    /// `per_host`: how many files may run at once per host.
    pub fn new(per_host: usize) -> Queue {
        Queue { jobs: Vec::new(), up: HashSet::new(), per_host, next_id: 1 }
    }

    /// Adds a job (state `Queued`). Ids count from 1.
    pub fn add(&mut self, req: JobRequest, _now: Duration) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;
        let items = (0..req.item_count()).map(|_| Item { state: ItemState::Naming, dst: None, found: 0 }).collect();
        self.jobs.push(Job {
            id,
            host: req.host().clone(),
            req,
            state: TaskState::Queued,
            items,
            files: Vec::new(),
            errors: Vec::new(),
            skipped: Vec::new(),
            apply_all: None,
            paused_preparing: false,
            outbox: Vec::new(),
            meter: SpeedMeter::new(),
            first_progress: None,
        });
        id
    }

    /// Hosts start down; nothing for a down host starts.
    pub fn host_up(&mut self, host: &HostId) {
        self.up.insert(host.clone());
    }

    /// The connection is gone: running files stay Running until the engine reports
    /// `file_interrupted`; nothing new starts until `host_up`.
    pub fn host_down(&mut self, host: &HostId) {
        self.up.remove(host);
        for j in self.jobs.iter_mut().filter(|j| &j.host == host) {
            j.reset_speed();
        }
    }

    /// Everything that may start now: naming for new jobs, parked scans, and pending files up to
    /// the per-host limit. Calling it again without new facts returns nothing.
    pub fn poll(&mut self, _now: Duration) -> Vec<Action> {
        let mut actions = Vec::new();
        for j in &mut self.jobs {
            if !self.up.contains(&j.host) {
                continue;
            }
            if j.state == TaskState::Queued
                && let Some(s) = step(&j.state, TaskEvent::Start)
            {
                j.state = s;
                for item in 0..j.items.len() {
                    actions.push(Action::Name { job: j.id, item });
                }
            }
            if matches!(j.state, TaskState::Preparing { .. }) {
                actions.append(&mut j.outbox);
                j.advance(); // a job without items is done at once
            }
        }

        let mut running: HashMap<HostId, usize> = HashMap::new();
        for j in &self.jobs {
            let n = j.files.iter().filter(|f| f.state == FileState::Running).count();
            *running.entry(j.host.clone()).or_default() += n;
        }
        for j in &mut self.jobs {
            if j.state != TaskState::Running || !self.up.contains(&j.host) {
                continue;
            }
            let count = running.entry(j.host.clone()).or_default();
            for (n, f) in j.files.iter_mut().enumerate() {
                if *count >= self.per_host {
                    break;
                }
                if f.state == FileState::Pending {
                    f.state = FileState::Running;
                    *count += 1;
                    actions.push(Action::Start { job: j.id, file: FileId(n as u32), plan: f.plan.clone() });
                }
            }
        }
        actions
    }

    fn live(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id && !j.state.is_finished())
    }

    // ---- naming -------------------------------------------------------------------------

    /// The engine settled the item's destination root (download: a free local name; upload
    /// after `FreeRemoteName`). The scan is queued for the next poll.
    pub fn named(&mut self, job: JobId, item: usize, dst: ItemDst) {
        let Some(j) = self.live(job) else { return };
        if j.items.get(item).is_some_and(|i| matches!(i.state, ItemState::Naming | ItemState::Renaming)) {
            j.begin_scan(item, dst);
        }
    }

    /// Upload: is `into/name` taken? `Some(is_dir)` / `None` = free.
    pub fn exists(&mut self, job: JobId, item: usize, existing: Option<bool>) {
        let Some(j) = self.live(job) else { return };
        let JobRequest::Upload { into, .. } = &j.req else { return };
        let path = into.join(&j.item_name(item));
        if !j.items.get(item).is_some_and(|i| i.state == ItemState::Naming) {
            return;
        }
        match (existing, j.apply_all) {
            (None, _) => {
                j.begin_scan(item, ItemDst::Remote { path, overwrite: false });
            }
            (Some(_), Some(choice)) => j.apply_choice(item, choice),
            (Some(existing_is_dir), None) => j.items[item].state = ItemState::Conflict { existing_is_dir },
        }
        j.advance();
    }

    /// The user's answer to the job's current conflict (the first item in Conflict). Ignored
    /// when there is none. `apply_all` also answers every other clash now and later ones.
    pub fn resolve(&mut self, job: JobId, choice: ConflictChoice, apply_all: bool) {
        let Some(j) = self.live(job) else { return };
        let clashing = |j: &Job| j.items.iter().position(|i| matches!(i.state, ItemState::Conflict { .. }));
        let Some(first) = clashing(j) else { return };
        j.apply_choice(first, choice);
        if apply_all {
            j.apply_all = Some(choice);
            while let Some(n) = clashing(j) {
                j.apply_choice(n, choice);
            }
        }
        j.advance();
    }

    // ---- scanning -----------------------------------------------------------------------

    pub fn scan_progress(&mut self, job: JobId, item: usize, files_found: u64) {
        let Some(j) = self.live(job) else { return };
        let Some(i) = j.items.get_mut(item).filter(|i| i.state == ItemState::Scanning) else { return };
        i.found = files_found;
        let total = j.files_found();
        if let Some(s) = step(&j.state, TaskEvent::ScanProgress(total)) {
            j.state = s;
        }
    }

    pub fn scanned(&mut self, job: JobId, item: usize, scan: Scan) {
        let Some(j) = self.live(job) else { return };
        if !j.items.get(item).is_some_and(|i| i.state == ItemState::Scanning) {
            return;
        }
        j.items[item].found = scan.files.len() as u64;
        j.items[item].state = ItemState::Scanned;
        j.skipped.extend(scan.skipped);
        j.files.extend(scan.files.into_iter().map(|plan| FileEntry { plan, state: FileState::Pending, done: 0 }));
        j.advance();
    }

    pub fn item_failed(&mut self, job: JobId, item: usize, error: String) {
        let Some(j) = self.live(job) else { return };
        if j.items.get(item).is_none_or(|i| i.state.is_settled()) {
            return;
        }
        j.items[item].state = ItemState::Failed;
        let line = format!("{}: {error}", j.item_name(item));
        j.errors.push(line);
        j.advance();
    }

    // ---- files --------------------------------------------------------------------------

    /// `done` = bytes of this file done so far (absolute, including a resumed offset).
    pub fn file_progress(&mut self, job: JobId, file: FileId, done: u64, now: Duration) {
        let Some(j) = self.live(job) else { return };
        let Some(f) = j.files.get_mut(file.0 as usize).filter(|f| f.state == FileState::Running) else { return };
        f.done = done.min(f.plan.size());
        j.record_speed(now);
    }

    pub fn file_done(&mut self, job: JobId, file: FileId, now: Duration) {
        let Some(j) = self.live(job) else { return };
        let Some(f) = j.files.get_mut(file.0 as usize).filter(|f| matches!(f.state, FileState::Pending | FileState::Running))
        else {
            return;
        };
        f.state = FileState::Done;
        f.done = f.plan.size();
        j.record_speed(now);
        j.check_finish();
    }

    pub fn file_failed(&mut self, job: JobId, file: FileId, error: String, now: Duration) {
        let Some(j) = self.live(job) else { return };
        let Some(f) = j.files.get_mut(file.0 as usize).filter(|f| f.state == FileState::Running) else { return };
        f.state = FileState::Failed;
        let line = format!("{}: {error}", f.plan.name());
        j.errors.push(line);
        j.record_speed(now);
        j.check_finish();
    }

    /// The connection was lost mid-file: back to Pending, `done` kept.
    pub fn file_interrupted(&mut self, job: JobId, file: FileId) {
        let Some(j) = self.live(job) else { return };
        if let Some(f) = j.files.get_mut(file.0 as usize).filter(|f| f.state == FileState::Running) {
            f.state = FileState::Pending;
        }
    }

    // ---- user controls ------------------------------------------------------------------

    /// Preparing or Running → Paused; aborts the running files (they keep `done`). Naming and
    /// scanning facts are still accepted while paused, but nothing starts until `resume`.
    pub fn pause(&mut self, job: JobId) -> Vec<Action> {
        let Some(j) = self.live(job) else { return Vec::new() };
        let preparing = matches!(j.state, TaskState::Preparing { .. });
        let Some(s) = step(&j.state, TaskEvent::Pause) else { return Vec::new() };
        j.state = s;
        j.paused_preparing = preparing;
        j.reset_speed();
        j.abort_running()
    }

    pub fn resume(&mut self, job: JobId) {
        let Some(j) = self.live(job) else { return };
        let Some(s) = step(&j.state, TaskEvent::Resume) else { return };
        j.reset_speed();
        if j.paused_preparing {
            // `step` resumes to Running; a job paused while preparing has items left, so it goes
            // back to Preparing (and `advance` moves on if they all settled meanwhile).
            j.state = TaskState::Preparing { files_found: j.files_found() };
            j.paused_preparing = false;
            j.advance();
        } else {
            j.state = s;
        }
        j.check_finish();
    }

    /// Any active job → Cancelled. Aborts running files and asks for the part files of every
    /// unfinished file with progress to be deleted (no `Cleanup` when there are none).
    pub fn cancel(&mut self, job: JobId) -> Vec<Action> {
        let Some(j) = self.live(job) else { return Vec::new() };
        let Some(s) = step(&j.state, TaskEvent::Cancel) else { return Vec::new() };
        j.state = s;
        j.outbox.clear();
        let plans: Vec<FilePlan> = j
            .files
            .iter()
            .filter(|f| match f.state {
                FileState::Running => true,
                FileState::Pending => f.done > 0,
                FileState::Done | FileState::Failed => false,
            })
            .map(|f| f.plan.clone())
            .collect();
        let mut out = j.abort_running();
        if !plans.is_empty() {
            out.push(Action::Cleanup { job: j.id, plans });
        }
        out
    }

    /// Failed → runs again: failed files go back to Pending (keeping `done`), failed items are
    /// named again, scanned items are not scanned again.
    pub fn retry(&mut self, job: JobId) {
        let Some(j) = self.jobs.iter_mut().find(|j| j.id == job) else { return };
        if step(&j.state, TaskEvent::Retry).is_none() {
            return;
        }
        for f in j.files.iter_mut().filter(|f| f.state == FileState::Failed) {
            f.state = FileState::Pending;
        }
        for (n, i) in j.items.iter_mut().enumerate() {
            if i.state == ItemState::Failed {
                i.state = ItemState::Naming;
                i.dst = None;
                i.found = 0;
                j.outbox.push(Action::Name { job, item: n });
            }
        }
        j.errors.clear();
        j.paused_preparing = false;
        j.reset_speed();
        // `step(Retry)` only says Failed → Queued, which would scan everything again; the
        // job continues from where it stopped instead.
        j.state = if j.all_settled() { TaskState::Running } else { TaskState::Preparing { files_found: j.files_found() } };
    }

    /// Removes Done, Failed and Cancelled jobs.
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| j.state.is_active());
    }

    // ---- queries ------------------------------------------------------------------------

    /// A download's destination root (or its part file) held by an active job.
    pub fn is_reserved_local(&self, path: &Path) -> bool {
        self.jobs.iter().filter(|j| j.state.is_active()).flat_map(|j| &j.items).any(|i| match &i.dst {
            Some(ItemDst::Local(root)) => {
                root == path
                    || root.file_name().is_some_and(|n| {
                        root.with_file_name(download_part_name(&n.to_string_lossy())) == path
                    })
            }
            _ => false,
        })
    }

    /// Newest job first.
    pub fn views(&self, now: Duration) -> Vec<JobView> {
        self.jobs.iter().rev().map(|j| self.make_view(j, now)).collect()
    }

    pub fn view(&self, job: JobId, now: Duration) -> Option<JobView> {
        self.jobs.iter().find(|j| j.id == job).map(|j| self.make_view(j, now))
    }

    /// Jobs whose state `is_active()`.
    pub fn active_count(&self) -> usize {
        self.jobs.iter().filter(|j| j.state.is_active()).count()
    }

    pub fn plan(&self, job: JobId, file: FileId) -> Option<&FilePlan> {
        self.jobs.iter().find(|j| j.id == job)?.files.get(file.0 as usize).map(|f| &f.plan)
    }

    fn make_view(&self, j: &Job, now: Duration) -> JobView {
        let total: u64 = j.files.iter().map(|f| f.plan.size()).sum();
        let done = j.bytes_done();
        let (speed, eta) = if j.state == TaskState::Running {
            let speed = j.meter.bytes_per_sec();
            let elapsed = j.first_progress.map_or(Duration::ZERO, |t| now.saturating_sub(t));
            (speed, eta(total - done, speed, elapsed))
        } else {
            (None, None)
        };
        let waiting = !self.up.contains(&j.host)
            && matches!(j.state, TaskState::Queued | TaskState::Preparing { .. } | TaskState::Running);
        let title = match j.items.len() {
            1 => j.item_name(0),
            n => format!("{n} items"),
        };
        JobView {
            id: j.id,
            kind: j.req.kind(),
            host: j.host.clone(),
            title,
            state: j.state.clone(),
            progress: Progress { done, total },
            files_done: j.files.iter().filter(|f| f.state == FileState::Done).count() as u64,
            files_total: j.files.len() as u64,
            speed,
            eta,
            waiting_for_connection: waiting,
            conflict: j.conflict_view(),
            reveal: j.reveal(),
            errors: j.errors.clone(),
            skipped: j.skipped.clone(),
        }
    }
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
