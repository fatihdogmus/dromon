//! Folder sizes for the Properties sheet (`Command::FolderSize`): a walk of one folder on the
//! host's SFTP session, in a task of its own so listings, prompts and operations keep flowing.
//!
//! The walk lists every folder below the one asked about, a few at once ([`LISTS_IN_FLIGHT`]),
//! and adds up what it finds ([`SizeTotal`]: files, symlinks and other non-folders by their own
//! size, Finder-style; symlinks are counted, never followed). A folder it can't read (no access,
//! or gone meanwhile) is counted as unreadable and the walk goes on; any other error (the
//! connection was lost) stops it with what it had counted. Progress goes to the window that
//! asked at most every `fileops::PROGRESS_EVERY` (as a delete's), and so does the end.
//!
//! Cancelling aborts the walk's task, which drops its listings in flight. Nothing changes on the
//! server and nothing is listed again. A lost connection found by a walk is left for the
//! session's own listings and checks to notice: the walk only says so.

use std::collections::HashMap;
use std::time::Instant;

use dromon_core::{Entry, EntryKind, Error, HostId, RemotePath};
use dromon_sftp::Sftp;
use tokio::task::{JoinHandle, JoinSet};

use super::Sink;
use super::fileops::Throttle;
use crate::state::{ServiceEvent, SizeId, SizeTotal, WindowId};

/// Folder listings a walk keeps in flight at once.
pub const LISTS_IN_FLIGHT: usize = 8;

/// What the manager asks of a host's walks.
pub enum SizeCmd {
    Start { id: SizeId, path: RemotePath },
    Cancel(SizeId),
}

/// The walks running on one connection, by id.
#[derive(Default)]
pub struct Walks {
    running: HashMap<SizeId, JoinHandle<()>>,
}

impl Drop for Walks {
    fn drop(&mut self) {
        for (_, walk) in self.running.drain() {
            walk.abort();
        }
    }
}

impl Walks {
    /// Carries out `cmd` from `window`: a walk on `sftp` (none: the session is down, and it
    /// fails at once), or cancelling one.
    pub fn command<S: Sink>(&mut self, window: WindowId, cmd: SizeCmd, sftp: Option<Sftp>, host: &HostId, sink: &S) {
        self.running.retain(|_, walk| !walk.is_finished());
        match cmd {
            SizeCmd::Start { id, path } => {
                let Some(sftp) = sftp else {
                    let ev = ServiceEvent::FolderSizeFailed {
                        id,
                        host: host.clone(),
                        total: SizeTotal::default(),
                        error: Error::ConnectionLost,
                    };
                    return sink.emit_to(window, ev);
                };
                let (host, sink) = (host.clone(), sink.clone());
                let walk = tokio::spawn(async move {
                    let progress = |total| {
                        sink.emit_to(window, ServiceEvent::FolderSizeProgress { id, host: host.clone(), total })
                    };
                    let ev = match walk(&sftp, &path, progress).await {
                        Ok(total) => ServiceEvent::FolderSizeDone { id, host, total },
                        Err((total, error)) => ServiceEvent::FolderSizeFailed { id, host, total, error },
                    };
                    sink.emit_to(window, ev);
                });
                if let Some(old) = self.running.insert(id, walk) {
                    old.abort();
                }
            }
            SizeCmd::Cancel(id) => {
                if let Some(walk) = self.running.remove(&id) {
                    walk.abort();
                }
            }
        }
    }
}

/// Adds the entries of `dir` to `total`, and its subfolders to `todo`.
pub fn count(total: &mut SizeTotal, dir: &RemotePath, entries: Vec<Entry>, todo: &mut Vec<RemotePath>) {
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

/// Walks `root` and everything below it, calling `progress` at most every
/// `fileops::PROGRESS_EVERY`.
/// The error comes with what had been counted when it stopped.
pub async fn walk(
    sftp: &Sftp,
    root: &RemotePath,
    mut progress: impl FnMut(SizeTotal),
) -> Result<SizeTotal, (SizeTotal, Error)> {
    let mut total = SizeTotal::default();
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
        let (dir, listed) = joined.map_err(|e| (total, Error::Other(format!("A folder size stopped: {e}"))))?;
        match listed {
            Ok(entries) => count(&mut total, &dir, entries, &mut todo),
            Err(e) if skippable(&e) => total.unreadable += 1,
            Err(e) => return Err((total, e)),
        }
        if throttle.due(Instant::now()) {
            progress(total);
        }
    }
    Ok(total)
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
    fn only_unreadable_folders_let_the_walk_go_on() {
        assert!(skippable(&Error::PermissionDenied(RemotePath::new("/x"))));
        assert!(skippable(&Error::NotFound(RemotePath::new("/x"))));
        assert!(!skippable(&Error::ConnectionLost));
        assert!(!skippable(&Error::Io("short read".into())));
    }
}
