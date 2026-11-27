//! The fixture backend's search in subfolders answers as the service's does: names with their
//! folders, unreadable folders noted, hidden files by the setting, a lost connection.

use std::sync::Arc;

use dromon_core::{ConnStatus, Error, HostId, RemotePath};

use super::*;
use crate::state::{Backend, Command, Found, SearchCommand, SearchEvent, SearchId, ServiceEvent};

fn prod() -> HostId {
    HostId("prod".into())
}

fn backend() -> FixtureBackend {
    let src = r#"{"version": 1, "now": "2026-11-25T14:00:00Z",
        "hosts": [{"alias": "prod", "source": "ssh-config"}],
        "servers": {"prod": {"home": "/w", "auth": [], "tree": {
            "/w": [{"name": "app.log", "kind": "file"}, {"name": "logs", "kind": "dir"},
                   {"name": ".cache", "kind": "dir"}, {"name": "locked", "kind": "dir"},
                   {"name": "flaky", "kind": "dir"}],
            "/w/logs": [{"name": "old.log", "kind": "file"}, {"name": "2026", "kind": "dir"}],
            "/w/logs/2026": [{"name": "jan.log", "kind": "file"}, {"name": "notes.txt", "kind": "file"}],
            "/w/.cache": [{"name": "cached.log", "kind": "file"}],
            "/w/locked": {"error": "permission-denied"},
            "/w/flaky": [{"name": "x", "kind": "file"}],
            "/w/gone": {"error": "connection-lost"}}}}}"#;
    let mut b = FixtureBackend::new(Arc::new(parse(src).unwrap_or_else(|e| panic!("{e}"))));
    b.send(Command::Connect { host: prod() });
    b
}

fn search(b: &mut FixtureBackend, root: &str, query: &str, show_hidden: bool) -> Vec<ServiceEvent> {
    let cmd = Command::Search(SearchCommand::Start {
        id: SearchId(1),
        host: prod(),
        root: RemotePath::new(root),
        query: query.into(),
        show_hidden,
    });
    b.send(cmd)
}

/// The search failing with `ConnectionLost`.
fn lost() -> ServiceEvent {
    ServiceEvent::Search(SearchEvent::Failed { id: SearchId(1), host: prod(), error: Error::ConnectionLost })
}

fn found(evs: &[ServiceEvent]) -> Vec<String> {
    let hits = evs.iter().filter_map(|e| match e {
        ServiceEvent::Search(SearchEvent::Found { entries, .. }) => {
            Some(entries.iter().map(|Found { folder, entry }| format!("{folder}/{}", entry.name)))
        }
        _ => None,
    });
    let mut out: Vec<String> = hits.flatten().collect();
    out.sort();
    out
}

#[test]
fn a_search_finds_names_in_every_folder_below_and_notes_what_it_couldnt_read() {
    let mut b = backend();
    let evs = search(&mut b, "/w", "LOG", false);
    assert_eq!(
        found(&evs),
        ["./app.log", "./logs", "logs/2026/jan.log", "logs/old.log"],
        "hidden .cache isn't searched"
    );
    assert_eq!(
        evs.last(),
        Some(&ServiceEvent::Search(SearchEvent::Done {
            id: SearchId(1),
            host: prod(),
            folders: 4,
            unreadable: 1,
            capped: false
        }))
    );
}

#[test]
fn hidden_entries_are_searched_when_they_are_shown_and_globs_match_whole_names() {
    let mut b = backend();
    assert_eq!(
        found(&search(&mut b, "/w", "*.log", true)),
        ["./app.log", ".cache/cached.log", "logs/2026/jan.log", "logs/old.log"]
    );
    assert_eq!(found(&search(&mut b, "/w/logs", "n?t*", true)), ["2026/notes.txt"]);
}

#[test]
fn a_search_on_a_host_that_isnt_up_fails_at_once() {
    let mut b = backend();
    b.send(Command::Disconnect { host: prod() });
    let evs = search(&mut b, "/w", "log", false);
    assert_eq!(evs, [lost()]);
}

#[test]
fn a_folder_that_loses_the_connection_fails_the_search_and_drops_it() {
    let mut b = backend();
    let evs = search(&mut b, "/w/gone", "x", false);
    assert!(evs.contains(&lost()));
    assert!(
        evs.iter().any(|e| matches!(e, ServiceEvent::Status { status: ConnStatus::Reconnecting { .. }, .. })),
        "{evs:#?}"
    );
}
