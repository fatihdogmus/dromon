//! Search in subfolders over the fixture's trees (`Command::Search`), at once: the service's
//! own visit ([`Matches`]) on the trees' walk, so names, folders, hidden files, unreadable
//! folders and the cap come out as they do for real. Everything found arrives in one batch,
//! then the end. There is nothing to cancel: a search is over before anything could.

use dromon_core::{Error, HostId, RemotePath};

use super::tree::Tree;
use crate::service::search::Matches;
use crate::state::{Found, SearchId, ServiceEvent};

/// The events that answer a search of `root` on `host` (`up`: its session is up; not up, it
/// fails with `ConnectionLost` at once, as the service's does).
pub(super) fn run(
    tree: &Tree,
    up: bool,
    id: SearchId,
    host: &HostId,
    root: &RemotePath,
    query: &str,
    show_hidden: bool,
) -> Vec<ServiceEvent> {
    if !up {
        return vec![ServiceEvent::SearchFailed { id, host: host.clone(), error: Error::ConnectionLost }];
    }
    let mut batches = Vec::new();
    let mut matches = Matches::new(root, query, show_hidden, |entries: Vec<Found>, folders| {
        batches.push(ServiceEvent::SearchFound { id, host: host.clone(), entries, folders });
    });
    let walked = tree.walk(host, root, &mut matches);
    let end = matches.end(id, host.clone(), walked);
    batches.push(end);
    batches
}
