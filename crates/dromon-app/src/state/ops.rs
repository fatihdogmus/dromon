//! File operations the UI started (§5 M3: new folder, rename, delete) and how each is going,
//! from the service's `FileOp*` events. Plain data: the toast and the sheets read it.
//!
//! The UI calls [`Ops::begin`] when it sends a [`Command::FileOp`](super::Command::FileOp); the
//! events for it arrive through [`App::apply`](super::App::apply). Events for ids it never began
//! (or already forgot) are ignored.

use dromon_core::{Error, HostId};

use super::service::{FileOp, FileOpKind, OpId};

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;

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

    /// `FileOpProgress`.
    pub fn progress(&mut self, id: OpId, host: &HostId, done: u64, total: Option<u64>) {
        if let Some(o) = self.get_mut(id, host) {
            o.done = done;
            o.total = total;
        }
    }

    /// `FileOpDone`.
    pub fn done(&mut self, id: OpId, host: &HostId) {
        if let Some(o) = self.get_mut(id, host) {
            o.finished = true;
            if let Some(total) = o.total {
                o.done = total;
            }
        }
    }

    /// `FileOpFailed`.
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
