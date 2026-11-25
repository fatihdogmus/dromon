//! Dragging rows within the window (§3.7): onto a folder row or a breadcrumb to move them
//! there (⌥, or Ctrl on Windows and Linux, copies instead), or onto another host in the sidebar
//! to copy them into that host's folder on screen. Escape cancels. The app decides which targets
//! take the drag (`App::drag_over` in `files.rs`); this is the drag itself and what it makes.

use dromon_core::{HostId, RemotePath};
use dromon_transfer::{JobRequest, RemoteItem};

#[cfg(test)]
#[path = "drag_tests.rs"]
mod tests;

/// What a drag can land on. Views attach it to the element with `El::data`, and a hit test
/// under the pointer reads it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragTarget {
    /// A folder row in the list: that subfolder of the folder on screen.
    Folder(String),
    /// A breadcrumb: that folder.
    Crumb(RemotePath),
    /// A host in the sidebar: its folder on screen.
    Host(HostId),
}

/// A target that takes the drag, and where it puts the items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Over {
    pub target: DragTarget,
    /// Copy instead of move (always for another host).
    pub copy: bool,
    /// The host and folder the items go into.
    pub to: HostId,
    pub into: RemotePath,
    /// What the pill calls it: the folder's name, or the host's.
    pub label: String,
}

/// Rows on their way somewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDrag {
    pub host: HostId,
    /// The folder they're in (on screen when the drag started).
    pub folder: RemotePath,
    pub names: Vec<String>,
    pub items: Vec<RemoteItem>,
    /// The target under the pointer, when it takes them.
    pub over: Option<Over>,
}

impl RowDrag {
    /// Whether `path` is one of the dragged items or inside one: nothing goes into itself.
    pub fn contains(&self, path: &RemotePath) -> bool {
        self.items.iter().any(|i| path.starts_with(&i.path))
    }

    /// "Move 3 items to www", "Copy 1 item to prod".
    pub fn pill(&self) -> Option<String> {
        let over = self.over.as_ref()?;
        let verb = if over.copy { "Copy" } else { "Move" };
        let n = self.items.len();
        let what = if n == 1 { "1 item".to_owned() } else { format!("{} items", crate::fmt::thousands(n as u64)) };
        Some(format!("{verb} {what} to {}", over.label))
    }

    /// The job dropping here starts.
    pub fn request(&self) -> Option<JobRequest> {
        let over = self.over.as_ref()?;
        let items = self.items.clone();
        let into = over.into.clone();
        Some(match (&over.target, over.copy) {
            (DragTarget::Host(_), _) => JobRequest::CrossHostCopy { from: self.host.clone(), items, to: over.to.clone(), into },
            (_, true) => JobRequest::RemoteCopy { host: self.host.clone(), items, into },
            (_, false) => JobRequest::Move { host: self.host.clone(), items, into },
        })
    }
}
