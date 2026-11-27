//! Editing in a local editor driven headless through the real view and handlers, on a fixture
//! (which makes the copies up and saves them each time the window gains focus): the row menu's
//! Edit opens the copy and the transfers popover's Editing section lists it; Stop editing takes
//! it away, the fixture's copy with it; a change elsewhere brings the conflict sheet, whose Keep
//! both uploads beside the file. The machine is `state/remote_edit_tests.rs`'s.

use std::sync::Arc;

use dromon_core::HostId;
use rune::{Env, Runtime};

use crate::app;
use crate::fixture::{self, Fixture, FixtureBackend};
use crate::state::menu::MenuKind;
use crate::state::remote_edit::Phase;
use crate::state::{Action, App, FileAction};

const SIZE: (f32, f32) = (1080.0, 680.0);

fn prod() -> HostId {
    HostId("prod".into())
}

/// prod, its /w holding notes.txt (rewritten elsewhere at the second poll) and a folder.
fn edited_fixture() -> Arc<Fixture> {
    let src = r#"{"version": 1, "now": "2026-11-25T14:00:00Z",
        "hosts": [{"alias": "prod", "source": "ssh-config"}],
        "servers": {"prod": {"home": "/w",
            "changes": [[], {"modify": "/w/notes.txt", "size": 99, "mtime": "2026-11-25T13:59:00Z"}],
            "tree": {"/w": [{"name": "notes.txt", "kind": "file", "size": 10, "mtime": "2026-11-20T10:00:00Z"},
                            {"name": "docs", "kind": "dir"}]}}}}"#;
    Arc::new(fixture::parse(src).unwrap_or_else(|e| panic!("{e}")))
}

/// The app on `fx`, connected to prod, and a look into its fixture.
fn runtime(fx: Arc<Fixture>) -> (Runtime<App>, FixtureBackend) {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    let backend = FixtureBackend::new(fx.clone());
    let probe = backend.probe();
    rt.update(move |s, _| {
        *s = fixture::app(fx);
        s.backend = Box::new(backend);
        s.act(Action::SelectHost(prod()));
    });
    settle(&mut rt);
    (rt, probe)
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn shown(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

fn text_of(rt: &Runtime<App>, k: &str) -> Option<String> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).and_then(|e| e.text)
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn right_click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    settle(rt);
}

/// The window went to the editor and came back (the fixture saves then).
fn come_back(rt: &mut Runtime<App>) {
    rt.window_focus(false);
    settle(rt);
    rt.window_focus(true);
    settle(rt);
}

#[test]
fn the_row_menus_edit_opens_the_copy_and_stop_editing_takes_it_away() {
    let (mut rt, probe) = runtime(edited_fixture());
    // A folder's Edit is there, but does nothing.
    right_click(&mut rt, "rows/docs");
    assert!(shown(&rt, "ctx-edit"));
    click(&mut rt, "ctx-edit");
    assert!(rt.state.edits.sessions.is_empty() && probe.edited_copies().is_empty());
    rt.update(|s, _| drop(s.act(Action::File(FileAction::CloseMenu))));
    settle(&mut rt);
    right_click(&mut rt, "rows/notes.txt");
    click(&mut rt, "ctx-edit");
    let copies = probe.edited_copies();
    let [copy] = &copies[..] else { panic!("{copies:?}") };
    let opened: Vec<_> = rt.take_path_requests().into_iter().map(|r| r.path).collect();
    assert_eq!(opened, std::slice::from_ref(copy), "the copy opens in the user's app");
    // The Editing section lists it, with its host and how it stands.
    click(&mut rt, "tb-transfers");
    assert!(shown(&rt, "editing"));
    let id = rt.state.edits.sessions[0].id.0;
    assert_eq!(text_of(&rt, &format!("edit-title-{id}")).as_deref(), Some("notes.txt"));
    assert_eq!(text_of(&rt, &format!("edit-line-{id}")).as_deref(), Some("prod · Editing"));
    click(&mut rt, &format!("edit-stop-{id}"));
    assert!(rt.state.edits.sessions.is_empty());
    assert!(!shown(&rt, "editing"), "the section goes with the last edit");
    assert!(probe.edited_copies().is_empty(), "the fixture's copy is gone");
}

#[test]
fn a_change_elsewhere_brings_the_conflict_sheet_and_keep_both_uploads_beside_it() {
    let (mut rt, _) = runtime(edited_fixture());
    rt.update(|s, _| {
        drop(s.act(Action::File(FileAction::OpenMenu { kind: MenuKind::Row("notes.txt".into()), at: None })))
    });
    rt.update(|s, _| {
        drop(s.act(Action::File(FileAction::Pick { kind: MenuKind::Row("notes.txt".into()), id: "edit".into() })))
    });
    settle(&mut rt);
    // Back from the editor: saved.
    come_back(&mut rt);
    assert!(!shown(&rt, "edit-conflict"));
    assert!(rt.state.toasts().iter().any(|t| t.text == "Saved notes.txt to prod"));
    // Someone else rewrote it meanwhile: the next save asks. With the popover open, its row
    // says so and offers to answer; that closes the popover and the sheet comes up.
    click(&mut rt, "tb-transfers");
    come_back(&mut rt);
    assert!(matches!(rt.state.edits.sessions[0].phase, Phase::Conflict { .. }));
    assert!(!shown(&rt, "edit-conflict"), "not over the popover");
    let id = rt.state.edits.sessions[0].id.0;
    assert_eq!(text_of(&rt, &format!("edit-line-{id}")).as_deref(), Some("prod · Changed on the server"));
    click(&mut rt, &format!("edit-resolve-{id}"));
    assert!(shown(&rt, "edit-conflict"));
    assert_eq!(text_of(&rt, "sheet-title").as_deref(), Some("‘notes.txt’ changed on prod"));
    assert_eq!(text_of(&rt, "edit-conflict-server-size").as_deref(), Some("99 bytes"));
    click(&mut rt, "edit-conflict-keep-both");
    assert!(!shown(&rt, "edit-conflict"));
    assert_eq!(rt.state.edits.sessions[0].phase, Phase::Open);
    let names: Vec<String> = rt.state.conn().unwrap().entries().iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"notes (1).txt".to_owned()), "{names:?}");
}
