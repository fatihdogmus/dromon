//! The context menus' entries, which are enabled when, and what their items do.

use dromon_core::{ConnStatus, HostId};

use super::*;
use crate::fixture;
use crate::state::{Action, App, ServiceEvent};

fn prod() -> HostId {
    HostId("prod".into())
}

fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::SelectHost(prod()));
    s
}

/// `(id, enabled)` per item, `—` for separators.
fn shape(s: &App, kind: &MenuKind) -> Vec<(&'static str, bool)> {
    s.menu(kind).iter().map(|e| e.item().map_or(("—", false), |i| (i.id, i.enabled))).collect()
}

#[test]
fn a_rows_menu_and_its_disabled_items() {
    let mut s = demo();
    s.act(Action::File(FileAction::OpenMenu { kind: MenuKind::Row("index.html".into()), at: Some((10.0, 20.0)) }));
    assert!(s.conn().unwrap().selection.contains("index.html"), "right-click selects the row first");
    let row = MenuKind::Row("index.html".into());
    assert_eq!(
        shape(&s, &row),
        [
            ("open", true),
            ("download", true),
            ("—", false),
            ("copy", true),
            ("cut", true),
            ("paste", false),
            ("copy-path", true),
            ("—", false),
            ("new-folder", true),
            ("rename", true),
            ("delete", true),
            ("—", false),
            ("properties", true)
        ],
        "no clip: Paste is off"
    );
    // New folder from a row's menu: the field opens in the folder on screen, as from the empty area.
    assert_eq!(s.menu_action(&row, "new-folder"), Some(Action::File(FileAction::NewFolder)));
    let item = |s: &App, id| s.menu(&row).into_iter().filter_map(|e| e.item().cloned()).find(|i| i.id == id).unwrap();
    assert!(item(&s, "delete").danger);
    assert_eq!(item(&s, "properties").shortcut, Some(properties_chord()));
    assert_eq!(properties_chord(), if cfg!(target_os = "macos") { "mod+i" } else { "alt+enter" });
    assert_eq!(s.menu_action(&row, "properties"), Some(Action::File(FileAction::Properties)));
    // With two selected, Rename and Properties are off; with a clip, Paste is on.
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: true });
    s.act(Action::File(FileAction::Copy));
    let enabled = |s: &App, id| shape(s, &row).into_iter().find(|(i, _)| *i == id).unwrap().1;
    assert!(!enabled(&s, "rename") && !enabled(&s, "properties") && enabled(&s, "paste"));
    assert_eq!(s.menu_action(&row, "properties"), None);
    s.act(Action::Click { name: "index.html".into(), extend: false, toggle: false });
    assert!(enabled(&s, "properties"));
    // Reconnecting: everything that touches the server is off; copy, copy path and Properties
    // (what the listing knows) stay.
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    for id in ["open", "download", "paste", "new-folder", "rename", "delete"] {
        assert!(!enabled(&s, id), "{id}");
    }
    assert!(enabled(&s, "copy") && enabled(&s, "cut") && enabled(&s, "copy-path") && enabled(&s, "properties"));
    assert_eq!(s.menu_action(&row, "delete"), None, "a disabled item does nothing");
    assert_eq!(s.menu_action(&row, "copy"), Some(Action::File(FileAction::Copy)));
}

#[test]
fn the_empty_areas_menu() {
    let mut s = demo();
    let empty = MenuKind::Empty;
    assert_eq!(shape(&s, &empty), [("new-folder", true), ("paste", false), ("—", false), ("refresh", true), ("hidden", true)]);
    let hidden = s.menu(&empty).into_iter().filter_map(|e| e.item().cloned()).find(|i| i.id == "hidden").unwrap();
    assert_eq!(hidden.checked, Some(true), "hidden files are shown by default");
    assert_eq!(s.menu_action(&empty, "hidden"), Some(Action::ToggleHidden));
    assert_eq!(s.menu_action(&empty, "new-folder"), Some(Action::File(FileAction::NewFolder)));
    // Picking runs it and closes the drawn menu.
    s.act(Action::File(FileAction::OpenMenu { kind: empty.clone(), at: Some((0.0, 0.0)) }));
    assert!(s.files.menu.is_some());
    s.act(Action::File(FileAction::Pick { kind: empty.clone(), id: "new-folder".into() }));
    assert!(s.files.menu.is_none() && s.files.edit.field().is_some());
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    assert_eq!(shape(&s, &empty), [("new-folder", false), ("paste", false), ("—", false), ("refresh", false), ("hidden", true)]);
}

#[test]
fn a_native_menu_only_selects_the_row() {
    let mut s = demo();
    s.act(Action::File(FileAction::OpenMenu { kind: MenuKind::Row("README.md".into()), at: None }));
    assert!(s.files.menu.is_none(), "the platform shows it");
    assert!(s.conn().unwrap().selection.contains("README.md"));
}
