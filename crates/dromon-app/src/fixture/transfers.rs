//! Transfers on a fixture: the real queue (`dromon_transfer::queue::Queue`) driven
//! synchronously with fake I/O, so the popover, the ring, the conflict sheet and finished
//! uploads in listings all behave as they will on the real engine, deterministically.
//!
//! - Hosts are up while their fixture connection is ready.
//! - Naming: a download takes a free Chrome-style name against the names it already "wrote"
//!   (and [`Queue::is_reserved_local`]); an upload asks the folder tree whether its name exists,
//!   and Keep both picks a free name there with `unique_name`.
//! - Scanning: a download walks the folder tree (links to folders inside are left out, as the
//!   real scan does); an upload is one file of [`UPLOAD_SIZE`] bytes per path (a local folder
//!   counts as one file: nothing on the disk is read).
//! - A file is done the moment it starts. Aborting and cleaning up do nothing.
//! - Finished uploads are added to the tree, so listing the folder again shows them.
//!
//! The clock is a tick per carried-out action; no threads, no real time.
//!
//! The fixture's own job snapshots (`transfers` in the fixture) stay listed after the session's
//! jobs, as pictures: their buttons move them along the task state machine, nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dromon_core::{Entry, EntryKind, Error, HostId, LinkTarget, RemotePath, natural_cmp};
use dromon_transfer::queue::{Action, ItemDst, Queue};
use dromon_transfer::{
    DirPlan, FilePlan, JobId, JobRequest, JobView, Scan, TaskEvent, TaskState, step, unique_name,
};

use super::{Fixture, default_mode};
use crate::state::{ServiceEvent, TransferCommand};

/// The size every uploaded path pretends to have.
pub const UPLOAD_SIZE: u64 = 1_200_000;

/// Files that run at once per host (the real default, §4.6).
pub const PER_HOST: usize = 3;

/// How deep a download scan goes before it stops looking.
const MAX_DEPTH: usize = 64;

/// The transfer engine of a fixture.
pub struct Engine {
    fixture: Arc<Fixture>,
    queue: Queue,
    /// What each of the session's jobs asked for.
    requests: BTreeMap<JobId, JobRequest>,
    /// The fixture's snapshots, older than any session job.
    pictures: Vec<JobView>,
    /// Local paths downloads have written.
    local: BTreeSet<PathBuf>,
    /// Entries uploads have added, by host and folder.
    added: BTreeMap<(HostId, RemotePath), Vec<Entry>>,
    /// Folders uploads changed since the last command.
    touched: BTreeSet<(HostId, RemotePath)>,
    tick: u64,
}

impl Engine {
    pub fn new(fixture: Arc<Fixture>) -> Engine {
        let pictures = fixture.transfers.clone();
        Engine {
            fixture,
            queue: Queue::new(PER_HOST),
            requests: BTreeMap::new(),
            pictures,
            local: BTreeSet::new(),
            added: BTreeMap::new(),
            touched: BTreeSet::new(),
            tick: 0,
        }
    }

    fn now(&self) -> Duration {
        Duration::from_millis(self.tick * 100)
    }

    /// A folder as the server lists it now: the fixture's tree plus what uploads added.
    pub fn list(&self, host: &HostId, path: &RemotePath) -> Result<Vec<Entry>, Error> {
        let mut entries = match self.fixture.server(host.as_str()) {
            Some(s) => s.list(path)?,
            None => return Err(Error::NotFound(path.clone())),
        };
        if let Some(added) = self.added.get(&(host.clone(), path.clone())) {
            for e in added {
                match entries.iter_mut().find(|x| x.name == e.name) {
                    Some(x) => *x = e.clone(),
                    None => entries.push(e.clone()),
                }
            }
        }
        Ok(entries)
    }

    /// Carries out `cmd` and everything it leads to with `up` the connected hosts. Returns the
    /// one `Transfers` snapshot to send, and the folders finished uploads changed.
    pub fn command(&mut self, cmd: TransferCommand, up: &BTreeSet<HostId>) -> (ServiceEvent, Vec<(HostId, RemotePath)>) {
        let now = self.now();
        let mut actions = Vec::new();
        if let TransferCommand::Start(req) = &cmd {
            let id = self.queue.add(req.clone(), now);
            self.requests.insert(id, req.clone());
        }
        let hosts: BTreeSet<HostId> = self.requests.values().map(|r| r.host().clone()).collect();
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
        (ServiceEvent::Transfers(views), std::mem::take(&mut self.touched).into_iter().collect())
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
            Action::Start { job, file, plan } => {
                self.finish(job, &plan);
                self.queue.file_done(job, file, self.now());
            }
            Action::Abort { .. } | Action::Cleanup { .. } => {}
        }
    }

    fn name(&mut self, job: JobId, item: usize) {
        match self.requests.get(&job).cloned() {
            Some(JobRequest::Download { items, into, .. }) => {
                let Some(it) = items.get(item) else { return };
                let name = it.path.file_name().unwrap_or("download").to_owned();
                let taken = |n: &str| {
                    let p = into.join(n);
                    self.local.contains(&p) || self.queue.is_reserved_local(&p)
                };
                let free = unique_name(&name, taken);
                self.queue.named(job, item, ItemDst::Local(into.join(free)));
            }
            Some(JobRequest::Upload { host, items, into }) => {
                let Some(name) = items.get(item).map(|p| local_name(p)) else { return };
                match self.list(&host, &into) {
                    Ok(entries) => {
                        let existing = entries.iter().find(|e| e.name == name).map(|e| e.kind.is_dir_like());
                        self.queue.exists(job, item, existing);
                    }
                    Err(e) => self.queue.item_failed(job, item, e.to_string()),
                }
            }
            None => {}
        }
    }

    fn free_remote_name(&mut self, job: JobId, item: usize) {
        let Some(JobRequest::Upload { host, items, into }) = self.requests.get(&job).cloned() else { return };
        let Some(name) = items.get(item).map(|p| local_name(p)) else { return };
        match self.list(&host, &into) {
            Ok(entries) => {
                let free = unique_name(&name, |n| entries.iter().any(|e| e.name == n));
                self.queue.named(job, item, ItemDst::Remote { path: into.join(&free), overwrite: false });
            }
            Err(e) => self.queue.item_failed(job, item, e.to_string()),
        }
    }

    fn scan(&mut self, job: JobId, item: usize, dst: ItemDst) {
        let result = match (self.requests.get(&job).cloned(), dst) {
            (Some(JobRequest::Download { host, items, .. }), ItemDst::Local(root)) => {
                let Some(it) = items.get(item) else { return };
                if it.kind.is_dir_like() {
                    let mut scan = Scan::default();
                    self.walk(&host, &it.path, &root, "", 0, &mut scan).map(|()| scan)
                } else {
                    let mtime = it.path.parent().and_then(|dir| self.list(&host, &dir).ok()).and_then(|entries| {
                        entries.into_iter().find(|e| Some(e.name.as_str()) == it.path.file_name()).and_then(|e| e.mtime)
                    });
                    let plan = FilePlan::Download { src: it.path.clone(), dst: root, size: it.size, mtime };
                    Ok(Scan { dirs: Vec::new(), files: vec![plan], skipped: Vec::new() })
                }
            }
            (Some(JobRequest::Upload { items, .. }), ItemDst::Remote { path, overwrite }) => {
                let Some(src) = items.get(item).cloned() else { return };
                let plan = FilePlan::Upload { src, dst: path, size: UPLOAD_SIZE, mtime: None, overwrite };
                Ok(Scan { dirs: Vec::new(), files: vec![plan], skipped: Vec::new() })
            }
            _ => return,
        };
        match result {
            Ok(scan) => self.queue.scanned(job, item, scan),
            Err(e) => self.queue.item_failed(job, item, e.to_string()),
        }
    }

    /// Plans a remote folder's download into `dst`, depth first, names in natural order.
    /// Subfolders that can't be listed are left out with their error; only the item itself
    /// failing to list fails it.
    fn walk(&self, host: &HostId, src: &RemotePath, dst: &Path, rel: &str, depth: usize, scan: &mut Scan) -> Result<(), Error> {
        scan.dirs.push(DirPlan::Local(dst.to_path_buf()));
        let mut entries = self.list(host, src)?;
        entries.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        for e in entries {
            let shown = format!("{rel}{}", e.name);
            match e.kind {
                EntryKind::File | EntryKind::Symlink { target: Some(LinkTarget::File) } => {
                    scan.files.push(FilePlan::Download { src: src.join(&e.name), dst: dst.join(&e.name), size: e.size, mtime: e.mtime });
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

    /// A file "arrived": a download's name is taken from now on; an upload shows in its folder.
    fn finish(&mut self, job: JobId, plan: &FilePlan) {
        match plan {
            FilePlan::Download { dst, .. } => {
                self.local.insert(dst.clone());
            }
            FilePlan::Upload { dst, size, .. } => {
                let (Some(host), Some(folder), Some(name)) =
                    (self.requests.get(&job).map(|r| r.host().clone()), dst.parent(), dst.file_name())
                else {
                    return;
                };
                let entry = Entry {
                    name: name.to_owned(),
                    kind: EntryKind::File,
                    size: *size,
                    mtime: self.fixture.now,
                    mode: default_mode(EntryKind::File),
                };
                let list = self.added.entry((host.clone(), folder.clone())).or_default();
                match list.iter_mut().find(|e| e.name == entry.name) {
                    Some(e) => *e = entry,
                    None => list.push(entry),
                }
                self.touched.insert((host, folder));
            }
        }
    }
}

fn local_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}
