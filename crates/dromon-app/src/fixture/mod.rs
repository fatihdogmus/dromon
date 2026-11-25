//! Fixtures: a data-driven fake backend, so the real app runs offline and headless on hosts,
//! folder trees, listing errors and scripted sign-in prompts written down as JSON
//! (`docs/fixtures.md` is the format's reference).
//!
//! - [`parse`] reads a fixture (versioned JSON) into a [`Fixture`], failing with an error that
//!   names the JSON path (`servers.prod.tree./var: entry 3: unknown kind "folder"`).
//! - [`FixtureBackend`] answers the UI's commands from it synchronously, with the same
//!   `ServiceEvent`s the real service emits (status changes driven by the service's own
//!   lifecycle machine, prompts with the real retry errors, log lines, 512-entry pages).
//! - [`demo`] is the built-in demo (`assets/fixtures/demo.json`): what scenes, the in-crate tests
//!   and `DROMON_MOCK=1` run on.
//! - [`transfers::Engine`] runs transfers on it: the real queue (`dromon_transfer::queue`) with
//!   fake, instant I/O against the folder trees; finished uploads show in later listings.
//! - [`Changes`] holds what new folders, renames and deletes did to the trees, on top of the
//!   engine's listings.
//!
//! `dromon --fixture <file.json>` runs the app on a fixture: no service thread, no ssh, no ssh
//! config; the fixture file is the only thing it reads.

mod backend;
mod generate;
mod parse;
pub mod transfers;

#[cfg(test)]
mod backend_tests;
#[cfg(test)]
mod fileops_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transfers_tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};

use dromon_core::{Entry, EntryKind, Error, HostId, HostSpec, RemotePath};
use dromon_transfer::JobView;

pub use backend::FixtureBackend;
pub use generate::Generator;
pub use parse::parse;

/// Ids of the fixture's own job snapshots start above this, clear of the ids a session's jobs get
/// (which count from 1).
pub const FIXTURE_JOBS: u64 = 1 << 32;

use crate::state::{App, HostDirectory};

/// The built-in demo's source.
pub const DEMO_JSON: &str = include_str!("../../assets/fixtures/demo.json");

/// The demo's clock (2026-11-25 14:00:00 UTC): what every scene and test reads.
pub const DEMO_NOW: i64 = 1_795_615_200;

/// The fixture format this build reads.
pub const VERSION: u64 = 1;

/// A whole fixture: the sidebar's hosts and what each server does.
#[derive(Clone, Debug, PartialEq)]
pub struct Fixture {
    /// Seconds since the epoch that relative dates (and log stamps) are measured from; the
    /// system clock when unset.
    pub now: Option<i64>,
    pub hosts: Vec<HostSpec>,
    /// By host alias. A host without a server can't be resolved.
    pub servers: BTreeMap<String, Server>,
    /// Transfer jobs already in the popover when the app starts, newest first: pictures of
    /// mid-progress states, which never move by themselves.
    pub transfers: Vec<JobView>,
}

/// One server: where sessions start, how sign-in goes, and its folders.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub home: RemotePath,
    /// Asked in order on every connection attempt.
    pub auth: Vec<Step>,
    /// Folders by normalized absolute path.
    pub tree: BTreeMap<RemotePath, Node>,
    /// ssh's last words when the connection fails after sign-in.
    pub fail: Option<String>,
}

/// One sign-in step, as ssh would ask it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// An unknown host key: trusting it once remembers it (as `known_hosts` would).
    HostKey { key_type: String, fingerprint: String },
    Password { prompt: Option<String>, accept: String },
    Passphrase { key_path: String, accept: String },
    /// Keyboard-interactive and everything else (a verification code): never remembered.
    Code { prompt: Option<String>, accept: String },
    /// A yes/no question; anything but yes refuses.
    Confirm { text: String },
    /// A toast with nothing to answer, up until the connection settles.
    Notice { text: String },
}

/// What a folder path holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Entries(Vec<Item>),
    Error(TreeError),
}

/// One line of a folder's listing: an entry, or a generator of many.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Entry(Entry),
    Generate(Generator),
}

/// What listing a folder can fail with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeError {
    PermissionDenied,
    NotFound,
    /// The SFTP channel stops answering: the connection is lost.
    ConnectionLost,
}

impl TreeError {
    pub fn to_error(self, path: &RemotePath) -> Error {
        match self {
            TreeError::PermissionDenied => Error::PermissionDenied(path.clone()),
            TreeError::NotFound => Error::NotFound(path.clone()),
            TreeError::ConnectionLost => Error::ConnectionLost,
        }
    }

    /// The tree error behind a listing's `e` (anything else lists as not found).
    pub fn of(e: &Error) -> TreeError {
        match e {
            Error::PermissionDenied(_) => TreeError::PermissionDenied,
            Error::ConnectionLost => TreeError::ConnectionLost,
            _ => TreeError::NotFound,
        }
    }
}

impl Fixture {
    pub fn host_directory(&self) -> HostDirectory {
        HostDirectory { hosts: self.hosts.clone() }
    }

    pub fn host(&self, alias: &str) -> Option<&HostSpec> {
        self.hosts.iter().find(|h| h.alias == alias)
    }

    pub fn server(&self, alias: &str) -> Option<&Server> {
        self.servers.get(alias)
    }
}

/// Default size and mode of an entry of `kind` that doesn't give its own.
pub fn default_size(kind: EntryKind) -> u64 {
    if kind == EntryKind::Dir { 4096 } else { 0 }
}

/// The `st_mode` type bits of `kind`, and its default permissions.
pub fn default_mode(kind: EntryKind) -> Option<u32> {
    match kind {
        EntryKind::Dir => Some(0o040_755),
        EntryKind::File => Some(0o100_644),
        EntryKind::Symlink { .. } => Some(0o120_777),
        EntryKind::Other => None,
    }
}

impl Server {
    /// Lists `path` (see `docs/fixtures.md`, "Folders"):
    /// 1. a listed path gives its entries (generators expanded) or its error;
    /// 2. a path that leads to listed folders or to home lists just those subfolders;
    /// 3. a folder its parent lists (a folder, or a link to one) is empty;
    /// 4. anything else doesn't exist.
    pub fn list(&self, path: &RemotePath) -> Result<Vec<Entry>, Error> {
        match self.tree.get(path) {
            Some(Node::Entries(items)) => return Ok(expand(items, path)),
            Some(Node::Error(e)) => return Err(e.to_error(path)),
            None => {}
        }
        let implied = self.implied_children(path);
        if !implied.is_empty() || path.is_root() || *path == self.home {
            return Ok(implied.into_iter().map(implied_dir).collect());
        }
        let parent = path.parent().ok_or_else(|| Error::NotFound(path.clone()))?;
        let name = path.file_name().unwrap_or_default();
        match self.list(&parent) {
            Ok(siblings) if siblings.iter().any(|e| e.name == name && e.kind.is_dir_like()) => Ok(Vec::new()),
            _ => Err(Error::NotFound(path.clone())),
        }
    }

    /// The names under `path` on the way to a listed folder or home.
    fn implied_children(&self, path: &RemotePath) -> BTreeSet<String> {
        let depth = path.components().count();
        self.tree
            .keys()
            .chain(std::iter::once(&self.home))
            .filter(|k| *k != path && k.starts_with(path))
            .filter_map(|k| k.components().nth(depth).map(str::to_owned))
            .collect()
    }
}

fn implied_dir(name: String) -> Entry {
    Entry { name, kind: EntryKind::Dir, size: default_size(EntryKind::Dir), mtime: None, mode: default_mode(EntryKind::Dir) }
}

/// A folder's entries with its generators run, in order.
fn expand(items: &[Item], folder: &RemotePath) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut generators = 0;
    for item in items {
        match item {
            Item::Entry(e) => out.push(e.clone()),
            Item::Generate(g) => {
                out.extend(g.entries(folder, generators));
                generators += 1;
            }
        }
    }
    out
}

/// What file operations (new folder, rename, delete) changed in the servers' folders, over the
/// listings of the transfer engine ([`transfers::Engine::list`]: the tree plus finished
/// uploads), which stay the base.
///
/// - A folder an operation changes is copied here whole ("changed") and edited; listing it gives
///   the copy. What the engine changes in it later (an upload finishing) is carried over: each
///   listing compares the engine's listing with the one the copy was last based on and applies
///   the difference.
/// - Below a changed folder, a path is a folder only if its parent lists it as one (so deleted
///   and renamed-away folders are gone, with everything inside them); otherwise the engine lists
///   it as before.
/// - A renamed folder takes its whole subtree along (copied as changed folders, errors kept).
///
/// The engine's own lookups (upload name clashes, download scans) don't see these changes.
#[derive(Debug, Default)]
pub struct Changes {
    folders: BTreeMap<(HostId, RemotePath), Changed>,
}

#[derive(Debug)]
struct Changed {
    /// The engine's listing this copy last took in (`None`: the engine couldn't list it).
    base: Option<Vec<Entry>>,
    /// The folder now, or what listing it fails with.
    now: Result<Vec<Entry>, TreeError>,
}

impl Changed {
    /// Applies what the engine changed since `base` (entries added, replaced or gone).
    fn rebase(&mut self, theirs: Option<Vec<Entry>>) {
        let Some(theirs) = theirs else { return };
        let Some(base) = self.base.replace(theirs.clone()) else { return };
        let Ok(now) = &mut self.now else { return };
        if base == theirs {
            return;
        }
        let before: BTreeMap<&str, &Entry> = base.iter().map(|e| (e.name.as_str(), e)).collect();
        for e in theirs.iter().filter(|e| before.get(e.name.as_str()) != Some(e)) {
            match now.iter_mut().find(|x| x.name == e.name) {
                Some(x) => *x = e.clone(),
                None => now.push(e.clone()),
            }
        }
        let after: BTreeSet<&str> = theirs.iter().map(|e| e.name.as_str()).collect();
        now.retain(|x| after.contains(x.name.as_str()) || !before.contains_key(x.name.as_str()));
    }
}

/// `e` from looking `path` up through its folder: about `path` itself.
fn for_path(e: Error, path: &RemotePath) -> Error {
    match e {
        Error::NotFound(_) => Error::NotFound(path.clone()),
        Error::PermissionDenied(_) => Error::PermissionDenied(path.clone()),
        other => other,
    }
}

/// `paths` without repeats and without paths inside another one of them, in order.
fn outermost(paths: &[RemotePath]) -> Vec<RemotePath> {
    let mut out: Vec<RemotePath> = Vec::new();
    for p in paths {
        if !paths.iter().any(|q| q != p && p.starts_with(q)) && !out.contains(p) {
            out.push(p.clone());
        }
    }
    out
}

impl Changes {
    /// True when `path` is a changed folder or lies inside one.
    fn covers(&self, host: &HostId, path: &RemotePath) -> bool {
        self.folders.keys().any(|(h, k)| h == host && path.starts_with(k))
    }

    /// `path` as the server lists it now.
    pub fn list(&mut self, engine: &transfers::Engine, host: &HostId, path: &RemotePath) -> Result<Vec<Entry>, Error> {
        if !self.covers(host, path) {
            return engine.list(host, path);
        }
        if let Some(f) = self.folders.get_mut(&(host.clone(), path.clone())) {
            f.rebase(engine.list(host, path).ok());
            return f.now.clone().map_err(|e| e.to_error(path));
        }
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return engine.list(host, path) };
        let siblings = self.list(engine, host, &parent).map_err(|e| for_path(e, path))?;
        if !siblings.iter().any(|e| e.name == name && e.kind.is_dir_like()) {
            return Err(Error::NotFound(path.clone()));
        }
        match engine.list(host, path) {
            // Not the engine's (moved here): empty, as the tree's unlisted folders are.
            Err(Error::NotFound(_)) => Ok(Vec::new()),
            other => other,
        }
    }

    /// The changed copy of `dir`, made now if it isn't one yet; `dir`'s error if it can't be
    /// listed.
    fn edit(&mut self, engine: &transfers::Engine, host: &HostId, dir: &RemotePath) -> Result<&mut Vec<Entry>, Error> {
        let now = self.list(engine, host, dir);
        let key = (host.clone(), dir.clone());
        let f = self.folders.entry(key).or_insert_with(|| Changed { base: engine.list(host, dir).ok(), now: now.map_err(|e| TreeError::of(&e)) });
        f.now.as_mut().map_err(|e| e.to_error(dir))
    }

    /// Forgets the changed folders at and below `path` (it's gone, or new).
    fn forget(&mut self, host: &HostId, path: &RemotePath) {
        self.folders.retain(|(h, k), _| !(h == host && k.starts_with(path)));
    }

    /// Every folder at and below `dir` (not through links), relative to it, as listed now.
    fn subtree(&mut self, engine: &transfers::Engine, host: &HostId, dir: &RemotePath) -> Vec<(String, Result<Vec<Entry>, TreeError>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let listed = self.list(engine, host, &d);
            if let Ok(entries) = &listed {
                stack.extend(entries.iter().filter(|e| e.kind == EntryKind::Dir).map(|e| d.join(&e.name)));
            }
            let rel = d.as_str().get(dir.as_str().len()..).unwrap_or("").trim_start_matches('/').to_owned();
            out.push((rel, listed.map_err(|e| TreeError::of(&e))));
        }
        out
    }

    /// Creates the folder `path`, stamped `mtime`.
    pub fn mkdir(&mut self, engine: &transfers::Engine, host: &HostId, path: &RemotePath, mtime: Option<i64>) -> Result<(), Error> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return Err(Error::AlreadyExists(path.clone())) };
        let siblings = self.list(engine, host, &parent).map_err(|e| for_path(e, path))?;
        if siblings.iter().any(|e| e.name == name) {
            return Err(Error::AlreadyExists(path.clone()));
        }
        let kind = EntryKind::Dir;
        self.edit(engine, host, &parent)?.push(Entry { name: name.to_owned(), kind, size: default_size(kind), mtime, mode: default_mode(kind) });
        self.forget(host, path);
        let base = engine.list(host, path).ok();
        self.folders.insert((host.clone(), path.clone()), Changed { base, now: Ok(Vec::new()) });
        Ok(())
    }

    /// Renames `from` to `to` (refusing to replace anything), as OpenSSH's plain rename does.
    pub fn rename(&mut self, engine: &transfers::Engine, host: &HostId, from: &RemotePath, to: &RemotePath) -> Result<(), Error> {
        let (Some(from_dir), Some(from_name)) = (from.parent(), from.file_name()) else { return Err(Error::PermissionDenied(from.clone())) };
        let siblings = self.list(engine, host, &from_dir).map_err(|e| for_path(e, from))?;
        let Some(mut entry) = siblings.into_iter().find(|e| e.name == from_name) else { return Err(Error::NotFound(from.clone())) };
        let (Some(to_dir), Some(to_name)) = (to.parent(), to.file_name()) else { return Err(Error::AlreadyExists(to.clone())) };
        let targets = self.list(engine, host, &to_dir).map_err(|e| for_path(e, from))?;
        if targets.iter().any(|e| e.name == to_name) {
            return Err(Error::AlreadyExists(to.clone()));
        }
        if to.starts_with(from) {
            return Err(Error::Io(format!("Couldn't move {from}: Invalid argument")));
        }
        let moved = if entry.kind == EntryKind::Dir { self.subtree(engine, host, from) } else { Vec::new() };
        self.edit(engine, host, &from_dir)?.retain(|e| e.name != from_name);
        entry.name = to_name.to_owned();
        self.edit(engine, host, &to_dir)?.push(entry);
        self.forget(host, from);
        self.forget(host, to);
        for (rel, now) in moved {
            let path = to.join(&rel);
            let base = engine.list(host, &path).ok();
            self.folders.insert((host.clone(), path), Changed { base, now });
        }
        Ok(())
    }

    /// Deletes `paths`, folders with everything inside them, links themselves. Everything is
    /// counted first, as the service does, so a folder that can't be listed stops the delete
    /// before anything goes. Returns how many entries went.
    pub fn delete(&mut self, engine: &transfers::Engine, host: &HostId, paths: &[RemotePath]) -> Result<u64, Error> {
        let roots = outermost(paths);
        let mut total = 0;
        for root in &roots {
            let (Some(dir), Some(name)) = (root.parent(), root.file_name()) else { return Err(Error::PermissionDenied(root.clone())) };
            let siblings = self.list(engine, host, &dir).map_err(|e| for_path(e, root))?;
            let entry = siblings.iter().find(|e| e.name == name).ok_or_else(|| Error::NotFound(root.clone()))?;
            total += if entry.kind == EntryKind::Dir { self.count(engine, host, root)? } else { 1 };
        }
        for root in &roots {
            if let (Some(dir), Some(name)) = (root.parent(), root.file_name()) {
                self.edit(engine, host, &dir)?.retain(|e| e.name != name);
                self.forget(host, root);
            }
        }
        Ok(total)
    }

    /// Entries at and below the folder `dir`, not through links.
    fn count(&mut self, engine: &transfers::Engine, host: &HostId, dir: &RemotePath) -> Result<u64, Error> {
        let mut n = 0;
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            n += 1;
            for e in self.list(engine, host, &d)? {
                match e.kind {
                    EntryKind::Dir => stack.push(d.join(&e.name)),
                    _ => n += 1,
                }
            }
        }
        Ok(n)
    }
}

/// The built-in demo, parsed once.
pub fn demo() -> Arc<Fixture> {
    static DEMO: OnceLock<Arc<Fixture>> = OnceLock::new();
    DEMO.get_or_init(|| Arc::new(parse(DEMO_JSON).unwrap_or_else(|e| panic!("assets/fixtures/demo.json: {e}")))).clone()
}

static ACTIVE: OnceLock<Arc<Fixture>> = OnceLock::new();

/// Makes `fx` the fixture scenes start from (`--fixture` with `--scene`). Once per process.
pub fn activate(fx: Arc<Fixture>) {
    ACTIVE.set(fx).ok();
}

/// The fixture the app runs on: `--fixture`'s, else the demo.
pub fn active() -> Arc<Fixture> {
    ACTIVE.get().cloned().unwrap_or_else(demo)
}

/// The fixture's clock in milliseconds, for log stamps.
static WALL_MS: AtomicI64 = AtomicI64::new(0);

fn fixed_wall_ms() -> i64 {
    WALL_MS.load(Ordering::Relaxed)
}

/// The app on `fx`: its hosts, its clock (or the system's), and a [`FixtureBackend`].
pub fn app(fx: Arc<Fixture>) -> App {
    let now = fx.now.unwrap_or_else(crate::now_secs);
    let fixed = fx.now;
    let jobs = fx.transfers.clone();
    let mut app = App::new(fx.host_directory(), now, Box::new(FixtureBackend::new(fx)));
    if !jobs.is_empty() {
        app.transfers.set(jobs);
    }
    if let Some(now) = fixed {
        WALL_MS.store(now * 1000, Ordering::Relaxed);
        app.wall_ms = fixed_wall_ms;
    }
    app
}

/// Reads and parses the fixture at `path`; the error is what to print.
pub fn load(path: &str) -> Result<Fixture, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("can't read the fixture {path}: {e}"))?;
    parse(&text).map_err(|e| format!("invalid fixture {path}: {e}"))
}
