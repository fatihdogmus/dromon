//! Transfers on a fixture: the real queue (`dromon_transfer::queue::Queue`) driven
//! synchronously with fake I/O, so the popover, the ring, the conflict sheet and finished
//! uploads, copies and moves in listings all behave as they will on the real engine,
//! deterministically.
//!
//! - Hosts are up while their fixture connection is ready; a job between two hosts needs both.
//! - Naming: a download takes a free Chrome-style name against the names it already "wrote"
//!   (and [`Queue::is_reserved_local`]); uploads, copies and moves ask the destination's tree
//!   whether the name exists, and Keep both picks a free name there with `unique_name`.
//! - Scanning: downloads and copies walk the source tree (links to folders inside are left
//!   out, as the real scan does); copies make their folders on the destination at once; an
//!   upload is one file of [`UPLOAD_SIZE`] bytes per path (a local folder counts as one file:
//!   nothing on the disk is read). A same-host move has no scan (the queue plans it).
//! - A file is done the moment it starts: an upload or copy adds its entry to the destination
//!   (a copy keeps the source's size and date), a move takes the whole item (its folder tree
//!   included) from the source to the destination, merging into a folder on Replace, and a move
//!   between hosts deletes each source file once copied; tidying removes the source folders
//!   left empty. Aborting and cleaning up do nothing.
//!
//! Every lookup and change goes through the engine's [`Tree`] (`fixture/tree.rs`), the one
//! overlay on the fixture's folders that file operations change too, so transfers see new,
//! renamed and deleted folders, and listings show what transfers did. The clock is a tick per
//! carried-out action; no threads, no real time.
//!
//! The fixture's own job snapshots (`transfers` in the fixture) stay listed after the session's
//! jobs, as pictures: their buttons move them along the task state machine, nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dromon_core::{EntryKind, Error, HostId, LinkTarget, RemotePath, natural_cmp};
use dromon_transfer::queue::{Action, ItemDst, Queue};
use dromon_transfer::{
    DirPlan, Existing, FileId, FilePlan, JobId, JobKind, JobRequest, JobView, Scan, Side, TaskEvent, TaskState, step,
    unique_name,
};

use super::Fixture;
use super::tree::Tree;
use crate::state::{ServiceEvent, TransferCommand};

/// The size every uploaded path pretends to have.
pub const UPLOAD_SIZE: u64 = 1_200_000;

/// Files that run at once per host (the real default, §4.6).
pub const PER_HOST: usize = 3;

/// How deep a scan goes before it stops looking.
const MAX_DEPTH: usize = 64;

/// The transfer engine of a fixture.
pub struct Engine {
    /// The servers' folders now; the backend's listings and file operations use it too.
    pub(super) tree: Tree,
    /// The fixture's clock: what uploads and the folders copies make are dated.
    mtime: Option<i64>,
    queue: Queue,
    /// What each of the session's jobs asked for.
    requests: BTreeMap<JobId, JobRequest>,
    /// The fixture's snapshots, older than any session job.
    pictures: Vec<JobView>,
    /// Local paths downloads have written.
    local: BTreeSet<PathBuf>,
    tick: u64,
}

/// Where a scan plans its files.
#[derive(Clone)]
enum To {
    Local(PathBuf),
    /// A copy's remote root, and its Replace.
    Remote(RemotePath, bool),
}

impl To {
    fn join(&self, name: &str) -> To {
        match self {
            To::Local(p) => To::Local(p.join(name)),
            To::Remote(p, o) => To::Remote(p.join(name), *o),
        }
    }

    fn dir(&self) -> DirPlan {
        match self {
            To::Local(p) => DirPlan::Local(p.clone()),
            To::Remote(p, _) => DirPlan::Remote(p.clone()),
        }
    }

    fn plan(&self, src: RemotePath, size: u64, mtime: Option<i64>) -> FilePlan {
        match self {
            To::Local(dst) => FilePlan::Download { src, dst: dst.clone(), size, mtime },
            To::Remote(dst, overwrite) => FilePlan::Copy { src, dst: dst.clone(), size, mtime, overwrite: *overwrite },
        }
    }
}

impl Engine {
    pub fn new(fixture: Arc<Fixture>) -> Engine {
        let pictures = fixture.transfers.clone();
        Engine {
            mtime: fixture.now,
            tree: Tree::new(fixture),
            queue: Queue::new(PER_HOST),
            requests: BTreeMap::new(),
            pictures,
            local: BTreeSet::new(),
            tick: 0,
        }
    }

    fn now(&self) -> Duration {
        Duration::from_millis(self.tick * 100)
    }

    /// Carries out `cmd` and everything it leads to with `up` the connected hosts. Returns the
    /// one `Transfers` snapshot to send, and the folders finished transfers changed.
    pub fn command(&mut self, cmd: TransferCommand, up: &BTreeSet<HostId>) -> (ServiceEvent, Vec<(HostId, RemotePath)>) {
        // Folders file operations changed since the last command: the backend relisted those.
        self.tree.take_touched();
        let now = self.now();
        let mut actions = Vec::new();
        if let TransferCommand::Start(req) = &cmd {
            let id = self.queue.add(req.clone(), now);
            self.requests.insert(id, req.clone());
        }
        let hosts: BTreeSet<HostId> = self.requests.values().flat_map(|r| r.hosts()).cloned().collect();
        for host in &hosts {
            if up.contains(host) {
                self.queue.host_up(host);
            } else {
                self.queue.host_down(host);
            }
        }
        match cmd {
            TransferCommand::Start(_) => {}
            TransferCommand::Pause(id) => {
                self.picture(id, TaskEvent::Pause);
                actions = self.queue.pause(id);
            }
            TransferCommand::Resume(id) => {
                self.picture(id, TaskEvent::Resume);
                self.queue.resume(id);
            }
            TransferCommand::Cancel(id) => {
                self.picture(id, TaskEvent::Cancel);
                actions = self.queue.cancel(id);
            }
            TransferCommand::Retry(id) => {
                self.picture(id, TaskEvent::Retry);
                self.queue.retry(id);
            }
            TransferCommand::Resolve { job, choice, apply_all } => {
                if let Some(p) = self.pictures.iter_mut().find(|p| p.id == job) {
                    p.conflict = None;
                }
                self.queue.resolve(job, choice, apply_all);
            }
            TransferCommand::ClearFinished => {
                self.queue.clear_finished();
                self.pictures.retain(|p| p.state.is_active());
            }
        }
        loop {
            for a in std::mem::take(&mut actions) {
                self.carry_out(a);
            }
            actions = self.queue.poll(self.now());
            if actions.is_empty() {
                break;
            }
        }
        let mut views = self.queue.views(self.now());
        views.extend(self.pictures.iter().cloned());
        (ServiceEvent::Transfers(views), self.tree.take_touched().into_iter().collect())
    }

    /// A button on one of the fixture's pictures: its state moves as the state machine allows.
    fn picture(&mut self, id: JobId, ev: TaskEvent) {
        let Some(p) = self.pictures.iter_mut().find(|p| p.id == id) else { return };
        if let Some(s) = step(&p.state, ev) {
            if !matches!(s, TaskState::Running) {
                p.speed = None;
                p.eta = None;
            }
            p.state = s;
        }
    }

    fn carry_out(&mut self, action: Action) {
        self.tick += 1;
        match action {
            Action::Name { job, item } => self.name(job, item),
            Action::FreeRemoteName { job, item } => self.free_remote_name(job, item),
            Action::Scan { job, item, dst } => self.scan(job, item, dst),
            Action::Start { job, file, plan } => self.start(job, file, &plan),
            Action::Tidy { job, item } => {
                let req = self.requests.get(&job).cloned();
                if let Some((req, it)) = req.as_ref().and_then(|r| Some((r, r.remote_items()?.get(item)?))) {
                    self.tree.remove_empty_dirs(req.source_host(), &it.path, 0);
                }
                self.queue.tidied(job, item);
            }
            Action::Abort { .. } | Action::Cleanup { .. } => {}
        }
    }

    fn name(&mut self, job: JobId, item: usize) {
        let Some(req) = self.requests.get(&job).cloned() else { return };
        if let JobRequest::Download { items, into, .. } = &req {
            let Some(it) = items.get(item) else { return };
            let name = it.path.file_name().unwrap_or("download").to_owned();
            let taken = |n: &str| {
                let p = into.join(n);
                self.local.contains(&p) || self.queue.is_reserved_local(&p)
            };
            let free = unique_name(&name, taken);
            self.queue.named(job, item, ItemDst::Local(into.join(free)));
            return;
        }
        let (Some(name), Some(into)) = (item_name(&req, item), req.remote_into()) else { return };
        match self.tree.list(req.host(), into) {
            Ok(entries) => {
                let existing = entries.iter().find(|e| e.name == name).map(|e| {
                    let is_dir = e.kind.is_dir_like();
                    Existing { is_dir, side: Side { size: (!is_dir).then_some(e.size), mtime: e.mtime } }
                });
                // An upload is the fixture's pretend file; a copy or move is its source entry.
                let incoming = match (&req, req.remote_items().and_then(|i| i.get(item))) {
                    (JobRequest::Upload { .. }, _) => Side { size: Some(UPLOAD_SIZE), mtime: self.mtime },
                    (_, Some(it)) => Side {
                        size: (!it.kind.is_dir_like()).then_some(it.size),
                        mtime: self.tree.entry_at(req.source_host(), &it.path).and_then(|e| e.mtime),
                    },
                    _ => Side::default(),
                };
                self.queue.exists(job, item, existing, incoming);
            }
            Err(e) => self.queue.item_failed(job, item, e.to_string()),
        }
    }

    fn free_remote_name(&mut self, job: JobId, item: usize) {
        let Some(req) = self.requests.get(&job).cloned() else { return };
        let (Some(name), Some(into)) = (item_name(&req, item), req.remote_into()) else { return };
        match self.tree.list(req.host(), into) {
            Ok(entries) => {
                let free = unique_name(&name, |n| entries.iter().any(|e| e.name == n));
                self.queue.named(job, item, ItemDst::Remote { path: into.join(&free), overwrite: false });
            }
            Err(e) => self.queue.item_failed(job, item, e.to_string()),
        }
    }

    fn scan(&mut self, job: JobId, item: usize, dst: ItemDst) {
        let Some(req) = self.requests.get(&job).cloned() else { return };
        let result = match (&req, dst) {
            (JobRequest::Upload { items, .. }, ItemDst::Remote { path, overwrite }) => {
                let Some(src) = items.get(item).cloned() else { return };
                let plan = FilePlan::Upload { src, dst: path, size: UPLOAD_SIZE, mtime: None, overwrite };
                Ok(Scan { dirs: Vec::new(), files: vec![plan], skipped: Vec::new() })
            }
            (JobRequest::Download { .. }, ItemDst::Local(root)) => self.scan_remote(&req, item, To::Local(root)),
            (JobRequest::Move { .. }, _) => return,
            (_, ItemDst::Remote { path, overwrite }) => {
                self.scan_remote(&req, item, To::Remote(path, overwrite)).and_then(|scan| {
                    for d in &scan.dirs {
                        if let DirPlan::Remote(d) = d {
                            self.tree.ensure_dir(req.host(), d, self.mtime)?;
                        }
                    }
                    Ok(scan)
                })
            }
            _ => return,
        };
        match result {
            Ok(scan) => self.queue.scanned(job, item, scan),
            Err(e) => self.queue.item_failed(job, item, e.to_string()),
        }
    }

    /// Plans a download or copy of the job's remote item `item` into `to`.
    fn scan_remote(&self, req: &JobRequest, item: usize, to: To) -> Result<Scan, Error> {
        let Some(it) = req.remote_items().and_then(|i| i.get(item)) else { return Ok(Scan::default()) };
        let host = req.source_host();
        if it.kind.is_dir_like() {
            let mut scan = Scan::default();
            self.walk(host, &it.path, &to, "", 0, &mut scan)?;
            return Ok(scan);
        }
        let mtime = self.tree.entry_at(host, &it.path).and_then(|e| e.mtime);
        Ok(Scan { dirs: Vec::new(), files: vec![to.plan(it.path.clone(), it.size, mtime)], skipped: Vec::new() })
    }

    /// Plans a remote folder's download or copy into `dst`, depth first, names in natural
    /// order. Subfolders that can't be listed are left out with their error; only the item
    /// itself failing to list fails it.
    fn walk(&self, host: &HostId, src: &RemotePath, dst: &To, rel: &str, depth: usize, scan: &mut Scan) -> Result<(), Error> {
        scan.dirs.push(dst.dir());
        let mut entries = self.tree.list(host, src)?;
        entries.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        for e in entries {
            let shown = format!("{rel}{}", e.name);
            match e.kind {
                EntryKind::File | EntryKind::Symlink { target: Some(LinkTarget::File) } => {
                    scan.files.push(dst.join(&e.name).plan(src.join(&e.name), e.size, e.mtime));
                }
                EntryKind::Dir if depth + 1 >= MAX_DEPTH => scan.skipped.push(format!("{shown}: too deep")),
                EntryKind::Dir => {
                    let (sub_src, sub_dst) = (src.join(&e.name), dst.join(&e.name));
                    if let Err(err) = self.walk(host, &sub_src, &sub_dst, &format!("{shown}/"), depth + 1, scan) {
                        scan.skipped.push(format!("{shown}: {err}"));
                    }
                }
                EntryKind::Symlink { target: Some(LinkTarget::Dir) } => scan.skipped.push(format!("{shown}: a link to a folder")),
                EntryKind::Symlink { .. } => scan.skipped.push(format!("{shown}: a broken link")),
                EntryKind::Other => scan.skipped.push(format!("{shown}: not a regular file")),
            }
        }
        Ok(())
    }

    /// One file "arrives" at once.
    fn start(&mut self, job: JobId, file: FileId, plan: &FilePlan) {
        let Some(req) = self.requests.get(&job).cloned() else { return };
        let result = match plan {
            FilePlan::Download { dst, .. } => {
                self.local.insert(dst.clone());
                Ok(())
            }
            FilePlan::Upload { dst, size, .. } => self.tree.put_file(req.host(), dst, *size, self.mtime),
            FilePlan::Copy { src, dst, size, mtime, .. } => self.tree.put_file(req.host(), dst, *size, *mtime).map(|()| {
                if req.kind() == JobKind::CrossHostMove {
                    // A source that's gone already stays gone.
                    let _ = self.tree.remove(req.source_host(), src);
                }
            }),
            FilePlan::Move { src, dst, overwrite, .. } => self.tree.move_to(req.host(), src, dst, *overwrite),
        };
        match result {
            Ok(()) => self.queue.file_done(job, file, self.now()),
            Err(e) => self.queue.file_failed(job, file, e.to_string(), self.now()),
        }
    }
}

/// The name of a job's item: an upload's local file name, else the remote one.
fn item_name(req: &JobRequest, item: usize) -> Option<String> {
    match req {
        JobRequest::Upload { items, .. } => items.get(item).map(|p| local_name(p)),
        req => req.remote_items()?.get(item)?.path.file_name().map(str::to_owned),
    }
}

fn local_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}
