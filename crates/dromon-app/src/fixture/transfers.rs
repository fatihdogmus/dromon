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
//! The tree's changes live in an overlay (added entries, removed paths, created folders) on
//! top of the fixture's folders, so listing a changed folder again shows them. The clock is a
//! tick per carried-out action; no threads, no real time.
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
    DirPlan, FileId, FilePlan, JobId, JobKind, JobRequest, JobView, Scan, TaskEvent, TaskState, step, unique_name,
};

use super::{Fixture, default_mode};
use crate::state::{ServiceEvent, TransferCommand};

/// The size every uploaded path pretends to have.
pub const UPLOAD_SIZE: u64 = 1_200_000;

/// Files that run at once per host (the real default, §4.6).
pub const PER_HOST: usize = 3;

/// How deep a scan or a moved tree goes before it stops looking.
const MAX_DEPTH: usize = 64;

type Key = (HostId, RemotePath);

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
    /// Entries uploads, copies and moves have added, by host and folder.
    added: BTreeMap<Key, Vec<Entry>>,
    /// Paths moves took away, with everything below them.
    removed: BTreeSet<Key>,
    /// Folders copies and moves made: they hold only what `added` gives them.
    created: BTreeSet<Key>,
    /// Folders that changed since the last command.
    touched: BTreeSet<Key>,
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
            fixture,
            queue: Queue::new(PER_HOST),
            requests: BTreeMap::new(),
            pictures,
            local: BTreeSet::new(),
            added: BTreeMap::new(),
            removed: BTreeSet::new(),
            created: BTreeSet::new(),
            touched: BTreeSet::new(),
            tick: 0,
        }
    }

    fn now(&self) -> Duration {
        Duration::from_millis(self.tick * 100)
    }

    /// A folder as the server lists it now: the fixture's tree with what transfers changed.
    pub fn list(&self, host: &HostId, path: &RemotePath) -> Result<Vec<Entry>, Error> {
        let key = |p: &RemotePath| (host.clone(), p.clone());
        let lineage = path.ancestors_from_root();
        let above = &lineage[..lineage.len().saturating_sub(1)];
        let made = self.created.contains(&key(path));
        // Gone with a moved folder, or below a made folder without having been made.
        if self.removed.contains(&key(path))
            || above.iter().any(|a| self.removed.contains(&key(a)))
            || (!made && above.iter().any(|a| self.created.contains(&key(a))))
        {
            return Err(Error::NotFound(path.clone()));
        }
        let mut entries = if made {
            Vec::new()
        } else {
            match self.fixture.server(host.as_str()) {
                Some(s) => s.list(path)?,
                None => return Err(Error::NotFound(path.clone())),
            }
        };
        entries.retain(|e| !self.removed.contains(&key(&path.join(&e.name))));
        if let Some(added) = self.added.get(&key(path)) {
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
    /// one `Transfers` snapshot to send, and the folders finished transfers changed.
    pub fn command(&mut self, cmd: TransferCommand, up: &BTreeSet<HostId>) -> (ServiceEvent, Vec<(HostId, RemotePath)>) {
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
            Action::Start { job, file, plan } => self.start(job, file, &plan),
            Action::Tidy { job, item } => {
                let req = self.requests.get(&job).cloned();
                if let Some((req, it)) = req.as_ref().and_then(|r| Some((r, r.remote_items()?.get(item)?))) {
                    self.remove_empty_dirs(req.source_host(), &it.path, 0);
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
        match self.list(req.host(), into) {
            Ok(entries) => {
                let existing = entries.iter().find(|e| e.name == name).map(|e| e.kind.is_dir_like());
                self.queue.exists(job, item, existing);
            }
            Err(e) => self.queue.item_failed(job, item, e.to_string()),
        }
    }

    fn free_remote_name(&mut self, job: JobId, item: usize) {
        let Some(req) = self.requests.get(&job).cloned() else { return };
        let (Some(name), Some(into)) = (item_name(&req, item), req.remote_into()) else { return };
        match self.list(req.host(), into) {
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
                            self.mkdir(req.host(), d, self.fixture.now)?;
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
        let mtime = self.entry_at(host, &it.path).and_then(|e| e.mtime);
        Ok(Scan { dirs: Vec::new(), files: vec![to.plan(it.path.clone(), it.size, mtime)], skipped: Vec::new() })
    }

    /// Plans a remote folder's download or copy into `dst`, depth first, names in natural
    /// order. Subfolders that can't be listed are left out with their error; only the item
    /// itself failing to list fails it.
    fn walk(&self, host: &HostId, src: &RemotePath, dst: &To, rel: &str, depth: usize, scan: &mut Scan) -> Result<(), Error> {
        scan.dirs.push(dst.dir());
        let mut entries = self.list(host, src)?;
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
            FilePlan::Upload { dst, size, .. } => {
                self.put(req.host(), dst, EntryKind::File, *size, self.fixture.now);
                Ok(())
            }
            FilePlan::Copy { src, dst, size, mtime, .. } => {
                self.put(req.host(), dst, EntryKind::File, *size, *mtime);
                if req.kind() == JobKind::CrossHostMove {
                    self.remove(req.source_host(), src);
                }
                Ok(())
            }
            FilePlan::Move { src, dst, overwrite, .. } => self.rename(req.host(), src, dst, *overwrite),
        };
        match result {
            Ok(()) => self.queue.file_done(job, file, self.now()),
            Err(e) => self.queue.file_failed(job, file, e.to_string(), self.now()),
        }
    }

    // ---- the overlay --------------------------------------------------------------------

    /// What the parent folder lists at `path`.
    fn entry_at(&self, host: &HostId, path: &RemotePath) -> Option<Entry> {
        let name = path.file_name()?;
        self.list(host, &path.parent()?).ok()?.into_iter().find(|e| e.name == name)
    }

    /// An entry of `kind` appears at `path` (replacing what was there).
    fn put(&mut self, host: &HostId, path: &RemotePath, kind: EntryKind, size: u64, mtime: Option<i64>) {
        let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else { return };
        let entry = Entry { name: name.to_owned(), kind, size, mtime, mode: default_mode(kind) };
        self.removed.remove(&(host.clone(), path.clone()));
        let list = self.added.entry((host.clone(), folder.clone())).or_default();
        match list.iter_mut().find(|e| e.name == entry.name) {
            Some(e) => *e = entry,
            None => list.push(entry),
        }
        self.touched.insert((host.clone(), folder));
    }

    /// Makes the folder `path` unless one is there; anything else in the way is an error (as
    /// the real `create_dirs`).
    fn mkdir(&mut self, host: &HostId, path: &RemotePath, mtime: Option<i64>) -> Result<(), Error> {
        match self.entry_at(host, path) {
            Some(e) if e.kind.is_dir_like() => Ok(()),
            Some(_) => Err(Error::AlreadyExists(path.clone())),
            None => {
                self.created.insert((host.clone(), path.clone()));
                self.put(host, path, EntryKind::Dir, 4096, mtime);
                Ok(())
            }
        }
    }

    /// `path` and everything below it are gone.
    fn remove(&mut self, host: &HostId, path: &RemotePath) {
        let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else { return };
        if let Some(list) = self.added.get_mut(&(host.clone(), folder.clone())) {
            list.retain(|e| e.name != name);
        }
        let below = |k: &Key| &k.0 == host && k.1.starts_with(path);
        self.created.retain(|k| !below(k));
        self.added.retain(|k, _| !below(k));
        self.removed.retain(|k| !below(k));
        self.removed.insert((host.clone(), path.clone()));
        self.touched.insert((host.clone(), folder));
    }

    /// A same-host move: the item and its tree go from `src` to `dst` (or, without Replace, a
    /// free name beside it if `dst` is taken); on Replace a folder is merged into a folder, and
    /// a file and a folder never replace each other.
    fn rename(&mut self, host: &HostId, src: &RemotePath, dst: &RemotePath, overwrite: bool) -> Result<(), Error> {
        let entry = self.entry_at(host, src).ok_or_else(|| Error::NotFound(src.clone()))?;
        let mut target = dst.clone();
        match self.entry_at(host, dst) {
            Some(_) if !overwrite => {
                let (Some(folder), Some(name)) = (dst.parent(), dst.file_name()) else { return Ok(()) };
                let taken = self.list(host, &folder)?;
                target = folder.join(&unique_name(name, |n| taken.iter().any(|e| e.name == n)));
            }
            Some(there) if (there.kind == EntryKind::Dir) != (entry.kind == EntryKind::Dir) => {
                return Err(Error::AlreadyExists(dst.clone()));
            }
            _ => {}
        }
        self.copy_tree(host, src, &entry, &target, 0);
        self.remove(host, src);
        Ok(())
    }

    /// Copies `entry` (at `src`) to `dst` on the same host, folders with everything inside.
    fn copy_tree(&mut self, host: &HostId, src: &RemotePath, entry: &Entry, dst: &RemotePath, depth: usize) {
        if entry.kind != EntryKind::Dir || depth >= MAX_DEPTH {
            self.put(host, dst, entry.kind, entry.size, entry.mtime);
            return;
        }
        if self.mkdir(host, dst, entry.mtime).is_err() {
            return;
        }
        for child in self.list(host, src).unwrap_or_default() {
            self.copy_tree(host, &src.join(&child.name), &child, &dst.join(&child.name), depth + 1);
        }
    }

    /// Removes the folders under `root` (and `root`) that are empty, deepest first.
    fn remove_empty_dirs(&mut self, host: &HostId, root: &RemotePath, depth: usize) {
        let Ok(entries) = self.list(host, root) else { return };
        if depth < MAX_DEPTH {
            for e in entries.iter().filter(|e| e.kind == EntryKind::Dir) {
                self.remove_empty_dirs(host, &root.join(&e.name), depth + 1);
            }
        }
        if self.list(host, root).is_ok_and(|e| e.is_empty()) {
            self.remove(host, root);
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
