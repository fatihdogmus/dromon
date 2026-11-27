//! Fixtures: a data-driven fake backend, so the real app runs offline and headless on hosts,
//! folder trees, listing errors and scripted sign-in prompts written down as JSON
//! (`docs/fixtures.md` is the format's reference).
//!
//! - [`parse`] reads a fixture (versioned JSON) into a [`Fixture`], failing with an error that
//!   names the JSON path (`servers.prod.tree./var: entry 3: unknown kind "folder"`).
//! - [`FixtureBackend`] answers the UI's commands from it synchronously, with the same
//!   `ServiceEvent`s the real service emits (status changes driven by the service's own
//!   lifecycle machine, prompts with the real retry errors, log lines, 512-entry pages), to
//!   every main window as the service does (`windows.rs`).
//! - [`demo`] is the built-in demo (`assets/fixtures/demo.json`): what scenes, the in-crate tests
//!   and `DROMON_MOCK=1` run on.
//! - [`transfers::Engine`] runs transfers on it: the real queue (`dromon_transfer::queue`) with
//!   fake, instant I/O against the folder trees; finished uploads show in later listings.
//! - [`tree::Tree`] is the folder trees now: the fixture's, with what transfers and file
//!   operations (new folder, rename, delete) changed in them, in one overlay that both use.
//! - `changes.rs`: a server's scripted changes "made elsewhere", applied one step per poll or
//!   refresh, which the backend answers as the service's polling would (§4.5).
//! - `preview.rs`: what files hold when previewed (Space): generated from their names and sizes.
//!
//! `dromon --fixture <file.json>` runs the app on a fixture: no service thread, no ssh, no ssh
//! config; the fixture file is the only thing it reads.

mod backend;
mod changes;
mod generate;
mod hostbook;
mod parse;
mod preview;
mod search;
mod sizes;
pub mod transfers;
mod tree;
mod windows;

#[cfg(test)]
mod backend_tests;
#[cfg(test)]
mod changes_tests;
#[cfg(test)]
mod fileops_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transfers_tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};

use dromon_core::{Entry, EntryKind, Error, HostSpec, RemotePath};
use dromon_transfer::JobView;

pub use generate::Generator;
pub use parse::parse;
pub use windows::FixtureBackend;

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
    /// Changes made "elsewhere", one step per poll or refresh of the folder on screen, in
    /// order (`changes.rs`).
    pub changes: Vec<Vec<Change>>,
}

/// One change someone else makes on a server, for polling to notice (`docs/fixtures.md`,
/// "Changes elsewhere").
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// An entry appears at the path (replacing what was there).
    Add { path: RemotePath, entry: Entry },
    /// The path and everything below it go.
    Remove { path: RemotePath },
    /// The entry at the path gets a new size or date.
    Modify { path: RemotePath, size: Option<u64>, mtime: Option<i64> },
}

/// One sign-in step, as ssh would ask it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// An unknown host key: trusting it once remembers it (as `known_hosts` would).
    HostKey {
        key_type: String,
        fingerprint: String,
    },
    Password {
        prompt: Option<String>,
        accept: String,
    },
    Passphrase {
        key_path: String,
        accept: String,
    },
    /// Keyboard-interactive and everything else (a verification code): never remembered.
    Code {
        prompt: Option<String>,
        accept: String,
    },
    /// A yes/no question; anything but yes refuses.
    Confirm {
        text: String,
    },
    /// A toast with nothing to answer, up until the connection settles.
    Notice {
        text: String,
    },
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
    Entry {
        name,
        kind: EntryKind::Dir,
        size: default_size(EntryKind::Dir),
        mtime: None,
        mode: default_mode(EntryKind::Dir),
        owner: None,
        group: None,
    }
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
