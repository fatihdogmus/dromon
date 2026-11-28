//! The pure model of every transfer job and file, with the per-host scheduler.
//!
//! No I/O and no clocks: the async engine reports facts (a name is free, a scan finished, a file
//! progressed, a host went down) and carries out the [`Action`]s the queue hands back. Actions
//! only come out of [`Queue::poll`], [`Queue::pause`] and [`Queue::cancel`]; facts that need
//! follow-up work (a scan after `named`) park it in the job's outbox for the next `poll`.
//! See `.plans/dromon-plan.md` §3.5–§3.7, §3.9 and §4.6.
//!
//! A job may involve two hosts (a cross-host copy or move, [`JobRequest::hosts`]): nothing of
//! it starts unless both are up, a running file counts against both hosts' limits, and either
//! going down makes it wait.
//!
//! Each job's items, files and the rules that move it on are `queue_job.rs`.

#[path = "queue_job.rs"]
mod job;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dromon_core::{HostId, RemotePath};

use self::job::{FileEntry, FileState, Item, ItemState, Job};
use crate::{
    ConflictChoice, Existing, FileId, FilePlan, JobId, JobRequest, JobView, Progress, Scan, Side, SpeedMeter,
    TaskEvent, TaskState, download_part_name, eta, step,
};

/// The settled destination of one top-level item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemDst {
    /// Download: the item's final local path (file or folder root).
    Local(PathBuf),
    /// Upload, copy, move: the final remote path (on the destination host); `overwrite` = the
    /// user chose Replace.
    Remote { path: RemotePath, overwrite: bool },
}

/// Work for the engine to carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Download: pick a free local name for item `item` (the engine uses `unique_name` with the
    /// disk and [`Queue::is_reserved_local`], then calls [`Queue::named`]). Upload, copy, move:
    /// check whether `into/name` exists on the destination host, then call [`Queue::exists`].
    Name { job: JobId, item: usize },
    /// Upload, copy, move after KeepBoth: list the destination folder, pick a free name with
    /// `unique_name`, call [`Queue::named`].
    FreeRemoteName { job: JobId, item: usize },
    /// Scan the item into `dst` (and create its folders), then call `scanned` / `item_failed`.
    /// Never for a same-host move: its one plan per item needs no scan.
    Scan { job: JobId, item: usize, dst: ItemDst },
    /// Copy (or move) one file, resuming from its part file when there is one; report
    /// progress/done/failed/interrupted.
    Start { job: JobId, file: FileId, plan: FilePlan },
    /// Stop a running file (pause or cancel): the engine drops its task; the part file stays.
    Abort { job: JobId, file: FileId },
    /// Delete these files' part files (cancel): best effort.
    Cleanup { job: JobId, plans: Vec<FilePlan> },
    /// Cross-host move: every file of folder item `item` is settled; remove the item's source
    /// folders that are empty now, deepest first (best effort), then call [`Queue::tidied`].
    Tidy { job: JobId, item: usize },
}

fn all_up(up: &HashSet<HostId>, hosts: &[HostId]) -> bool {
    hosts.iter().all(|h| up.contains(h))
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

    /// Changes the per-host limit (at least 1) for the starts from the next `poll` on. Running
    /// files keep going: a lower limit holds new starts back until fewer run.
    pub fn set_per_host(&mut self, per_host: usize) {
        self.per_host = per_host.max(1);
    }

    pub fn per_host(&self) -> usize {
        self.per_host
    }

    /// Adds a job (state `Queued`). Ids count from 1.
    pub fn add(&mut self, req: JobRequest, _now: Duration) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;
        let items = (0..req.item_count())
            .map(|_| Item { state: ItemState::Naming, dst: None, found: 0, tidied: false })
            .collect();
        self.jobs.push(Job {
            id,
            hosts: req.hosts().into_iter().cloned().collect(),
            dest: req.host().clone(),
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

    /// The connection is gone: running files of every job touching `host` stay Running until
    /// the engine reports `file_interrupted`; nothing new starts until `host_up`.
    pub fn host_down(&mut self, host: &HostId) {
        self.up.remove(host);
        for j in self.jobs.iter_mut().filter(|j| j.hosts.contains(host)) {
            j.reset_speed();
        }
    }

    /// Everything that may start now: naming for new jobs, parked work, and pending files up to
    /// the per-host limit. A job's work only starts while all its hosts are up. Calling it
    /// again without new facts returns nothing.
    pub fn poll(&mut self, _now: Duration) -> Vec<Action> {
        let mut actions = Vec::new();
        for j in &mut self.jobs {
            if !all_up(&self.up, &j.hosts) {
                continue;
            }
            if j.state == TaskState::Queued
                && let Some(s) = step(&j.state, TaskEvent::Start)
            {
                j.state = s;
                for item in 0..j.items.len() {
                    actions.extend(j.first_step(item));
                }
            }
            if matches!(j.state, TaskState::Preparing { .. } | TaskState::Running) {
                actions.append(&mut j.outbox);
                j.advance(); // a job without items (or with every item settled at once) moves on
            }
        }

        let mut running: HashMap<HostId, usize> = HashMap::new();
        for j in &self.jobs {
            let n = j.files.iter().filter(|f| f.state == FileState::Running).count();
            for h in &j.hosts {
                *running.entry(h.clone()).or_default() += n;
            }
        }
        for j in &mut self.jobs {
            if j.state != TaskState::Running || !all_up(&self.up, &j.hosts) {
                continue;
            }
            for (n, f) in j.files.iter_mut().enumerate() {
                if j.hosts.iter().any(|h| running.get(h).copied().unwrap_or(0) >= self.per_host) {
                    break;
                }
                if f.state == FileState::Pending {
                    f.state = FileState::Running;
                    for h in &j.hosts {
                        *running.entry(h.clone()).or_default() += 1;
                    }
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

    /// The engine settled the item's destination root (download: a free local name; the others
    /// after `FreeRemoteName`). The scan is queued for the next poll.
    pub fn named(&mut self, job: JobId, item: usize, dst: ItemDst) {
        let Some(j) = self.live(job) else { return };
        if j.items.get(item).is_some_and(|i| matches!(i.state, ItemState::Naming | ItemState::Renaming)) {
            j.begin_scan(item, dst);
            j.advance();
        }
    }

    /// Upload, copy, move: is `into/name` taken at the destination? `Some(what is there)` /
    /// `None` = free. `incoming` is what the item to transfer looks like, for the conflict view.
    pub fn exists(&mut self, job: JobId, item: usize, existing: Option<Existing>, incoming: Side) {
        let Some(j) = self.live(job) else { return };
        let Some(into) = j.req.remote_into() else { return };
        let path = into.join(&j.item_name(item));
        if !j.items.get(item).is_some_and(|i| i.state == ItemState::Naming) {
            return;
        }
        match (existing, j.apply_all) {
            (None, _) => {
                j.begin_scan(item, ItemDst::Remote { path, overwrite: false });
            }
            (Some(_), Some(choice)) => j.apply_choice(item, choice),
            (Some(existing), None) => j.items[item].state = ItemState::Conflict { existing, incoming },
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
        j.files.extend(scan.files.into_iter().map(|plan| FileEntry { plan, state: FileState::Pending, done: 0, item }));
        j.maybe_tidy(item); // a folder with nothing to copy
        j.advance();
    }

    pub fn item_failed(&mut self, job: JobId, item: usize, error: String) {
        let Some(j) = self.live(job) else { return };
        if j.items.get(item).is_none_or(|i| i.state.is_settled()) {
            return;
        }
        j.fail_item(item, &error);
        j.advance();
    }

    /// Cross-host move: the item's empty source folders are gone (or stayed: best effort).
    pub fn tidied(&mut self, job: JobId, item: usize) {
        let Some(j) = self.live(job) else { return };
        let Some(i) = j.items.get_mut(item).filter(|i| i.state == ItemState::Tidying) else { return };
        i.state = ItemState::Scanned;
        i.tidied = true;
        j.advance();
        j.check_finish();
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
        let Some(f) =
            j.files.get_mut(file.0 as usize).filter(|f| matches!(f.state, FileState::Pending | FileState::Running))
        else {
            return;
        };
        f.state = FileState::Done;
        f.done = f.plan.size();
        let item = f.item;
        j.record_speed(now);
        j.maybe_tidy(item);
        j.check_finish();
    }

    pub fn file_failed(&mut self, job: JobId, file: FileId, error: String, now: Duration) {
        let Some(j) = self.live(job) else { return };
        let Some(f) = j.files.get_mut(file.0 as usize).filter(|f| f.state == FileState::Running) else { return };
        f.state = FileState::Failed;
        let line = format!("{}: {error}", f.plan.name());
        let item = f.item;
        j.errors.push(line);
        j.record_speed(now);
        j.maybe_tidy(item);
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
            j.items[f.item].tidied = false;
        }
        j.errors.clear();
        for n in 0..j.items.len() {
            if j.items[n].state == ItemState::Failed {
                let i = &mut j.items[n];
                i.state = ItemState::Naming;
                i.dst = None;
                i.found = 0;
                if let Some(a) = j.first_step(n) {
                    j.outbox.push(a);
                }
            }
        }
        j.paused_preparing = false;
        j.reset_speed();
        // `step(Retry)` only says Failed → Queued, which would scan everything again; the
        // job continues from where it stopped instead.
        j.state =
            if j.all_settled() { TaskState::Running } else { TaskState::Preparing { files_found: j.files_found() } };
        j.advance();
        j.check_finish();
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
                    || root
                        .file_name()
                        .is_some_and(|n| root.with_file_name(download_part_name(&n.to_string_lossy())) == path)
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

    /// Whether the job is there and not finished (it still takes facts: work cut off for it is
    /// worth running again).
    pub fn is_live(&self, job: JobId) -> bool {
        self.jobs.iter().any(|j| j.id == job && !j.state.is_finished())
    }

    /// Jobs whose state `is_active()`.
    pub fn active_count(&self) -> usize {
        self.jobs.iter().filter(|j| j.state.is_active()).count()
    }

    pub fn plan(&self, job: JobId, file: FileId) -> Option<&FilePlan> {
        self.jobs.iter().find(|j| j.id == job)?.files.get(file.0 as usize).map(|f| &f.plan)
    }

    fn make_view(&self, j: &Job, now: Duration) -> JobView {
        let total = j.bytes_total();
        let done = j.bytes_done();
        let (speed, eta) = if j.state == TaskState::Running {
            let speed = j.meter.bytes_per_sec();
            let elapsed = j.first_progress.map_or(Duration::ZERO, |t| now.saturating_sub(t));
            (speed, eta(total.saturating_sub(done), speed, elapsed))
        } else {
            (None, None)
        };
        let waiting = !all_up(&self.up, &j.hosts)
            && matches!(j.state, TaskState::Queued | TaskState::Preparing { .. } | TaskState::Running);
        let title = match j.items.len() {
            1 => j.item_name(0),
            n => format!("{n} items"),
        };
        JobView {
            id: j.id,
            kind: j.req.kind(),
            host: j.dest.clone(),
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
            folders: j.req.changed_folders(),
        }
    }
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "queue_remote_tests.rs"]
mod remote_tests;

#[cfg(test)]
#[path = "queue_props.rs"]
mod props;
