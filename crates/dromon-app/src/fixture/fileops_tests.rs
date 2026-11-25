//! File operations on a fixture: new folder, rename and delete against the folder trees, with
//! the events the service sends and the errors a real server gives, and later listings
//! (and finished uploads) on top of what they changed.

use std::path::PathBuf;
use std::sync::Arc;

use dromon_core::{ConnStatus, Error, HostId, RemotePath};
use dromon_transfer::JobRequest;

use super::*;
use crate::state::{Backend, Command, FileOp, OpId, ServiceEvent, TransferCommand};

const HOME: &str = "/home/deploy";

fn prod() -> HostId {
    HostId("prod".into())
}

fn p(rel: &str) -> RemotePath {
    RemotePath::new(HOME).join(rel)
}

/// prod opens at /home/deploy: a file, a nested tree with a link to a folder outside it, an
/// unreadable folder, one whose listing loses the connection, and a 600-file folder.
fn fixture() -> Arc<Fixture> {
    let src = r#"{"version": 1, "now": "2026-11-25T14:00:00Z",
        "hosts": [{"alias": "prod", "source": "ssh-config"}, {"alias": "pi", "source": "saved"}],
        "servers": {"prod": {"home": "/home/deploy", "auth": [], "tree": {
            "/home/deploy": [{"name": "x.txt", "kind": "file", "size": 3}, {"name": "tree", "kind": "dir"},
                             {"name": "keep", "kind": "dir"}, {"name": "locked", "kind": "dir"},
                             {"name": "flaky", "kind": "dir"}, {"name": "many", "kind": "dir"}],
            "/home/deploy/tree": [{"name": "a.txt", "kind": "file"}, {"name": "sub", "kind": "dir"},
                                  {"name": "to-keep", "kind": "symlink", "target": "dir"}],
            "/home/deploy/tree/sub": [{"name": "b.txt", "kind": "file"}, {"name": "deeper", "kind": "dir"}],
            "/home/deploy/keep": [{"name": "k.txt", "kind": "file"}],
            "/home/deploy/locked": {"error": "permission-denied"},
            "/home/deploy/flaky": {"error": "connection-lost"},
            "/home/deploy/many": [{"generate": {"pattern": "f{03}", "count": 600}}]}}}}"#;
    Arc::new(parse(src).unwrap_or_else(|e| panic!("{e}")))
}

/// Connected to prod, with its home on screen.
fn connected() -> FixtureBackend {
    let mut b = FixtureBackend::new(fixture());
    b.send(Command::Connect { host: prod() });
    b.send(Command::List { host: prod(), path: p("") });
    b
}

fn op(b: &mut FixtureBackend, id: u64, op: FileOp) -> Vec<ServiceEvent> {
    b.send(Command::FileOp { id: OpId(id), host: prod(), op })
}

fn done(id: u64) -> ServiceEvent {
    ServiceEvent::FileOpDone { id: OpId(id), host: prod() }
}

fn failed(id: u64, error: Error) -> ServiceEvent {
    ServiceEvent::FileOpFailed { id: OpId(id), host: prod(), error }
}

/// The names of the listing of `path` in `evs` (pages joined), sorted.
fn listed(evs: &[ServiceEvent], path: &RemotePath) -> Option<Vec<String>> {
    let mut names: Option<Vec<String>> = None;
    for e in evs {
        if let ServiceEvent::ListingPage { path: at, entries, first, .. } = e
            && at == path
        {
            if *first {
                names = Some(Vec::new());
            }
            names.get_or_insert_default().extend(entries.iter().map(|e| e.name.clone()));
        }
    }
    names.map(|mut n| {
        n.sort();
        n
    })
}

/// `path` listed now: its names, or the error.
fn ls(b: &mut FixtureBackend, path: &RemotePath) -> Result<Vec<String>, Error> {
    let evs = b.send(Command::List { host: prod(), path: path.clone() });
    if let Some(ServiceEvent::ListFailed { error, .. }) = evs.first() {
        return Err(error.clone());
    }
    Ok(listed(&evs, path).expect("a listing"))
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn without_a_connection_an_operation_fails_at_once() {
    let mut b = FixtureBackend::new(fixture());
    assert_eq!(op(&mut b, 1, FileOp::Mkdir { path: p("new") }), [failed(1, Error::ConnectionLost)]);
    b.send(Command::Connect { host: prod() });
    b.send(Command::Disconnect { host: prod() });
    assert_eq!(op(&mut b, 2, FileOp::Delete { paths: vec![p("x.txt")] }), [failed(2, Error::ConnectionLost)]);
    b.send(Command::Connect { host: prod() });
    assert_eq!(ls(&mut b, &p("")).unwrap().len(), 6, "nothing was deleted");
}

#[test]
fn a_new_folder_lists_in_its_parent_and_is_empty() {
    let mut b = connected();
    let evs = op(&mut b, 1, FileOp::Mkdir { path: p("new") });
    assert_eq!(evs[0], done(1));
    assert_eq!(listed(&evs, &p("")), Some(names(&["flaky", "keep", "locked", "many", "new", "tree", "x.txt"])), "the folder on screen again");
    let entry = b
        .send(Command::List { host: prod(), path: p("") })
        .into_iter()
        .find_map(|e| match e {
            ServiceEvent::ListingPage { entries, .. } => entries.into_iter().find(|e| e.name == "new"),
            _ => None,
        })
        .expect("listed");
    assert_eq!((entry.kind, entry.size, entry.mtime), (EntryKind::Dir, 4096, Some(DEMO_NOW)));

    // Taken, a missing parent, an unreadable parent: what the server says; still listed again.
    let evs = op(&mut b, 2, FileOp::Mkdir { path: p("x.txt") });
    assert_eq!(evs[0], failed(2, Error::AlreadyExists(p("x.txt"))));
    assert!(listed(&evs, &p("")).is_some());
    assert_eq!(op(&mut b, 3, FileOp::Mkdir { path: p("nope/new") })[0], failed(3, Error::NotFound(p("nope/new"))));
    assert_eq!(op(&mut b, 4, FileOp::Mkdir { path: p("locked/new") })[0], failed(4, Error::PermissionDenied(p("locked/new"))));
    // Inside the new folder (not on screen: nothing listed).
    assert_eq!(op(&mut b, 5, FileOp::Mkdir { path: p("new/inner") }), [done(5)]);
    assert_eq!(ls(&mut b, &p("new")), Ok(names(&["inner"])));
    assert_eq!(ls(&mut b, &p("new/inner")), Ok(Vec::new()));
}

#[test]
fn a_rename_moves_the_entry_and_a_folder_takes_its_contents_along() {
    let mut b = connected();
    let evs = op(&mut b, 1, FileOp::Rename { from: p("x.txt"), to: p("y.txt") });
    assert_eq!(evs[0], done(1));
    assert_eq!(listed(&evs, &p("")), Some(names(&["flaky", "keep", "locked", "many", "tree", "y.txt"])));

    // Onto a name that's taken, from a name that isn't there, onto itself.
    let evs = op(&mut b, 2, FileOp::Rename { from: p("y.txt"), to: p("tree") });
    assert_eq!(evs[0], failed(2, Error::AlreadyExists(p("tree"))));
    assert_eq!(op(&mut b, 3, FileOp::Rename { from: p("x.txt"), to: p("z.txt") })[0], failed(3, Error::NotFound(p("x.txt"))));
    assert_eq!(op(&mut b, 4, FileOp::Rename { from: p("tree"), to: p("tree") })[0], failed(4, Error::AlreadyExists(p("tree"))));

    // A folder: everything inside it moves; the old path is gone with all below it.
    assert_eq!(op(&mut b, 5, FileOp::Rename { from: p("tree"), to: p("moved") })[0], done(5));
    assert_eq!(ls(&mut b, &p("moved")), Ok(names(&["a.txt", "sub", "to-keep"])));
    assert_eq!(ls(&mut b, &p("moved/sub")), Ok(names(&["b.txt", "deeper"])));
    assert_eq!(ls(&mut b, &p("moved/sub/deeper")), Ok(Vec::new()));
    assert_eq!(ls(&mut b, &p("tree")), Err(Error::NotFound(p("tree"))));
    assert_eq!(ls(&mut b, &p("tree/sub")), Err(Error::NotFound(p("tree/sub"))));
    // Not on screen (the last folder listed is tree/sub): nothing listed again.
    assert_eq!(op(&mut b, 6, FileOp::Rename { from: p("moved/sub"), to: p("moved/sub2") }), [done(6)]);
    // Renaming the folder on screen: its old path isn't listed again.
    ls(&mut b, &p("moved/sub2")).unwrap();
    assert_eq!(op(&mut b, 7, FileOp::Rename { from: p("moved/sub2"), to: p("moved/sub3") }), [done(7)]);
    assert_eq!(ls(&mut b, &p("moved/sub3")), Ok(names(&["b.txt", "deeper"])));

    // An unreadable folder can be renamed (its parent is readable), and stays unreadable.
    assert_eq!(op(&mut b, 8, FileOp::Rename { from: p("locked"), to: p("still-locked") })[0], done(8));
    assert_eq!(ls(&mut b, &p("still-locked")), Err(Error::PermissionDenied(p("still-locked"))));
    // Inside it nothing can be renamed.
    let evs = op(&mut b, 9, FileOp::Rename { from: p("still-locked/a"), to: p("still-locked/b") });
    assert_eq!(evs[0], failed(9, Error::PermissionDenied(p("still-locked/a"))));
}

#[test]
fn a_delete_counts_then_removes_everything_below_with_one_progress_event() {
    let mut b = connected();
    // tree, a.txt, sub, to-keep (a link: removed itself), b.txt, deeper; and x.txt.
    let evs = op(&mut b, 1, FileOp::Delete { paths: vec![p("tree"), p("x.txt"), p("tree/sub")] });
    assert_eq!(evs[..2], [ServiceEvent::FileOpProgress { id: OpId(1), host: prod(), done: 7, total: Some(7) }, done(1)]);
    assert_eq!(listed(&evs, &p("")), Some(names(&["flaky", "keep", "locked", "many"])));
    assert_eq!(ls(&mut b, &p("tree")), Err(Error::NotFound(p("tree"))));
    assert_eq!(ls(&mut b, &p("tree/sub/deeper")), Err(Error::NotFound(p("tree/sub/deeper"))));
    assert_eq!(ls(&mut b, &p("keep")), Ok(names(&["k.txt"])), "the link's target stays");

    // A big folder is counted whole.
    let evs = op(&mut b, 2, FileOp::Delete { paths: vec![p("many")] });
    assert_eq!(evs[0], ServiceEvent::FileOpProgress { id: OpId(2), host: prod(), done: 601, total: Some(601) });

    // Missing, unreadable: fails, and nothing goes (it counts first).
    let evs = op(&mut b, 3, FileOp::Delete { paths: vec![p("keep"), p("gone")] });
    assert_eq!(evs[0], failed(3, Error::NotFound(p("gone"))));
    assert_eq!(op(&mut b, 4, FileOp::Delete { paths: vec![p("keep"), p("locked")] })[0], failed(4, Error::PermissionDenied(p("locked"))));
    assert_eq!(ls(&mut b, &p("")), Ok(names(&["flaky", "keep", "locked"])));
    assert_eq!(op(&mut b, 5, FileOp::Delete { paths: vec![RemotePath::root()] })[0], failed(5, Error::PermissionDenied(RemotePath::root())));

    // A recreated folder is new and empty.
    op(&mut b, 6, FileOp::Mkdir { path: p("tree") });
    assert_eq!(ls(&mut b, &p("tree")), Ok(Vec::new()));
    assert_eq!(ls(&mut b, &p("tree/sub")), Err(Error::NotFound(p("tree/sub"))));
}

#[test]
fn deleting_the_folder_on_screen_lists_nothing_again() {
    let mut b = connected();
    ls(&mut b, &p("tree/sub")).unwrap();
    let evs = op(&mut b, 1, FileOp::Delete { paths: vec![p("tree")] });
    assert_eq!(evs, [ServiceEvent::FileOpProgress { id: OpId(1), host: prod(), done: 6, total: Some(6) }, done(1)]);
}

#[test]
fn a_folder_that_loses_the_connection_fails_the_delete_and_drops_it() {
    let mut b = connected();
    let evs = op(&mut b, 1, FileOp::Delete { paths: vec![p("flaky")] });
    assert_eq!(evs[0], failed(1, Error::ConnectionLost));
    assert!(evs.iter().any(|e| matches!(e, ServiceEvent::Status { status: ConnStatus::Reconnecting { attempt: 1 }, .. })), "{evs:#?}");
}

#[test]
fn uploads_after_a_change_show_on_top_of_it() {
    let mut b = connected();
    op(&mut b, 1, FileOp::Mkdir { path: p("new") });
    op(&mut b, 2, FileOp::Delete { paths: vec![p("x.txt")] });
    let up = JobRequest::Upload { host: prod(), items: vec![PathBuf::from("/Users/me/report.pdf")], into: p("") };
    let evs = b.send(Command::Transfer(TransferCommand::Start(up)));
    assert_eq!(listed(&evs, &p("")), Some(names(&["flaky", "keep", "locked", "many", "new", "report.pdf", "tree"])));
}

#[test]
fn rebasing_applies_what_the_engine_changed_since() {
    let e = |name: &str, size: u64| Entry { name: name.into(), kind: EntryKind::File, size, mtime: None, mode: None };
    let mut f = Changed { base: Some(vec![e("a", 1), e("b", 1), e("c", 1)]), now: Ok(vec![e("a", 1), e("c", 1), e("mine", 1)]) };
    // The engine replaced a, dropped c, added d; b (deleted here) is unchanged there.
    f.rebase(Some(vec![e("a", 2), e("b", 1), e("d", 1)]));
    assert_eq!(f.now, Ok(vec![e("a", 2), e("mine", 1), e("d", 1)]));
    assert_eq!(f.base, Some(vec![e("a", 2), e("b", 1), e("d", 1)]));
    // The engine can't list it: nothing changes.
    f.rebase(None);
    assert_eq!(f.now.as_ref().map(Vec::len), Ok(3));
    // Without a base, the first listing becomes it.
    let mut g = Changed { base: None, now: Ok(Vec::new()) };
    g.rebase(Some(vec![e("z", 1)]));
    assert_eq!((g.base, g.now), (Some(vec![e("z", 1)]), Ok(Vec::new())));
    assert_eq!(outermost(&[p("a/b"), p("a"), p("a")]), [p("a")]);
}
