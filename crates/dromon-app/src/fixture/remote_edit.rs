//! Edit in a local editor on a fixture (`Command::RemoteEdit`, `state/remote_edit.rs`), answered
//! at once against the folder trees, the same on every run. Nothing is written to or read from
//! the disk: a fixture holds no files, so each copy is a made-up path under [`FOLDER`] that only
//! this backend knows of (and a windowed run has nothing there to open).
//!
//! There's no editor and no clock either: each time a window gains focus (`Command::Poll`, as if
//! the user came back from saving in the editor) every file being edited on that host is saved.
//! A save checks the server's copy first, as the service does: when the tree has it with
//! another size or date than the edit last downloaded or uploaded (a step of the server's
//! `changes` that poll applied, a transfer, a rename), it's a conflict. An upload lands as a
//! file of the copy's size dated the fixture's clock; Keep both lands under the next free name,
//! Discard takes the tree's copy. Disconnecting closes the host's edits.

use std::collections::BTreeMap;
use std::path::PathBuf;

use dromon_core::{Error, HostId, RemotePath};
use dromon_transfer::{Side, unique_name};

use super::backend::Core;
use crate::state::{EditChoice, EditId, EditStage, RemoteEditCommand, RemoteEditEvent, ServiceEvent};

#[cfg(test)]
#[path = "remote_edit_tests.rs"]
mod tests;

/// The made-up folder the fixture's local copies are under.
pub const FOLDER: &str = "dromon-editing";

/// The files being edited on a fixture.
#[derive(Debug, Default)]
pub struct Edits {
    open: BTreeMap<EditId, Copy>,
    /// The fixture's clock: what uploads are dated.
    now: Option<i64>,
}

/// One file being edited, as the fixture keeps it.
#[derive(Debug)]
struct Copy {
    host: HostId,
    path: RemotePath,
    /// The tree's copy as the edit last downloaded or uploaded it.
    base: Side,
    /// What the last save is waiting for.
    wait: Wait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    Watching,
    Asking,
    Failed,
}

impl Edits {
    /// None yet, uploads dated `now` (the fixture's clock).
    pub fn at(now: Option<i64>) -> Edits {
        Edits { open: BTreeMap::new(), now }
    }

    /// The local copies the fixture holds now (tests check that a stopped edit's goes).
    #[cfg(test)]
    pub fn copies(&self) -> Vec<PathBuf> {
        self.open.values().map(|c| local_of(&c.host, &c.path)).collect()
    }
}

/// The made-up local copy of `path` on `host`: under [`FOLDER`], by host and path.
fn local_of(host: &HostId, path: &RemotePath) -> PathBuf {
    PathBuf::from(FOLDER).join(host.as_str()).join(path.as_str().trim_start_matches('/'))
}

fn event(ev: RemoteEditEvent) -> ServiceEvent {
    ServiceEvent::RemoteEdit(ev)
}

impl Core {
    /// An edit command from any window; every window hears what it leads to.
    pub(super) fn remote_edit(&mut self, cmd: RemoteEditCommand, out: &mut Vec<ServiceEvent>) {
        match cmd {
            RemoteEditCommand::Start { id, host, path } => self.start_edit(id, host, path, out),
            RemoteEditCommand::Stop { id, host } => {
                self.edits.open.remove(&id);
                out.push(event(RemoteEditEvent::Closed { id, host }));
            }
            RemoteEditCommand::Resolve { id, host, choice } => match self.edits.open.get(&id).map(|c| c.wait) {
                Some(Wait::Asking) => self.resolve_edit(id, choice, out),
                Some(_) => {}
                None => out.push(event(RemoteEditEvent::Closed { id, host })),
            },
            RemoteEditCommand::Retry { id, host } => match self.edits.open.get(&id).map(|c| c.wait) {
                Some(Wait::Failed) => self.save_edit(id, out),
                Some(_) => {}
                None => out.push(event(RemoteEditEvent::Closed { id, host })),
            },
        }
    }

    /// The tree's copy of `path` now; `None` when it isn't a file there.
    fn edited_side(&self, host: &HostId, path: &RemotePath) -> Option<Side> {
        let e = self.transfers.tree.entry_at(host, path).filter(|e| !e.kind.is_dir_like())?;
        Some(Side { size: Some(e.size), mtime: e.mtime })
    }

    fn start_edit(&mut self, id: EditId, host: HostId, path: RemotePath, out: &mut Vec<ServiceEvent>) {
        let fail = |error| event(RemoteEditEvent::Failed { id, host: host.clone(), stage: EditStage::Download, error });
        if !self.up(&host) {
            return out.push(fail(Error::ConnectionLost));
        }
        if self.edits.open.values().any(|c| c.host == host && c.path == path) {
            return out.push(fail(Error::Other(format!("{path} is open for editing already"))));
        }
        let Some(base) = self.edited_side(&host, &path) else { return out.push(fail(Error::NotFound(path))) };
        let local = local_of(&host, &path);
        let copy = Copy { host: host.clone(), path: path.clone(), base, wait: Wait::Watching };
        self.edits.open.insert(id, copy);
        out.push(event(RemoteEditEvent::Opened { id, host, path, local }));
    }

    /// The window came back to the app: every file being edited on `host` that isn't waiting
    /// for its conflict sheet is saved.
    pub(super) fn save_edits(&mut self, host: &HostId, out: &mut Vec<ServiceEvent>) {
        let ids: Vec<EditId> = self
            .edits
            .open
            .iter()
            .filter(|(_, c)| &c.host == host && c.wait != Wait::Asking)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.save_edit(id, out);
        }
    }

    /// A save of `id`: a conflict when the tree's copy changed since, else an upload.
    fn save_edit(&mut self, id: EditId, out: &mut Vec<ServiceEvent>) {
        let Some(c) = self.edits.open.get(&id) else { return };
        let (host, path, base) = (c.host.clone(), c.path.clone(), c.base);
        out.push(event(RemoteEditEvent::Uploading { id, host: host.clone() }));
        if !self.up(&host) {
            return self.edit_failed(id, EditStage::Upload, Error::ConnectionLost, out);
        }
        let server = self.edited_side(&host, &path).unwrap_or_default();
        if server != base {
            if let Some(c) = self.edits.open.get_mut(&id) {
                c.wait = Wait::Asking;
            }
            let local = Side { size: base.size, mtime: self.edits.now };
            return out.push(event(RemoteEditEvent::Conflict { id, host, server, local }));
        }
        self.upload_edit(id, path, out);
    }

    /// Uploads `id`'s copy to `to` (its own path, or Keep both's name): a file of the copy's
    /// size dated the fixture's clock.
    fn upload_edit(&mut self, id: EditId, to: RemotePath, out: &mut Vec<ServiceEvent>) {
        let Some(c) = self.edits.open.get(&id) else { return };
        let (host, own, size) = (c.host.clone(), to == c.path, c.base.size.unwrap_or(0));
        let now = self.edits.now;
        if let Err(error) = self.transfers.tree.put_file(&host, &to, size, now) {
            return self.edit_failed(id, EditStage::Upload, error, out);
        }
        if let Some(c) = self.edits.open.get_mut(&id) {
            c.wait = Wait::Watching;
            if own {
                c.base = Side { size: Some(size), mtime: now };
            }
        }
        if let Some(folder) = to.parent() {
            self.relist(&host, folder, out);
        }
        out.push(event(RemoteEditEvent::Saved { id, host, to }));
    }

    fn edit_failed(&mut self, id: EditId, stage: EditStage, error: Error, out: &mut Vec<ServiceEvent>) {
        let Some(c) = self.edits.open.get_mut(&id) else { return };
        if stage == EditStage::Upload {
            c.wait = Wait::Failed;
        }
        out.push(event(RemoteEditEvent::Failed { id, host: c.host.clone(), stage, error }));
    }

    /// The conflict sheet's answer for `id`.
    fn resolve_edit(&mut self, id: EditId, choice: EditChoice, out: &mut Vec<ServiceEvent>) {
        let Some(c) = self.edits.open.get(&id) else { return };
        let (host, path) = (c.host.clone(), c.path.clone());
        match choice {
            EditChoice::Replace => self.upload_edit(id, path, out),
            EditChoice::KeepBoth => {
                let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else { return };
                let taken = self.transfers.tree.list(&host, &folder).unwrap_or_default();
                let free = unique_name(name, |n| taken.iter().any(|e| e.name == n));
                self.upload_edit(id, folder.join(&free), out);
            }
            EditChoice::Discard => match self.edited_side(&host, &path) {
                Some(server) if self.up(&host) => {
                    if let Some(c) = self.edits.open.get_mut(&id) {
                        (c.base, c.wait) = (server, Wait::Watching);
                    }
                    out.push(event(RemoteEditEvent::Reloaded { id, host }));
                }
                Some(_) => self.edit_failed(id, EditStage::Reload, Error::ConnectionLost, out),
                None => self.edit_failed(id, EditStage::Reload, Error::NotFound(path), out),
            },
        }
    }

    /// `host` disconnected: its edits end.
    pub(super) fn close_edits(&mut self, host: &HostId, out: &mut Vec<ServiceEvent>) {
        let ids: Vec<EditId> = self.edits.open.iter().filter(|(_, c)| &c.host == host).map(|(id, _)| *id).collect();
        for id in ids {
            self.edits.open.remove(&id);
            out.push(event(RemoteEditEvent::Closed { id, host: host.clone() }));
        }
    }
}
