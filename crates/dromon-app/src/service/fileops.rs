//! A connection's file operations (§5 M3): new folder, rename and delete, each run on the
//! host's SFTP session in a task of its own so listings and prompts keep flowing.
//!
//! New folders and renames start at once; deletes run one at a time per host, the others
//! waiting in order. A delete reports progress at most every [`PROGRESS_EVERY`]. Whatever
//! ends an operation comes back to the connection task through [`FileOps::next`]: it says
//! Done or Failed and lists the folder on screen again when that changed (`conn.rs`).
//!
//! A session that goes away fails the deletes still waiting with [`Error::ConnectionLost`];
//! the running ones fail by themselves as their requests do (closing the session fails them).
//! Nothing is resumed.

use std::collections::VecDeque;
use std::future::pending;
use std::time::{Duration, Instant};

use dromon_core::{Error, HostId};
use dromon_sftp::Sftp;
use tokio::sync::mpsc;

use super::Sink;
use crate::state::{FileOp, FileOpEvent, OpId, ServiceEvent};

/// The least time between two progress events of one delete.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// An operation that ended.
pub struct Finished {
    pub id: OpId,
    pub op: FileOp,
    /// The session it ran on (see `Task::epoch` in `conn.rs`).
    pub epoch: u64,
    pub result: Result<(), Error>,
}

/// The file operations of one connection.
pub struct FileOps {
    tx: mpsc::UnboundedSender<Finished>,
    rx: mpsc::UnboundedReceiver<Finished>,
    /// Deletes waiting for the running one.
    queued: VecDeque<(OpId, FileOp)>,
    /// A delete is running.
    deleting: bool,
}

impl Default for FileOps {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        FileOps { tx, rx, queued: VecDeque::new(), deleting: false }
    }
}

/// Lets at most one event through per [`PROGRESS_EVERY`]; the first one at once.
#[derive(Debug, Default)]
pub struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    pub fn due(&mut self, now: Instant) -> bool {
        if self.last.is_some_and(|last| now.saturating_duration_since(last) < PROGRESS_EVERY) {
            return false;
        }
        self.last = Some(now);
        true
    }
}

impl FileOps {
    /// The next operation that ended.
    pub async fn next(&mut self) -> Finished {
        match self.rx.recv().await {
            Some(done) => done,
            // Never: `self` keeps a sender.
            None => pending().await,
        }
    }

    /// Starts `op` on `sftp` (session `epoch`), or queues a delete behind the running one.
    pub fn start<S: Sink>(&mut self, id: OpId, op: FileOp, sftp: &Sftp, epoch: u64, host: &HostId, sink: &S) {
        if matches!(op, FileOp::Delete { .. }) {
            if self.deleting {
                self.queued.push_back((id, op));
                return;
            }
            self.deleting = true;
        }
        self.spawn(id, op, sftp.clone(), epoch, host.clone(), sink.clone());
    }

    /// A delete ended: the next one waiting starts on `sftp`, or, without a session, every
    /// one waiting fails.
    pub fn delete_ended<S: Sink>(&mut self, sftp: Option<&Sftp>, epoch: u64, host: &HostId, sink: &S) {
        self.deleting = false;
        match sftp {
            Some(sftp) => {
                if let Some((id, op)) = self.queued.pop_front() {
                    self.start(id, op, sftp, epoch, host, sink);
                }
            }
            None => self.fail_queued(host, sink),
        }
    }

    /// The session went away: the deletes waiting fail.
    pub fn fail_queued<S: Sink>(&mut self, host: &HostId, sink: &S) {
        for (id, _) in self.queued.drain(..) {
            sink.emit(ServiceEvent::FileOp(FileOpEvent::Failed {
                id,
                host: host.clone(),
                error: Error::ConnectionLost,
            }));
        }
    }

    fn spawn<S: Sink>(&self, id: OpId, op: FileOp, sftp: Sftp, epoch: u64, host: HostId, sink: S) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = run(id, &op, &sftp, &host, &sink).await;
            let _ = tx.send(Finished { id, op, epoch, result });
        });
    }
}

/// Carries out one operation; a delete sends its progress to `sink`.
async fn run<S: Sink>(id: OpId, op: &FileOp, sftp: &Sftp, host: &HostId, sink: &S) -> Result<(), Error> {
    match op {
        FileOp::Mkdir { path } => sftp.mkdir(path).await,
        FileOp::Rename { from, to } => sftp.rename(from, to).await,
        FileOp::Delete { paths } => {
            let mut throttle = Throttle::default();
            let progress = |done, total| {
                if throttle.due(Instant::now()) {
                    sink.emit(ServiceEvent::FileOp(FileOpEvent::Progress { id, host: host.clone(), done, total }));
                }
            };
            sftp.remove_all_of(paths, progress).await.map(drop)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_let_through_at_most_every_100_ms() {
        let mut t = Throttle::default();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        assert!(t.due(at(0)), "the first one at once");
        assert!(!t.due(at(1)) && !t.due(at(99)));
        assert!(t.due(at(100)));
        assert!(!t.due(at(150)));
        assert!(t.due(at(260)));
        // A clock that went backwards doesn't open it.
        assert!(!t.due(start));
    }
}
