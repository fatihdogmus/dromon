//! [`Tree`]: the servers' folders as they are now: the fixture's trees with what transfers and
//! file operations changed in them, in one overlay that both read and write.
//!
//! - A folder something changes is copied whole into the overlay ("changed") and edited there;
//!   listing it gives the copy (or the error it failed with, kept when a folder moves).
//! - Below a changed folder, a path is a folder only if its parent lists it as one (so deleted
//!   and moved-away folders are gone, with everything inside them); one that the fixture can't
//!   list is empty (it was made or moved there), else the fixture lists it as before.
//! - Elsewhere the fixture's tree answers ([`Server::list`](super::Server::list)).
//!
//! The file operations (new folder, rename, delete) fail as a server does; the transfer
//! engine's changes (a file landing, folders a copy makes, a move with its merge on Replace,
//! tidying emptied folders) are plainer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dromon_core::{Entry, EntryKind, Error, HostId, RemotePath};
use dromon_transfer::unique_name;

use super::{Fixture, TreeError, default_mode, default_size};

/// How deep a moved tree or a tidy goes before it stops looking.
const MAX_DEPTH: usize = 64;

type Key = (HostId, RemotePath);

/// The fixture's folder trees with what changed in them.
pub struct Tree {
    fixture: Arc<Fixture>,
    /// Changed folders, by host and path: what each lists now, or what listing it fails with.
    folders: BTreeMap<Key, Result<Vec<Entry>, TreeError>>,
    /// Folders whose listing changed since [`Tree::take_touched`].
    touched: BTreeSet<Key>,
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
pub(super) fn outermost(paths: &[RemotePath]) -> Vec<RemotePath> {
    let mut out: Vec<RemotePath> = Vec::new();
    for p in paths {
        if !paths.iter().any(|q| q != p && p.starts_with(q)) && !out.contains(p) {
            out.push(p.clone());
        }
    }
    out
}

fn dir_entry(name: &str, mtime: Option<i64>) -> Entry {
    let kind = EntryKind::Dir;
    Entry { name: name.to_owned(), kind, size: default_size(kind), mtime, mode: default_mode(kind) }
}

impl Tree {
    pub fn new(fixture: Arc<Fixture>) -> Tree {
        Tree { fixture, folders: BTreeMap::new(), touched: BTreeSet::new() }
    }

    /// The folders whose listing changed since the last call.
    pub fn take_touched(&mut self) -> BTreeSet<Key> {
        std::mem::take(&mut self.touched)
    }

    /// `path` as the fixture lists it, unchanged.
    fn base(&self, host: &HostId, path: &RemotePath) -> Result<Vec<Entry>, Error> {
        match self.fixture.server(host.as_str()) {
            Some(s) => s.list(path),
            None => Err(Error::NotFound(path.clone())),
        }
    }

    /// `path` as the server lists it now.
    pub fn list(&self, host: &HostId, path: &RemotePath) -> Result<Vec<Entry>, Error> {
        let key = |p: &RemotePath| (host.clone(), p.clone());
        if let Some(now) = self.folders.get(&key(path)) {
            return now.clone().map_err(|e| e.to_error(path));
        }
        if !path.ancestors_from_root().iter().any(|a| self.folders.contains_key(&key(a))) {
            return self.base(host, path);
        }
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return self.base(host, path) };
        let siblings = self.list(host, &parent).map_err(|e| for_path(e, path))?;
        if !siblings.iter().any(|e| e.name == name && e.kind.is_dir_like()) {
            return Err(Error::NotFound(path.clone()));
        }
        match self.base(host, path) {
            // Not the fixture's (made or moved here): empty, as the tree's unlisted folders are.
            Err(Error::NotFound(_)) => Ok(Vec::new()),
            other => other,
        }
    }

    /// What the parent folder lists at `path`.
    pub fn entry_at(&self, host: &HostId, path: &RemotePath) -> Option<Entry> {
        let name = path.file_name()?;
        self.list(host, &path.parent()?).ok()?.into_iter().find(|e| e.name == name)
    }

    // ---- the overlay's edits ---------------------------------------------------------------

    /// The changed copy of `dir`, made now if it isn't one yet; `dir`'s error if it can't be
    /// listed.
    fn edit(&mut self, host: &HostId, dir: &RemotePath) -> Result<&mut Vec<Entry>, Error> {
        let key = (host.clone(), dir.clone());
        if !self.folders.contains_key(&key) {
            let now = self.list(host, dir)?;
            self.folders.insert(key.clone(), Ok(now));
        }
        self.touched.insert(key.clone());
        match self.folders.entry(key).or_insert_with(|| Ok(Vec::new())) {
            Ok(entries) => Ok(entries),
            Err(e) => Err(e.to_error(dir)),
        }
    }

    /// Forgets the changed folders at and below `path` (it's gone, or new).
    fn forget(&mut self, host: &HostId, path: &RemotePath) {
        self.folders.retain(|(h, k), _| !(h == host && k.starts_with(path)));
    }

    /// `entry` appears at `path` under `path`'s name, replacing what was there (and everything
    /// below it).
    fn place(&mut self, host: &HostId, path: &RemotePath, mut entry: Entry) -> Result<(), Error> {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Err(Error::AlreadyExists(path.clone())) };
        entry.name = name.to_owned();
        let entries = self.edit(host, &dir)?;
        match entries.iter_mut().find(|e| e.name == entry.name) {
            Some(e) => *e = entry,
            None => entries.push(entry),
        }
        self.forget(host, path);
        Ok(())
    }

    /// A new, empty folder at `path`.
    fn add_dir(&mut self, host: &HostId, path: &RemotePath, mtime: Option<i64>) -> Result<(), Error> {
        self.place(host, path, dir_entry("", mtime))?;
        self.folders.insert((host.clone(), path.clone()), Ok(Vec::new()));
        Ok(())
    }

    /// A file of `size` bytes lands at `path` (replacing what was there).
    pub fn put_file(&mut self, host: &HostId, path: &RemotePath, size: u64, mtime: Option<i64>) -> Result<(), Error> {
        let kind = EntryKind::File;
        self.place(host, path, Entry { name: String::new(), kind, size, mtime, mode: default_mode(kind) })
    }

    /// Makes the folder `path` unless one is there; anything else in the way is an error (as
    /// the real `create_dirs`).
    pub fn ensure_dir(&mut self, host: &HostId, path: &RemotePath, mtime: Option<i64>) -> Result<(), Error> {
        match self.entry_at(host, path) {
            Some(e) if e.kind.is_dir_like() => Ok(()),
            Some(_) => Err(Error::AlreadyExists(path.clone())),
            None => self.add_dir(host, path, mtime),
        }
    }

    /// `path` and everything below it are gone.
    pub fn remove(&mut self, host: &HostId, path: &RemotePath) -> Result<(), Error> {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Err(Error::PermissionDenied(path.clone())) };
        self.edit(host, &dir)?.retain(|e| e.name != name);
        self.forget(host, path);
        Ok(())
    }

    /// Every folder at and below `dir` (not through links), relative to it, as listed now.
    fn subtree(&self, host: &HostId, dir: &RemotePath) -> Vec<(String, Result<Vec<Entry>, TreeError>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let listed = self.list(host, &d);
            if let Ok(entries) = &listed {
                stack.extend(entries.iter().filter(|e| e.kind == EntryKind::Dir).map(|e| d.join(&e.name)));
            }
            let rel = d.as_str().get(dir.as_str().len()..).unwrap_or("").trim_start_matches('/').to_owned();
            out.push((rel, listed.map_err(|e| TreeError::of(&e))));
        }
        out
    }

    /// Copies `entry` (at `src`) to `dst`, replacing a file there: a folder with its whole tree
    /// as listed now (folders that can't be listed keep their error).
    fn graft(&mut self, host: &HostId, src: &RemotePath, entry: &Entry, dst: &RemotePath) -> Result<(), Error> {
        let tree = if entry.kind == EntryKind::Dir { self.subtree(host, src) } else { Vec::new() };
        self.place(host, dst, entry.clone())?;
        for (rel, now) in tree {
            self.folders.insert((host.clone(), dst.join(&rel)), now);
        }
        Ok(())
    }

    /// Copies `entry` (at `src`) to `dst` on the same host: onto a free name (or a file) with
    /// its tree, onto a folder merged into it. A file and a folder never replace each other.
    fn copy_tree(&mut self, host: &HostId, src: &RemotePath, entry: &Entry, dst: &RemotePath, depth: usize) -> Result<(), Error> {
        let is_dir = entry.kind == EntryKind::Dir;
        match self.entry_at(host, dst) {
            Some(there) if (there.kind == EntryKind::Dir) != is_dir => Err(Error::AlreadyExists(dst.clone())),
            Some(_) if is_dir && depth < MAX_DEPTH => {
                for child in self.list(host, src).unwrap_or_default() {
                    // A clash below is left as it is; the rest merges.
                    let _ = self.copy_tree(host, &src.join(&child.name), &child, &dst.join(&child.name), depth + 1);
                }
                Ok(())
            }
            _ => self.graft(host, src, entry, dst),
        }
    }

    /// A same-host move: the item and its tree go from `src` to `dst` (or, without Replace, a
    /// free name beside it if `dst` is taken); on Replace a folder is merged into a folder, and
    /// a file and a folder never replace each other.
    pub fn move_to(&mut self, host: &HostId, src: &RemotePath, dst: &RemotePath, overwrite: bool) -> Result<(), Error> {
        let entry = self.entry_at(host, src).ok_or_else(|| Error::NotFound(src.clone()))?;
        let mut target = dst.clone();
        if !overwrite && self.entry_at(host, dst).is_some() {
            let (Some(folder), Some(name)) = (dst.parent(), dst.file_name()) else { return Ok(()) };
            let taken = self.list(host, &folder)?;
            target = folder.join(&unique_name(name, |n| taken.iter().any(|e| e.name == n)));
        }
        self.copy_tree(host, src, &entry, &target, 0)?;
        self.remove(host, src)
    }

    /// Removes the folders under `root` (and `root`) that are empty, deepest first.
    pub fn remove_empty_dirs(&mut self, host: &HostId, root: &RemotePath, depth: usize) {
        let Ok(entries) = self.list(host, root) else { return };
        if depth < MAX_DEPTH {
            for e in entries.iter().filter(|e| e.kind == EntryKind::Dir) {
                self.remove_empty_dirs(host, &root.join(&e.name), depth + 1);
            }
        }
        if self.list(host, root).is_ok_and(|e| e.is_empty()) {
            let _ = self.remove(host, root);
        }
    }

    // ---- file operations, failing as a server does -----------------------------------------

    /// New folder: creates `path`, stamped `mtime`.
    pub fn mkdir(&mut self, host: &HostId, path: &RemotePath, mtime: Option<i64>) -> Result<(), Error> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return Err(Error::AlreadyExists(path.clone())) };
        let siblings = self.list(host, &parent).map_err(|e| for_path(e, path))?;
        if siblings.iter().any(|e| e.name == name) {
            return Err(Error::AlreadyExists(path.clone()));
        }
        self.add_dir(host, path, mtime)
    }

    /// Renames `from` to `to` (refusing to replace anything), as OpenSSH's plain rename does.
    pub fn rename(&mut self, host: &HostId, from: &RemotePath, to: &RemotePath) -> Result<(), Error> {
        let (Some(from_dir), Some(from_name)) = (from.parent(), from.file_name()) else { return Err(Error::PermissionDenied(from.clone())) };
        let siblings = self.list(host, &from_dir).map_err(|e| for_path(e, from))?;
        let Some(entry) = siblings.into_iter().find(|e| e.name == from_name) else { return Err(Error::NotFound(from.clone())) };
        let (Some(to_dir), Some(to_name)) = (to.parent(), to.file_name()) else { return Err(Error::AlreadyExists(to.clone())) };
        let targets = self.list(host, &to_dir).map_err(|e| for_path(e, from))?;
        if targets.iter().any(|e| e.name == to_name) {
            return Err(Error::AlreadyExists(to.clone()));
        }
        if to.starts_with(from) {
            return Err(Error::Io(format!("Couldn't move {from}: Invalid argument")));
        }
        self.graft(host, from, &entry, to)?;
        self.remove(host, from)
    }

    /// Deletes `paths`, folders with everything inside them, links themselves. Everything is
    /// counted first, as the service does, so a folder that can't be listed stops the delete
    /// before anything goes. Returns how many entries went.
    pub fn delete(&mut self, host: &HostId, paths: &[RemotePath]) -> Result<u64, Error> {
        let roots = outermost(paths);
        let mut total = 0;
        for root in &roots {
            let (Some(dir), Some(name)) = (root.parent(), root.file_name()) else { return Err(Error::PermissionDenied(root.clone())) };
            let siblings = self.list(host, &dir).map_err(|e| for_path(e, root))?;
            let entry = siblings.iter().find(|e| e.name == name).ok_or_else(|| Error::NotFound(root.clone()))?;
            total += if entry.kind == EntryKind::Dir { self.count(host, root)? } else { 1 };
        }
        for root in &roots {
            self.remove(host, root)?;
        }
        Ok(total)
    }

    /// Entries at and below the folder `dir`, not through links.
    fn count(&self, host: &HostId, dir: &RemotePath) -> Result<u64, Error> {
        let mut n = 0;
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            n += 1;
            for e in self.list(host, &d)? {
                match e.kind {
                    EntryKind::Dir => stack.push(d.join(&e.name)),
                    _ => n += 1,
                }
            }
        }
        Ok(n)
    }
}
