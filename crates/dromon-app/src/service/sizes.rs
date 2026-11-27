//! Folder walks on the host's SFTP session, each in a task of its own so listings, prompts and
//! operations keep flowing: the Properties sheet's folder sizes (`Command::FolderSize`, here)
//! and search in subfolders (`Command::Search`, `search.rs`). One walk serves both; what it
//! does with each folder it lists is a [`Visit`]'s.
//!
//! The walk lists every folder below the one asked about, a few at once ([`LISTS_IN_FLIGHT`]),
//! never following symlinks (a visit queues only real folders). A folder it can't read (no
//! access, or gone meanwhile) is noted as unreadable and the walk goes on; any other error (the
//! connection was lost) stops it with what it had found. The visit reports progress at most
//! every `fileops::PROGRESS_EVERY` (as a delete's), to the window that asked, as is the end.
//!
//! A folder size adds up what it finds ([`SizeTotal`]: files, symlinks and other non-folders
//! by their own size, Finder-style; symlinks are counted, never followed).
//!
//! Cancelling aborts the walk's task, which drops its listings in flight. Nothing changes on the
//! server and nothing is listed again. A lost connection found by a walk is left for the
//! session's own listings and checks to notice: the walk only says so.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::ops::ControlFlow;
use std::time::Instant;

use dromon_core::{Entry, EntryKind, Error, HostId, RemotePath};
use dromon_sftp::Sftp;
use tokio::task::{JoinHandle, JoinSet};

use super::Sink;
use super::conn::{ConnCmd, ConnHandle};
use super::fileops::Throttle;
use crate::state::{ServiceEvent, SizeCommand, SizeEvent, SizeId, SizeTotal, WindowId};

/// Folder listings a walk keeps in flight at once.
pub const LISTS_IN_FLIGHT: usize = 8;

/// What a walk does with what it finds.
pub trait Visit {
    /// `dir` listed `entries`: the folders to walk on go into `todo`. `Break` ends the walk
    /// there, as if it had gone through everything.
    fn listed(&mut self, dir: &RemotePath, entries: Vec<Entry>, todo: &mut Vec<RemotePath>) -> ControlFlow<()>;
    /// A folder couldn't be read (no access, or gone meanwhile).
    fn unreadable(&mut self);
    /// How far it got, at most every `fileops::PROGRESS_EVERY`.
    fn progress(&mut self);
}

/// What the manager asks of a host's folder size walks.
pub enum SizeCmd {
    Start { id: SizeId, path: RemotePath },
    Cancel(SizeId),
}

/// A folder size command from window `from`: the host's task walks the folder, or stops the
/// walk; a host without one isn't connected, and the walk fails at once.
pub(super) fn route<S: Sink>(from: WindowId, cmd: SizeCommand, conns: &HashMap<HostId, ConnHandle>, sink: &S) {
    let send =
        |host: &HostId, cmd: SizeCmd| conns.get(host).is_some_and(|c| c.cmds.send(ConnCmd::Size(from, cmd)).is_ok());
    match cmd {
        SizeCommand::Start { id, host, path } => {
            if !send(&host, SizeCmd::Start { id, path }) {
                let ev = SizeEvent::Failed { id, host, total: SizeTotal::default(), error: Error::ConnectionLost };
                sink.emit_to(from, ServiceEvent::FolderSize(ev));
            }
        }
        SizeCommand::Cancel { id, host } => {
            send(&host, SizeCmd::Cancel(id));
        }
    }
}

/// The walks of one kind running on one connection, by id.
pub struct Walks<Id> {
    running: HashMap<Id, JoinHandle<()>>,
}

impl<Id> Default for Walks<Id> {
    fn default() -> Self {
        Walks { running: HashMap::new() }
    }
}

impl<Id> Drop for Walks<Id> {
    fn drop(&mut self) {
        for (_, walk) in self.running.drain() {
            walk.abort();
        }
    }
}

impl<Id: Eq + Hash> Walks<Id> {
    /// Runs `walk` as `id` (one already running as `id` stops).
    pub fn start(&mut self, id: Id, walk: impl Future<Output = ()> + Send + 'static) {
        self.running.retain(|_, walk| !walk.is_finished());
        if let Some(old) = self.running.insert(id, tokio::spawn(walk)) {
            old.abort();
        }
    }

    /// Stops the walk `id`, if it runs.
    pub fn cancel(&mut self, id: &Id) {
        self.running.retain(|_, walk| !walk.is_finished());
        if let Some(walk) = self.running.remove(id) {
            walk.abort();
        }
    }
}

impl Walks<SizeId> {
    /// Carries out `cmd` from `window`: a walk on `sftp` (none: the session is down, and it
    /// fails at once), or cancelling one.
    pub fn command<S: Sink>(&mut self, window: WindowId, cmd: SizeCmd, sftp: Option<Sftp>, host: &HostId, sink: &S) {
        match cmd {
            SizeCmd::Start { id, path } => {
                let Some(sftp) = sftp else {
                    let ev = SizeEvent::Failed {
                        id,
                        host: host.clone(),
                        total: SizeTotal::default(),
                        error: Error::ConnectionLost,
                    };
                    return sink.emit_to(window, ServiceEvent::FolderSize(ev));
                };
                let (host, sink) = (host.clone(), sink.clone());
                self.start(id, async move {
                    let mut sizes = Sizes::new(|total| {
                        let ev = SizeEvent::Progress { id, host: host.clone(), total };
                        sink.emit_to(window, ServiceEvent::FolderSize(ev))
                    });
                    let ended = walk(&sftp, &path, &mut sizes).await;
                    let total = sizes.total;
                    let ev = match ended {
                        Ok(()) => SizeEvent::Done { id, host, total },
                        Err(error) => SizeEvent::Failed { id, host, total, error },
                    };
                    sink.emit_to(window, ServiceEvent::FolderSize(ev));
                });
            }
            SizeCmd::Cancel(id) => self.cancel(&id),
        }
    }
}

/// A folder size: the [`SizeTotal`] so far, and where its progress goes.
pub struct Sizes<F> {
    pub total: SizeTotal,
    progress: F,
}

impl<F: FnMut(SizeTotal)> Sizes<F> {
    pub fn new(progress: F) -> Self {
        Sizes { total: SizeTotal::default(), progress }
    }
}

impl<F: FnMut(SizeTotal)> Visit for Sizes<F> {
    fn listed(&mut self, dir: &RemotePath, entries: Vec<Entry>, todo: &mut Vec<RemotePath>) -> ControlFlow<()> {
        count(&mut self.total, dir, entries, todo);
        ControlFlow::Continue(())
    }

    fn unreadable(&mut self) {
        self.total.unreadable += 1;
    }

    fn progress(&mut self) {
        (self.progress)(self.total);
    }
}

/// Adds the entries of `dir` to `total`, and its subfolders to `todo`.
fn count(total: &mut SizeTotal, dir: &RemotePath, entries: Vec<Entry>, todo: &mut Vec<RemotePath>) {
    for e in entries {
        match e.kind {
            EntryKind::Dir => {
                total.folders += 1;
                todo.push(dir.join(&e.name));
            }
            EntryKind::File | EntryKind::Symlink { .. } | EntryKind::Other => {
                total.files += 1;
                total.bytes += e.size;
            }
        }
    }
}

/// Whether a folder's listing error leaves the walk going (the folder is counted unreadable).
pub fn skippable(e: &Error) -> bool {
    matches!(e, Error::PermissionDenied(_) | Error::NotFound(_))
}

/// Walks `root` and everything below it that `visit` queues, telling it of each folder.
pub async fn walk(sftp: &Sftp, root: &RemotePath, visit: &mut impl Visit) -> Result<(), Error> {
    let mut todo = vec![root.clone()];
    let mut lists = JoinSet::new();
    let mut throttle = Throttle::default();
    loop {
        while lists.len() < LISTS_IN_FLIGHT
            && let Some(dir) = todo.pop()
        {
            let sftp = sftp.clone();
            lists.spawn(async move {
                let listed = sftp.list(&dir).await;
                (dir, listed)
            });
        }
        let Some(joined) = lists.join_next().await else { break };
        let (dir, listed) = joined.map_err(|e| Error::Other(format!("A folder walk stopped: {e}")))?;
        match listed {
            Ok(entries) => {
                if visit.listed(&dir, entries, &mut todo).is_break() {
                    break;
                }
            }
            Err(e) if skippable(&e) => visit.unreadable(),
            Err(e) => return Err(e),
        }
        if throttle.due(Instant::now()) {
            visit.progress();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, kind: EntryKind, size: u64) -> Entry {
        Entry { name: name.into(), kind, size, mtime: None, mode: None, owner: None, group: None }
    }

    #[test]
    fn files_links_and_others_count_their_bytes_folders_go_on_the_list() {
        let (mut total, mut todo) = (SizeTotal::default(), Vec::new());
        let entries = vec![
            e("a.txt", EntryKind::File, 10),
            e("sub", EntryKind::Dir, 4096),
            e("link", EntryKind::Symlink { target: Some(dromon_core::LinkTarget::Dir) }, 7),
            e("sock", EntryKind::Other, 0),
        ];
        count(&mut total, &RemotePath::new("/w"), entries, &mut todo);
        assert_eq!(total, SizeTotal { bytes: 17, files: 3, folders: 1, unreadable: 0 });
        assert_eq!(todo, [RemotePath::new("/w/sub")], "a link to a folder is never followed");
        assert_eq!(total.items(), 4);
    }

    #[test]
    fn a_folder_size_visit_counts_unreadable_folders_and_reports_its_total() {
        let mut seen = Vec::new();
        let mut sizes = Sizes::new(|t| seen.push(t));
        let mut todo = Vec::new();
        let flow = sizes.listed(&RemotePath::new("/w"), vec![e("a", EntryKind::File, 5)], &mut todo);
        assert_eq!(flow, ControlFlow::Continue(()), "a folder size never stops early");
        sizes.unreadable();
        sizes.progress();
        let total = sizes.total;
        assert_eq!(total, SizeTotal { bytes: 5, files: 1, folders: 0, unreadable: 1 });
        assert_eq!(seen, [total]);
    }

    #[test]
    fn only_unreadable_folders_let_the_walk_go_on() {
        assert!(skippable(&Error::PermissionDenied(RemotePath::new("/x"))));
        assert!(skippable(&Error::NotFound(RemotePath::new("/x"))));
        assert!(!skippable(&Error::ConnectionLost));
        assert!(!skippable(&Error::Io("short read".into())));
    }
}
