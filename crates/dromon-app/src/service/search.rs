//! Search in subfolders (`Command::Search`): the folder walk of `sizes.rs`, matching names
//! instead of adding sizes. What it finds streams to the window that asked in batches, at most
//! every `fileops::PROGRESS_EVERY`; it ends at `state::search::CAP` results. With hidden files
//! off, a hidden entry is neither matched nor walked into, as the list shows it. The fixture
//! backend searches its trees with the same [`Matches`].

use std::ops::ControlFlow;

use dromon_core::{Entry, EntryKind, Error, HostId, RemotePath};
use dromon_sftp::Sftp;

use super::Sink;
use super::sizes::{Visit, Walks, walk};
use crate::state::search::{CAP, Matcher};
use crate::state::{Found, SearchId, ServiceEvent, WindowId};

/// What the manager asks of a host's searches.
pub enum SearchCmd {
    Start { id: SearchId, root: RemotePath, query: String, show_hidden: bool },
    Cancel(SearchId),
}

/// A search's visit: what it found since the last batch, and how far it got.
pub struct Matches<F> {
    matcher: Matcher,
    root: RemotePath,
    show_hidden: bool,
    batch: Vec<Found>,
    found: usize,
    /// Folders listed so far.
    pub folders: u64,
    pub unreadable: u64,
    /// It stopped at the cap.
    pub capped: bool,
    /// Takes each batch and the folders searched so far.
    emit: F,
}

impl<F: FnMut(Vec<Found>, u64)> Matches<F> {
    pub fn new(root: &RemotePath, query: &str, show_hidden: bool, emit: F) -> Self {
        Matches {
            matcher: Matcher::new(query),
            root: root.clone(),
            show_hidden,
            batch: Vec::new(),
            found: 0,
            folders: 0,
            unreadable: 0,
            capped: false,
            emit,
        }
    }

    /// Hands on what's waiting (also when nothing is: the count of folders moved on).
    pub fn flush(&mut self) {
        (self.emit)(std::mem::take(&mut self.batch), self.folders);
    }

    /// The event that ends the search `id`: its last batch has gone out.
    pub fn end(mut self, id: SearchId, host: HostId, walked: Result<(), Error>) -> ServiceEvent {
        self.flush();
        match walked {
            Ok(()) => ServiceEvent::SearchDone {
                id,
                host,
                folders: self.folders,
                unreadable: self.unreadable,
                capped: self.capped,
            },
            Err(error) => ServiceEvent::SearchFailed { id, host, error },
        }
    }
}

/// `dir` under `root`, as a result's folder: `assets/img`, `.` for `root` itself.
fn relative(root: &RemotePath, dir: &RemotePath) -> String {
    let rel = dir.as_str().get(root.as_str().len()..).unwrap_or("").trim_start_matches('/');
    if rel.is_empty() { ".".into() } else { rel.to_owned() }
}

impl<F: FnMut(Vec<Found>, u64)> Visit for Matches<F> {
    fn listed(&mut self, dir: &RemotePath, entries: Vec<Entry>, todo: &mut Vec<RemotePath>) -> ControlFlow<()> {
        self.folders += 1;
        let folder = relative(&self.root, dir);
        for e in entries {
            if !self.show_hidden && e.hidden() {
                continue;
            }
            if e.kind == EntryKind::Dir {
                todo.push(dir.join(&e.name));
            }
            if self.matcher.matches(&e.name) {
                self.batch.push(Found { folder: folder.clone(), entry: e });
                self.found += 1;
                if self.found >= CAP {
                    self.capped = true;
                    return ControlFlow::Break(());
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn unreadable(&mut self) {
        self.unreadable += 1;
    }

    fn progress(&mut self) {
        self.flush();
    }
}

impl Walks<SearchId> {
    /// Carries out `cmd` from `window`: a search on `sftp` (none: the session is down, and it
    /// fails at once), or cancelling one.
    pub fn command<S: Sink>(&mut self, window: WindowId, cmd: SearchCmd, sftp: Option<Sftp>, host: &HostId, sink: &S) {
        match cmd {
            SearchCmd::Start { id, root, query, show_hidden } => {
                let Some(sftp) = sftp else {
                    let ev = ServiceEvent::SearchFailed { id, host: host.clone(), error: Error::ConnectionLost };
                    return sink.emit_to(window, ev);
                };
                let (host, sink) = (host.clone(), sink.clone());
                self.start(id, async move {
                    let batches = sink.clone();
                    let to = host.clone();
                    let mut matches = Matches::new(&root, &query, show_hidden, move |entries, folders| {
                        let ev = ServiceEvent::SearchFound { id, host: to.clone(), entries, folders };
                        batches.emit_to(window, ev);
                    });
                    let walked = walk(&sftp, &root, &mut matches).await;
                    sink.emit_to(window, matches.end(id, host, walked));
                });
            }
            SearchCmd::Cancel(id) => self.cancel(&id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, kind: EntryKind) -> Entry {
        Entry { name: name.into(), kind, size: 1, mtime: None, mode: None, owner: None, group: None }
    }

    #[test]
    fn results_name_their_folder_below_the_root() {
        let root = RemotePath::new("/srv/www");
        assert_eq!(relative(&root, &root), ".");
        assert_eq!(relative(&root, &RemotePath::new("/srv/www/assets/img")), "assets/img");
        assert_eq!(relative(&RemotePath::root(), &RemotePath::new("/etc")), "etc");
    }

    #[test]
    fn a_search_matches_names_walks_real_folders_and_skips_hidden_ones_when_they_are_hidden() {
        let mut batches = Vec::new();
        let root = RemotePath::new("/w");
        let mut todo = Vec::new();
        let entries = || {
            vec![
                e("app.log", EntryKind::File),
                e("logs", EntryKind::Dir),
                e(".log-cache", EntryKind::Dir),
                e("link.log", EntryKind::Symlink { target: Some(dromon_core::LinkTarget::Dir) }),
                e("readme", EntryKind::File),
            ]
        };
        let mut m = Matches::new(&root, "LOG", false, |b: Vec<Found>, f| batches.push((b, f)));
        assert_eq!(m.listed(&root, entries(), &mut todo), ControlFlow::Continue(()));
        assert_eq!(todo, [RemotePath::new("/w/logs")], "no link and no hidden folder is walked into");
        m.flush();
        let mut all = Matches::new(&root, "*.log", true, |_, _| {});
        let mut every = Vec::new();
        assert_eq!(all.listed(&root, entries(), &mut every), ControlFlow::Continue(()));
        assert_eq!(every, [RemotePath::new("/w/logs"), RemotePath::new("/w/.log-cache")]);
        assert_eq!(all.found, 2, "the glob matches app.log and link.log whole");
        let names: Vec<&str> = batches[0].0.iter().map(|f| f.entry.name.as_str()).collect();
        assert_eq!(names, ["app.log", "logs", "link.log"]);
        assert_eq!(batches[0].1, 1);
        assert!(batches[0].0.iter().all(|f| f.folder == "."));
    }

    #[test]
    fn a_search_stops_at_the_cap() {
        let root = RemotePath::new("/w");
        let mut m = Matches::new(&root, "f", true, |_, _| {});
        let many: Vec<Entry> = (0..CAP + 10).map(|i| e(&format!("f{i}"), EntryKind::File)).collect();
        assert_eq!(m.listed(&root, many, &mut Vec::new()), ControlFlow::Break(()));
        assert!(m.capped);
        assert_eq!(m.batch.len(), CAP);
        let ServiceEvent::SearchDone { capped, folders, .. } = m.end(SearchId(1), HostId("h".into()), Ok(())) else {
            panic!("a search that went through ends Done")
        };
        assert!(capped && folders == 1);
    }
}
