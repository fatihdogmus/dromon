//! Changes made elsewhere (`changes.rs`): parsing, and the backend answering polls as the
//! service does (only while the window shows, nothing when nothing changed, the folder on
//! screen vanishing, big folders left alone).

use std::sync::Arc;

use dromon_core::{EntryKind, Error, HostId, RemotePath};

use super::*;
use crate::state::{Backend, Command, ServiceEvent};

fn prod() -> HostId {
    HostId("prod".into())
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// prod, home /w (a.txt, sub/), /w/big with `big` generated files, and `changes` (JSON).
fn fixture(changes: &str, big: usize) -> Arc<Fixture> {
    let src = format!(
        r#"{{"version": 1, "now": "2026-11-25T14:00:00Z",
            "hosts": [{{"alias": "prod", "source": "ssh-config"}}],
            "servers": {{"prod": {{"home": "/w", "changes": {changes}, "tree": {{
                "/w": [{{"name": "a.txt", "kind": "file", "size": 10, "mtime": 1}}, {{"name": "sub", "kind": "dir"}}, {{"name": "big", "kind": "dir"}}],
                "/w/sub": [{{"name": "inner.txt", "kind": "file", "size": 1}}],
                "/w/big": [{{"generate": {{"pattern": "f{{05}}", "count": {big}}}}}]}}}}}}}}"#
    );
    Arc::new(parse(&src).unwrap_or_else(|e| panic!("{e}\n{src}")))
}

fn error(changes: &str) -> String {
    match parse(&format!(
        r#"{{"version": 1, "hosts": [{{"alias": "h", "source": "saved"}}], "servers": {{"h": {{"home": "/", "changes": {changes}}}}}}}"#
    )) {
        Ok(_) => panic!("parsed: {changes}"),
        Err(e) => e.to_string(),
    }
}

/// Connected to prod with `path` listed and the window `focused` (or hidden).
fn connected(fx: Arc<Fixture>, path: &str, visible: bool) -> FixtureBackend {
    let mut b = FixtureBackend::new(fx);
    b.send(Command::Window { visible, focused: visible });
    b.send(Command::Connect { host: prod() });
    b.send(Command::List { host: prod(), path: p(path) });
    b
}

fn poll(b: &mut FixtureBackend) -> Vec<ServiceEvent> {
    b.send(Command::Poll { host: prod() })
}

fn refreshed(evs: &[ServiceEvent]) -> Option<Vec<String>> {
    evs.iter().find_map(|e| match e {
        ServiceEvent::Refreshed { entries, .. } => {
            let mut names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
            names.sort();
            Some(names)
        }
        _ => None,
    })
}

#[test]
fn changes_parse_as_steps_of_one_or_several() {
    let fx = fixture(
        r#"[{"add": "/w/new.txt", "kind": "file", "size": 5, "mtime": "2026-11-25T13:59:00Z"},
            [{"remove": "/w/a.txt"}, {"modify": "/w/sub", "mtime": 7}]]"#,
        1,
    );
    let steps = &fx.server("prod").unwrap().changes;
    assert_eq!(steps.len(), 2);
    let Change::Add { path, entry } = &steps[0][0] else { panic!("{steps:?}") };
    assert_eq!((path.as_str(), entry.name.as_str(), entry.kind, entry.size), ("/w/new.txt", "new.txt", EntryKind::File, 5));
    assert_eq!(steps[1], [Change::Remove { path: p("/w/a.txt") }, Change::Modify { path: p("/w/sub"), size: None, mtime: Some(7) }]);
}

#[test]
fn change_errors_name_their_place() {
    assert_eq!(error(r#"[{"remove": "w"}]"#), r#"servers.h.changes: step 1: remove: "w" is not an absolute path"#);
    assert_eq!(error(r#"[{"remove": "/"}]"#), "servers.h.changes: step 1: remove: the root can't change");
    assert_eq!(error(r#"[{"rename": "/a"}]"#), r#"servers.h.changes: step 1: a change is {"add": path, …}, {"remove": path} or {"modify": path, …}"#);
    assert_eq!(error(r#"[[{"remove": "/a"}, {"modify": "/a"}]]"#), "servers.h.changes: step 1: change 2: a modify changes a size, an mtime or both");
    assert_eq!(error(r#"[{"add": "/a"}]"#), "servers.h.changes: step 1: kind: missing");
    assert_eq!(error(r#"[{"add": "/a", "name": "b", "kind": "file"}]"#), "servers.h.changes: step 1: an added entry takes its name from its path, and can't be a generator");
    assert_eq!(error(r#"[{"remove": "/a", "size": 1}]"#), r#"servers.h.changes: step 1: unknown field "size" (expected remove)"#);
}

#[test]
fn a_poll_takes_the_next_step_and_says_only_what_changed() {
    let mut b = connected(fixture(r#"[{"add": "/w/new.txt", "kind": "file", "size": 5}, [], {"modify": "/w/a.txt", "size": 11}]"#, 1), "/w", true);
    assert_eq!(refreshed(&poll(&mut b)).unwrap(), ["a.txt", "big", "new.txt", "sub"]);
    // An empty step: nothing changed, nothing sent.
    assert!(poll(&mut b).is_empty());
    let evs = poll(&mut b);
    let ServiceEvent::Refreshed { entries, .. } = &evs[0] else { panic!("{evs:?}") };
    assert_eq!(entries.iter().find(|e| e.name == "a.txt").unwrap().size, 11);
    // Out of steps: quiet from now on.
    assert!(poll(&mut b).is_empty());
}

#[test]
fn nothing_is_polled_while_the_window_is_hidden() {
    let mut b = connected(fixture(r#"[{"add": "/w/new.txt", "kind": "file"}]"#, 1), "/w", false);
    assert!(poll(&mut b).is_empty());
    // The step is still waiting when the window comes back.
    b.send(Command::Window { visible: true, focused: false });
    assert_eq!(refreshed(&poll(&mut b)).unwrap(), ["a.txt", "big", "new.txt", "sub"]);
}

#[test]
fn a_refresh_of_the_folder_on_screen_takes_the_next_step_too() {
    let mut b = connected(fixture(r#"[{"remove": "/w/a.txt"}]"#, 1), "/w", true);
    // Navigating elsewhere doesn't.
    b.send(Command::List { host: prod(), path: p("/w/sub") });
    let evs = b.send(Command::List { host: prod(), path: p("/w") });
    assert!(evs.iter().any(|e| matches!(e, ServiceEvent::ListingPage { entries, .. } if entries.iter().any(|e| e.name == "a.txt"))));
    // ⌘R on /w does: a.txt is gone.
    let evs = b.send(Command::List { host: prod(), path: p("/w") });
    let ServiceEvent::ListingPage { entries, .. } = &evs[0] else { panic!("{evs:?}") };
    assert!(!entries.iter().any(|e| e.name == "a.txt"), "{entries:?}");
}

#[test]
fn the_folder_on_screen_deleted_elsewhere_fails_once() {
    let mut b = connected(fixture(r#"[{"remove": "/w/sub"}]"#, 1), "/w/sub", true);
    let evs = poll(&mut b);
    assert_eq!(evs, [ServiceEvent::ListFailed { host: prod(), path: p("/w/sub"), error: Error::NotFound(p("/w/sub")) }]);
    assert!(poll(&mut b).is_empty(), "not polled again");
    // Its parent lists without it.
    let evs = b.send(Command::List { host: prod(), path: p("/w") });
    let ServiceEvent::ListingPage { entries, .. } = &evs[0] else { panic!("{evs:?}") };
    assert!(!entries.iter().any(|e| e.name == "sub"));
}

#[test]
fn a_folder_over_twenty_thousand_entries_is_never_polled() {
    let mut b = connected(fixture(r#"[{"add": "/w/big/late.txt", "kind": "file"}]"#, 20_001), "/w/big", true);
    assert!(poll(&mut b).is_empty());
    // ⌘R still lists it, with the change.
    let evs = b.send(Command::List { host: prod(), path: p("/w/big") });
    let n: usize = evs.iter().map(|e| if let ServiceEvent::ListingPage { entries, .. } = e { entries.len() } else { 0 }).sum();
    assert_eq!(n, 20_002);
}
