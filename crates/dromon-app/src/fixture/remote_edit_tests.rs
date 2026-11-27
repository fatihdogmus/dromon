//! Editing in a local editor on the fixture backend answers as the service does: a made-up copy
//! opens, each window focus saves it into the tree, a change elsewhere in between is a conflict,
//! Keep both lands under the next free name and the edit goes on against its path, Discard
//! takes the tree's copy, and Stop and a disconnect close edits.

use std::sync::Arc;

use dromon_core::{Error, HostId, RemotePath};
use dromon_transfer::Side;

use super::FOLDER;
use crate::fixture::{FixtureBackend, parse};
use crate::state::{
    Backend, Command, EditChoice, EditId, EditStage, RemoteEditCommand, RemoteEditEvent, SearchCommand, SearchEvent,
    SearchId, ServiceEvent,
};

fn prod() -> HostId {
    HostId("prod".into())
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// prod, its /w holding notes.txt; between the first and the second poll someone else rewrites
/// it, then nothing more changes.
fn backend() -> FixtureBackend {
    let src = r#"{"version": 1, "now": "2026-11-25T14:00:00Z",
        "hosts": [{"alias": "prod", "source": "ssh-config"}],
        "servers": {"prod": {"home": "/w",
            "changes": [[], {"modify": "/w/notes.txt", "size": 99, "mtime": "2026-11-25T13:59:00Z"}],
            "tree": {"/w": [{"name": "notes.txt", "kind": "file", "size": 10, "mtime": "2026-11-20T10:00:00Z"},
                            {"name": "notes (1).txt", "kind": "file", "size": 3},
                            {"name": "docs", "kind": "dir"}]}}}}"#;
    let mut b = FixtureBackend::new(Arc::new(parse(src).unwrap_or_else(|e| panic!("{e}"))));
    b.send(Command::Window { visible: true, focused: true });
    b.send(Command::Connect { host: prod() });
    b.send(Command::List { host: prod(), path: p("/w") });
    b
}

fn edits(evs: Vec<ServiceEvent>) -> Vec<RemoteEditEvent> {
    evs.into_iter()
        .filter_map(|e| match e {
            ServiceEvent::RemoteEdit(ev) => Some(ev),
            _ => None,
        })
        .collect()
}

fn send(b: &mut FixtureBackend, cmd: RemoteEditCommand) -> Vec<RemoteEditEvent> {
    edits(b.send(Command::RemoteEdit(cmd)))
}

/// The window came back from the editor.
fn poll(b: &mut FixtureBackend) -> Vec<RemoteEditEvent> {
    edits(b.send(Command::Poll { host: prod() }))
}

const ID: EditId = EditId(1);

fn start(b: &mut FixtureBackend, path: &str) -> Vec<RemoteEditEvent> {
    send(b, RemoteEditCommand::Start { id: ID, host: prod(), path: p(path) })
}

fn saved(to: &str) -> Vec<RemoteEditEvent> {
    vec![
        RemoteEditEvent::Uploading { id: ID, host: prod() },
        RemoteEditEvent::Saved { id: ID, host: prod(), to: p(to) },
    ]
}

/// What the tree has at `path` now: its size and date (found by a search, which, unlike listing
/// the folder on screen again, takes no step of the changes elsewhere).
fn side(b: &mut FixtureBackend, path: &str) -> Option<Side> {
    let path = p(path);
    let (root, name) = (path.parent().unwrap(), path.file_name().unwrap().to_owned());
    let search = SearchCommand::Start { id: SearchId(1), host: prod(), root, query: name.clone(), show_hidden: true };
    b.send(Command::Search(search)).into_iter().find_map(|e| match e {
        ServiceEvent::Search(SearchEvent::Found { entries, .. }) => entries
            .into_iter()
            .find(|f| f.folder == "." && f.entry.name == name)
            .map(|f| Side { size: Some(f.entry.size), mtime: f.entry.mtime }),
        _ => None,
    })
}

#[test]
fn an_edit_opens_a_made_up_copy_and_each_focus_saves_it() {
    let mut b = backend();
    let [RemoteEditEvent::Opened { id, host, path, local }] = &start(&mut b, "/w/notes.txt")[..] else { panic!() };
    assert_eq!((*id, host, path.as_str()), (ID, &prod(), "/w/notes.txt"));
    assert!(local.starts_with(FOLDER) && local.ends_with("notes.txt"), "{local:?}");
    assert_eq!(b.probe().edited_copies(), std::slice::from_ref(local));
    // The first poll changes nothing elsewhere: the save lands, dated the fixture's clock.
    assert_eq!(poll(&mut b), saved("/w/notes.txt"));
    assert_eq!(side(&mut b, "/w/notes.txt"), Some(Side { size: Some(10), mtime: Some(1_795_615_200) }));
    // A folder, a file that isn't there, a host that isn't connected: no edit.
    let fail = |error| vec![RemoteEditEvent::Failed { id: EditId(2), host: prod(), stage: EditStage::Download, error }];
    let mut other = |path: &str| send(&mut b, RemoteEditCommand::Start { id: EditId(2), host: prod(), path: p(path) });
    assert_eq!(other("/w/docs"), fail(Error::NotFound(p("/w/docs"))));
    assert_eq!(other("/w/gone.txt"), fail(Error::NotFound(p("/w/gone.txt"))));
    assert_eq!(other("/w/notes.txt"), fail(Error::Other("/w/notes.txt is open for editing already".into())));
}

#[test]
fn a_change_elsewhere_is_a_conflict_and_keep_both_takes_the_next_free_name() {
    let mut b = backend();
    start(&mut b, "/w/notes.txt");
    assert_eq!(poll(&mut b), saved("/w/notes.txt"));
    // The second poll brings someone else's change: the save asks instead.
    let evs = poll(&mut b);
    let server = Side { size: Some(99), mtime: Some(1_795_615_140) };
    let [RemoteEditEvent::Uploading { .. }, RemoteEditEvent::Conflict { server: s, .. }] = &evs[..] else {
        panic!("{evs:?}")
    };
    assert_eq!(*s, server);
    assert!(poll(&mut b).is_empty(), "nothing uploads while it asks");
    assert!(send(&mut b, RemoteEditCommand::Retry { id: ID, host: prod() }).is_empty());
    // Keep both: "notes (1).txt" is taken, so "notes (2).txt"; theirs stays.
    let evs = send(&mut b, RemoteEditCommand::Resolve { id: ID, host: prod(), choice: EditChoice::KeepBoth });
    assert_eq!(evs, [RemoteEditEvent::Saved { id: ID, host: prod(), to: p("/w/notes (2).txt") }]);
    assert_eq!(side(&mut b, "/w/notes.txt"), Some(server));
    assert!(side(&mut b, "/w/notes (2).txt").is_some());
    // The edit goes on against notes.txt, still the newer copy there: the next save asks again.
    let evs = poll(&mut b);
    assert!(matches!(evs[..], [RemoteEditEvent::Uploading { .. }, RemoteEditEvent::Conflict { .. }]), "{evs:?}");
    // Discard takes theirs; from then on saves land on it.
    let evs = send(&mut b, RemoteEditCommand::Resolve { id: ID, host: prod(), choice: EditChoice::Discard });
    assert_eq!(evs, [RemoteEditEvent::Reloaded { id: ID, host: prod() }]);
    assert_eq!(poll(&mut b), saved("/w/notes.txt"));
}

#[test]
fn replace_uploads_over_the_change() {
    let mut b = backend();
    start(&mut b, "/w/notes.txt");
    poll(&mut b);
    poll(&mut b);
    let evs = send(&mut b, RemoteEditCommand::Resolve { id: ID, host: prod(), choice: EditChoice::Replace });
    assert_eq!(evs, [RemoteEditEvent::Saved { id: ID, host: prod(), to: p("/w/notes.txt") }]);
    // The copy's size (it holds what was downloaded), dated the fixture's clock.
    assert_eq!(side(&mut b, "/w/notes.txt"), Some(Side { size: Some(10), mtime: Some(1_795_615_200) }));
    assert_eq!(poll(&mut b), saved("/w/notes.txt"), "no conflict after it");
}

#[test]
fn stop_and_a_disconnect_close_edits() {
    let mut b = backend();
    start(&mut b, "/w/notes.txt");
    assert_eq!(
        send(&mut b, RemoteEditCommand::Stop { id: ID, host: prod() }),
        [RemoteEditEvent::Closed { id: ID, host: prod() }]
    );
    assert!(b.probe().edited_copies().is_empty(), "its copy is gone");
    start(&mut b, "/w/notes.txt");
    let evs = edits(b.send(Command::Disconnect { host: prod() }));
    assert_eq!(evs, [RemoteEditEvent::Closed { id: ID, host: prod() }]);
    assert!(b.probe().edited_copies().is_empty());
    // Not connected: nothing starts, and what's unknown is closed.
    let lost =
        RemoteEditEvent::Failed { id: ID, host: prod(), stage: EditStage::Download, error: Error::ConnectionLost };
    assert_eq!(start(&mut b, "/w/notes.txt"), [lost]);
    assert_eq!(
        send(&mut b, RemoteEditCommand::Retry { id: ID, host: prod() }),
        [RemoteEditEvent::Closed { id: ID, host: prod() }]
    );
}
