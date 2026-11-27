//! File operations the UI started (§5 M3: new folder, rename, delete) and how each is going,
//! from the service's `FileOp*` events. Plain data: the toast and the sheets read it.
//!
//! The UI calls [`Ops::begin`] when it sends a [`Command::FileOp`](super::Command::FileOp); the
//! events for it arrive through [`App::apply`](super::App::apply). Events for ids it never began
//! (or already forgot) are ignored.

use dromon_core::{Error, HostId, RemotePath};

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;

/// Identifies one file operation, chosen by the UI (unique per session).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpId(pub u64);

/// A simple file operation (§5 M3): new folder, rename, delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    /// Create one folder (its parent must exist).
    Mkdir { path: RemotePath },
    /// Rename an item within its folder (`to` is a sibling of `from`); fails with AlreadyExists if `to` exists.
    Rename { from: RemotePath, to: RemotePath },
    /// Delete items; folders recursively. Symlinks are removed themselves, never followed.
    Delete { paths: Vec<RemotePath> },
}

/// What kind of [`FileOp`] it is, without its paths (for trace lines and the ops list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOpKind {
    Mkdir,
    Rename,
    Delete,
}

impl FileOpKind {
    pub fn name(self) -> &'static str {
        match self {
            FileOpKind::Mkdir => "Mkdir",
            FileOpKind::Rename => "Rename",
            FileOpKind::Delete => "Delete",
        }
    }
}

impl FileOp {
    pub fn kind(&self) -> FileOpKind {
        match self {
            FileOp::Mkdir { .. } => FileOpKind::Mkdir,
            FileOp::Rename { .. } => FileOpKind::Rename,
            FileOp::Delete { .. } => FileOpKind::Delete,
        }
    }

    /// Every path the operation changes.
    pub fn affected(&self) -> Vec<&RemotePath> {
        match self {
            FileOp::Mkdir { path } => vec![path],
            FileOp::Rename { from, to } => vec![from, to],
            FileOp::Delete { paths } => paths.iter().collect(),
        }
    }

    /// The paths that no longer exist once the operation succeeded.
    pub fn removed(&self) -> Vec<&RemotePath> {
        match self {
            FileOp::Mkdir { .. } => Vec::new(),
            FileOp::Rename { from, to } if from != to => vec![from],
            FileOp::Rename { .. } => Vec::new(),
            FileOp::Delete { paths } => paths.iter().collect(),
        }
    }

    /// Whether the folder on screen, `on_screen`, is listed again after the operation ended
    /// (`ok`: it succeeded). It is when it's the parent of an affected path, or affected itself
    /// (or lies inside one) — except after a success that removed it (it, or an ancestor, was
    /// deleted or renamed away): a path that's gone isn't listed, and the UI moves away from it.
    pub fn relists(&self, on_screen: &RemotePath, ok: bool) -> bool {
        let touched =
            self.affected().iter().any(|a| a.parent().as_ref() == Some(on_screen) || on_screen.starts_with(a));
        let gone = ok && self.removed().iter().any(|r| on_screen.starts_with(r));
        touched && !gone
    }
}

/// What the service tells the UI about a file operation (`ServiceEvent::FileOp`).
#[derive(Clone, Debug, PartialEq)]
pub enum FileOpEvent {
    /// How far a delete got, at most about ten times a second: while it counts what's there,
    /// `done` is the entries found so far and `total` is `None`; while it removes them, `done`
    /// is the entries removed and `total` all of them.
    Progress { id: OpId, host: HostId, done: u64, total: Option<u64> },
    /// The operation succeeded.
    Done { id: OpId, host: HostId },
    /// The operation stopped at its first error (a delete may have removed some entries).
    Failed { id: OpId, host: HostId, error: Error },
}

/// A `FileOp` event's trace line (`DROMON_TRACE=1`): its kind, host and id only.
pub fn trace_line(ev: &FileOpEvent) -> String {
    match ev {
        FileOpEvent::Progress { id, host, .. } => format!("FileOpProgress host={host} id={}", id.0),
        FileOpEvent::Done { id, host } => format!("FileOpDone host={host} id={}", id.0),
        FileOpEvent::Failed { id, host, .. } => format!("FileOpFailed host={host} id={}", id.0),
    }
}

/// One operation and its latest state.
#[derive(Clone, Debug, PartialEq)]
pub struct OpView {
    pub id: OpId,
    pub host: HostId,
    pub kind: FileOpKind,
    /// How many items it acts on (a delete's selection; 1 otherwise).
    pub items: usize,
    /// A delete's progress: entries found so far while `total` is `None`, then entries removed.
    pub done: u64,
    pub total: Option<u64>,
    /// Why it stopped, once it failed.
    pub error: Option<Error>,
    /// It ended (succeeded or failed).
    pub finished: bool,
}

impl OpView {
    pub fn succeeded(&self) -> bool {
        self.finished && self.error.is_none()
    }
}

/// Every operation of the session, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ops {
    pub ops: Vec<OpView>,
    /// The last id [`Ops::begin`] handed out.
    last_id: u64,
}

impl Ops {
    /// No operations yet; ids are handed out after `base` (each window has its own range).
    pub fn numbered_from(base: u64) -> Ops {
        Ops { ops: Vec::new(), last_id: base }
    }

    /// Records a new operation on `host` and returns the id to send it with.
    pub fn begin(&mut self, host: HostId, op: &FileOp) -> OpId {
        self.last_id += 1;
        let id = OpId(self.last_id);
        let items = match op {
            FileOp::Delete { paths } => paths.len(),
            FileOp::Mkdir { .. } | FileOp::Rename { .. } => 1,
        };
        self.ops.push(OpView { id, host, kind: op.kind(), items, done: 0, total: None, error: None, finished: false });
        id
    }

    pub fn get(&self, id: OpId) -> Option<&OpView> {
        self.ops.iter().find(|o| o.id == id)
    }

    fn get_mut(&mut self, id: OpId, host: &HostId) -> Option<&mut OpView> {
        self.ops.iter_mut().find(|o| o.id == id && o.host == *host && !o.finished)
    }

    /// A file operation event from the service.
    pub fn event(&mut self, ev: FileOpEvent) {
        match ev {
            FileOpEvent::Progress { id, host, done, total } => self.progress(id, &host, done, total),
            FileOpEvent::Done { id, host } => self.done(id, &host),
            FileOpEvent::Failed { id, host, error } => self.failed(id, &host, error),
        }
    }

    /// `FileOpEvent::Progress`.
    pub fn progress(&mut self, id: OpId, host: &HostId, done: u64, total: Option<u64>) {
        if let Some(o) = self.get_mut(id, host) {
            o.done = done;
            o.total = total;
        }
    }

    /// `FileOpEvent::Done`.
    pub fn done(&mut self, id: OpId, host: &HostId) {
        if let Some(o) = self.get_mut(id, host) {
            o.finished = true;
            if let Some(total) = o.total {
                o.done = total;
            }
        }
    }

    /// `FileOpEvent::Failed`.
    pub fn failed(&mut self, id: OpId, host: &HostId, error: Error) {
        if let Some(o) = self.get_mut(id, host) {
            o.finished = true;
            o.error = Some(error);
        }
    }

    /// Operations still running.
    pub fn running(&self) -> impl Iterator<Item = &OpView> {
        self.ops.iter().filter(|o| !o.finished)
    }

    /// Forgets one operation (its toast was dismissed).
    pub fn forget(&mut self, id: OpId) {
        self.ops.retain(|o| o.id != id);
    }

    /// Forgets every finished operation.
    pub fn clear_finished(&mut self) {
        self.ops.retain(|o| !o.finished);
    }
}
