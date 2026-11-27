//! Search in subfolders over the fixture's trees (`Command::Search`), at once: the service's
//! own visit ([`Matches`]) on the trees' walk, so names, folders, hidden files, unreadable
//! folders and the cap come out as they do for real. Everything found arrives in one batch,
//! then the end. There is nothing to cancel: a search is over before anything could.

use dromon_core::{Error, HostId, RemotePath};

use super::backend::Core;
use super::tree::Tree;
use crate::service::search::Matches;
use crate::state::{Found, SearchCommand, SearchEvent, SearchId, ServiceEvent};

impl Core {
    /// A search command from window `Core::from`.
    pub(super) fn search(&mut self, cmd: SearchCommand, out: &mut Vec<ServiceEvent>) {
        match cmd {
            SearchCommand::Start { id, host, root, query, show_hidden } => {
                let up = self.up(&host);
                out.extend(run(&self.transfers.tree, up, id, &host, &root, &query, show_hidden));
                // A folder whose listing loses the connection drops it, as a folder size's does.
                let lost = matches!(
                    out.last(),
                    Some(ServiceEvent::Search(SearchEvent::Failed { error: Error::ConnectionLost, .. }))
                );
                if up && lost {
                    self.walk_lost(&host, out);
                }
            }
            SearchCommand::Cancel { .. } => {}
        }
    }
}

/// The events that answer a search of `root` on `host` (`up`: its session is up; not up, it
/// fails with `ConnectionLost` at once, as the service's does).
fn run(
    tree: &Tree,
    up: bool,
    id: SearchId,
    host: &HostId,
    root: &RemotePath,
    query: &str,
    show_hidden: bool,
) -> Vec<ServiceEvent> {
    if !up {
        let ev = SearchEvent::Failed { id, host: host.clone(), error: Error::ConnectionLost };
        return vec![ServiceEvent::Search(ev)];
    }
    let mut batches = Vec::new();
    let mut matches = Matches::new(root, query, show_hidden, |entries: Vec<Found>, folders| {
        batches.push(ServiceEvent::Search(SearchEvent::Found { id, host: host.clone(), entries, folders }));
    });
    let walked = tree.walk(host, root, &mut matches);
    let end = matches.end(id, host.clone(), walked);
    batches.push(end);
    batches
}
