//! What the quick preview asks of the service (`Command::Preview`) and what comes back
//! (`ServiceEvent::Preview`): one range of one file, read into memory only.

use dromon_core::{Error, HostId, RemotePath};

/// Identifies one preview read, chosen by the UI (unique per session: each window numbers its
/// own, as it does its [`SizeId`](crate::state::SizeId)s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PreviewId(pub u64);

/// Which bytes of a file a preview reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewRange {
    /// From the start: at most this many bytes.
    Head(u64),
    /// The end: the last this many bytes (the whole file when it's shorter).
    Tail(u64),
}

impl PreviewRange {
    /// Where the read starts and how many bytes it takes, in a file of `size` bytes.
    pub fn span(self, size: u64) -> (u64, u64) {
        match self {
            PreviewRange::Head(n) => (0, n.min(size)),
            PreviewRange::Tail(n) => {
                let len = n.min(size);
                (size - len, len)
            }
        }
    }
}

/// What a preview read: a file's contents, which may hold secrets (a config file's), so its
/// `Debug` says only how many bytes there are. Shared, never copied on the way to the view.
#[derive(Clone, PartialEq, Eq)]
pub struct PreviewBytes(pub std::sync::Arc<[u8]>);

impl std::fmt::Debug for PreviewBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PreviewBytes(<{} bytes>)", self.0.len())
    }
}

/// What the UI asks of the service about previews (`Command::Preview`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewCommand {
    /// Read `range` of the file `path` on the host's session, in a task of its own, into memory
    /// only. Answered to the sending window only with [`PreviewEvent::Done`], or
    /// [`PreviewEvent::Failed`]; a host that isn't connected answers `Failed` with
    /// [`Error::ConnectionLost`] at once.
    Start { id: PreviewId, host: HostId, path: RemotePath, range: PreviewRange },
    /// Stop the read `id` (the preview moved on, or closed). Nothing answers; a read that already
    /// ended, or an unknown id, is ignored.
    Cancel { id: PreviewId, host: HostId },
}

/// What the service tells the UI about a preview read (`ServiceEvent::Preview`).
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewEvent {
    /// The read `id`: the asked range of a file of `total` bytes (shorter only when the file
    /// shrank while it was read).
    Done { id: PreviewId, host: HostId, bytes: PreviewBytes, total: u64 },
    /// The read `id` couldn't be done: no access, the file is gone, the connection was lost.
    Failed { id: PreviewId, host: HostId, error: Error },
}

/// A `Preview` event's trace line (`DROMON_TRACE=1`): its host, id and sizes, never what it read.
pub fn trace_line(ev: &PreviewEvent) -> String {
    match ev {
        PreviewEvent::Done { id, host, bytes, total } => {
            format!("Preview host={host} id={} bytes={} total={total}", id.0, bytes.0.len())
        }
        PreviewEvent::Failed { id, host, .. } => format!("PreviewFailed host={host} id={}", id.0),
    }
}
