//! What editing in a local editor asks of the service (`Command::RemoteEdit`) and what comes
//! back (`ServiceEvent::RemoteEdit`). Events carry ids, paths, sizes and dates, never what a file
//! holds; the service sends every one of them to every main window, so each window's list of
//! files being edited is the same.

use std::path::PathBuf;

use dromon_core::{Error, HostId, RemotePath};
use dromon_transfer::Side;

/// Identifies one editing session, chosen by the window that starts it (unique per session:
/// each window numbers its own, as it does its [`PreviewId`](crate::state::PreviewId)s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EditId(pub u64);

/// The conflict sheet's answers: the server's copy changed since the edit downloaded it (or last
/// uploaded it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditChoice {
    /// Upload the local copy over the server's.
    Replace,
    /// Upload the local copy under the next free name beside the server's ("notes (1).txt");
    /// the edit goes on against the original path, whose newer copy stays as it is.
    KeepBoth,
    /// Download the server's copy again over the local one.
    Discard,
}

/// Which part of an edit failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditStage {
    /// The first download: there's no local copy, and the edit ends.
    Download,
    /// Uploading a save: the local copy stays, and Retry uploads it again.
    Upload,
    /// Downloading the server's copy again (Discard my changes): the conflict still stands.
    Reload,
}

impl EditStage {
    fn name(self) -> &'static str {
        match self {
            EditStage::Download => "Download",
            EditStage::Upload => "Upload",
            EditStage::Reload => "Reload",
        }
    }
}

/// What the UI asks of the service about files being edited (`Command::RemoteEdit`). A host
/// that isn't connected answers a `Start` with [`RemoteEditEvent::Failed`]
/// ([`Error::ConnectionLost`]) at once, and anything else with [`RemoteEditEvent::Closed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteEditCommand {
    /// Download the file `path` into a private folder of its own and watch the copy: answered
    /// with [`RemoteEditEvent::Opened`], or `Failed` at [`EditStage::Download`]. From then on each
    /// save of the copy is checked against the server's copy and uploaded
    /// ([`RemoteEditEvent::Uploading`], then `Saved`, `Conflict` or `Failed`).
    Start { id: EditId, host: HostId, path: RemotePath },
    /// Stop watching and remove the local copy: answered with [`RemoteEditEvent::Closed`].
    Stop { id: EditId, host: HostId },
    /// The conflict sheet's answer: answered with `Saved` (Replace, Keep both) or `Reloaded`
    /// (Discard), or `Failed`. Ignored while there's no conflict.
    Resolve { id: EditId, host: HostId, choice: EditChoice },
    /// Upload the copy again after a failed upload, checking the server's copy first, as a save
    /// does. Ignored unless the last upload failed.
    Retry { id: EditId, host: HostId },
}

/// What the service tells the UI about files being edited (`ServiceEvent::RemoteEdit`).
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteEditEvent {
    /// The edit `id` of `path` has its local copy at `local`, watched for saves: the window that
    /// started it opens it in the user's app.
    Opened { id: EditId, host: HostId, path: RemotePath, local: PathBuf },
    /// A save of the local copy was seen: it's being checked and uploaded.
    Uploading { id: EditId, host: HostId },
    /// The save is on the server at `to`: the edited path, or Keep both's new name.
    Saved { id: EditId, host: HostId, to: RemotePath },
    /// The server's copy changed since the edit last downloaded or uploaded it: `server` is it
    /// now, `local` the copy that would replace it. Nothing is uploaded until it's resolved.
    Conflict { id: EditId, host: HostId, server: Side, local: Side },
    /// Discard my changes: the server's copy is in place of the local one.
    Reloaded { id: EditId, host: HostId },
    /// What failed at `stage`, and why.
    Failed { id: EditId, host: HostId, stage: EditStage, error: Error },
    /// The edit ended (stopped, or its host disconnected): its local copy is gone.
    Closed { id: EditId, host: HostId },
}

/// A `RemoteEdit` event's trace line (`DROMON_TRACE=1`): its kind, host and id (and a failure's
/// stage), never a path, a name or an error message.
pub fn trace_line(ev: &RemoteEditEvent) -> String {
    match ev {
        RemoteEditEvent::Opened { id, host, .. } => format!("RemoteEditOpened host={host} id={}", id.0),
        RemoteEditEvent::Uploading { id, host } => format!("RemoteEditUploading host={host} id={}", id.0),
        RemoteEditEvent::Saved { id, host, .. } => format!("RemoteEditSaved host={host} id={}", id.0),
        RemoteEditEvent::Conflict { id, host, .. } => format!("RemoteEditConflict host={host} id={}", id.0),
        RemoteEditEvent::Reloaded { id, host } => format!("RemoteEditReloaded host={host} id={}", id.0),
        RemoteEditEvent::Failed { id, host, stage, .. } => {
            format!("RemoteEditFailed host={host} id={} stage={}", id.0, stage.name())
        }
        RemoteEditEvent::Closed { id, host } => format!("RemoteEditClosed host={host} id={}", id.0),
    }
}
