//! Context menus driven headless: tests draw them (as Linux does), so what they hold can be
//! read and clicked. The native path builds its `PopupMenu` from the same entries.

use dromon_core::{ConnStatus, HostId};
use rune::{Env, Key, KeyEvent, Mods, PopupEntry, Runtime};

use crate::state::menu::MenuKind;
use crate::state::{Action, App, ServiceEvent};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

fn prod() -> HostId {
    HostId("prod".into())
}

fn runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.use_clipboard(&rune::clipboard::memory());
    rt.update(|s, _| {
        *s = scenes::base();
        s.act(Action::SelectHost(prod()));
    });
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn at(rt: &Runtime<App>, key: &str) -> (f32, f32) {
    rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"))
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = at(rt, key);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn right_click(rt: &mut Runtime<App>, (x, y): (f32, f32)) {
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    settle(rt);
}

fn shown(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

/// The drawn menu's items, by label, in order.
fn items(rt: &Runtime<App>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting && matches!(e.role, "menuitem" | "menuitemcheckbox"))
        .filter_map(|e| e.label)
        .collect()
}

fn below_rows(rt: &Runtime<App>) -> (f32, f32) {
    let (x, y) = at(rt, "rows/server.log");
    (x, y + 80.0)
}

#[test]
fn a_rows_menu_selects_it_and_lists_its_actions() {
    let mut rt = runtime();
    click(&mut rt, "rows/README.md");
    let at_row = at(&rt, "rows/index.html");
    right_click(&mut rt, at_row);
    assert!(shown(&rt, "ctx-menu"));
    let c = rt.state.conn().unwrap();
    assert!(c.selection.contains("index.html") && !c.selection.contains("README.md"), "right-click selects the row");
    assert_eq!(items(&rt), ["Open", "Download", "Copy", "Cut", "Paste", "Copy path", "New folder", "Rename", "Delete"]);
    // Paste without a clip is disabled: clicking it does nothing, and the menu stays.
    click(&mut rt, "ctx-paste");
    assert!(shown(&rt, "ctx-menu"));
    click(&mut rt, "ctx-rename");
    assert!(!shown(&rt, "ctx-menu"));
    assert!(shown(&rt, "edit-input"), "Rename opens the field");
}

#[test]
fn new_folder_from_a_rows_menu_for_a_folder_with_no_empty_area() {
    let mut rt = runtime();
    let at_row = at(&rt, "rows/index.html");
    right_click(&mut rt, at_row);
    click(&mut rt, "ctx-new-folder");
    assert!(!shown(&rt, "ctx-menu"));
    assert!(shown(&rt, "edit-input"), "the new folder's name field opens in the folder on screen");
}

#[test]
fn right_clicking_a_selected_row_keeps_the_selection_and_rename_needs_one() {
    let mut rt = runtime();
    rt.update(|s, _| {
        s.act(Action::Click { name: "README.md".into(), extend: false, toggle: false });
        s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: true });
    });
    settle(&mut rt);
    let at_row = at(&rt, "rows/robots.txt");
    right_click(&mut rt, at_row);
    assert_eq!(rt.state.conn().unwrap().selection.len(), 2);
    click(&mut rt, "ctx-rename");
    assert!(!shown(&rt, "edit-input"), "Rename is for one item");
    // Escape closes it.
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    settle(&mut rt);
    assert!(!shown(&rt, "ctx-menu"));
}

#[test]
fn the_empty_areas_menu_makes_a_folder_and_toggles_hidden_files() {
    let mut rt = runtime();
    let spot = below_rows(&rt);
    right_click(&mut rt, spot);
    assert_eq!(items(&rt), ["New folder", "Paste", "Refresh", "Show hidden files"]);
    let check = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("ctx-hidden")).map(|e| e.role);
    assert_eq!(check, Some("menuitemcheckbox"));
    click(&mut rt, "ctx-hidden");
    assert!(rt.find("rows/.env").is_none(), "hidden files hidden");
    right_click(&mut rt, spot);
    click(&mut rt, "ctx-new-folder");
    assert!(shown(&rt, "new-folder"));
}

#[test]
fn a_press_outside_closes_the_menu() {
    let mut rt = runtime();
    let spot = below_rows(&rt);
    right_click(&mut rt, spot);
    assert!(shown(&rt, "ctx-menu"));
    rt.pointer_down(20.0, 640.0);
    rt.pointer_up(20.0, 640.0);
    settle(&mut rt);
    assert!(!shown(&rt, "ctx-menu"));
    assert_eq!(rt.state.files.menu, None);
}

#[test]
fn while_reconnecting_the_server_changing_items_are_off() {
    let mut rt = runtime();
    rt.update(|s, _| {
        s.act(Action::Click { name: "index.html".into(), extend: false, toggle: false });
        s.act(Action::File(crate::state::FileAction::Copy));
        s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
        // The listing is inert while it reconnects; a menu already asked for still opens.
        s.act(Action::File(crate::state::FileAction::OpenMenu { kind: MenuKind::Row("index.html".into()), at: Some((400.0, 200.0)) }));
    });
    settle(&mut rt);
    for k in ["ctx-delete", "ctx-rename", "ctx-new-folder", "ctx-paste", "ctx-download"] {
        click(&mut rt, k);
        assert!(shown(&rt, "ctx-menu"), "{k} is disabled");
    }
    assert!(!rt.state.ui.sheets.is_open());
    click(&mut rt, "ctx-copy-path");
    assert!(!shown(&rt, "ctx-menu"), "copy path still works");
}

const SHIFT: Mods = Mods { shift: true, cmd: false, alt: false, ctrl: false, logo: false };

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

fn rect_of(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("no element {key:?}")).rect
}

#[test]
fn shift_f10_opens_the_cursor_rows_menu_at_the_row_and_the_menu_key_too() {
    let row_items = ["Open", "Download", "Copy", "Cut", "Paste", "Copy path", "New folder", "Rename", "Delete"];
    for (key, mods) in [(Key::Function(10), SHIFT), (Key::ContextMenu, Mods::default())] {
        let mut rt = runtime();
        click(&mut rt, "rows/index.html");
        assert!(!shown(&rt, "ctx-menu"));
        press(&mut rt, key, mods);
        assert!(shown(&rt, "ctx-menu"), "the menu opens");
        assert_eq!(items(&rt), row_items);
        // Anchored below the cursor row's bottom-left, as a right-click's is at the pointer.
        let (list, menu) = (rect_of(&rt, "list"), rect_of(&rt, "ctx-menu"));
        let (_, centre) = at(&rt, "rows/index.html");
        let bottom = centre + rt.state.settings.density.row() / 2.0;
        assert!((menu[0] - list[0]).abs() < 1.5, "menu x {} at the row's left {}", menu[0], list[0]);
        assert!((menu[1] - (bottom + 2.0)).abs() < 1.5, "menu y {} below the row's bottom {bottom}", menu[1]);
        assert_eq!(rt.state.conn().unwrap().selection.cursor(), Some("index.html"));
        press(&mut rt, Key::Escape, Mods::default());
        assert!(!shown(&rt, "ctx-menu"));
    }
}

#[test]
fn shift_f10_without_a_cursor_row_opens_the_empty_areas_menu_and_never_over_a_field() {
    let mut rt = runtime();
    rt.update(|s, _| drop(s.act(Action::Escape)));
    settle(&mut rt);
    assert_eq!(rt.state.conn().unwrap().selection.cursor(), None);
    press(&mut rt, Key::Function(10), SHIFT);
    assert_eq!(items(&rt), ["New folder", "Paste", "Refresh", "Show hidden files"]);
    let (list, menu) = (rect_of(&rt, "list"), rect_of(&rt, "ctx-menu"));
    assert!((menu[0] - list[0]).abs() < 1.5 && (menu[1] - (list[1] + 2.0)).abs() < 1.5, "at the list's top-left: {menu:?} in {list:?}");
    press(&mut rt, Key::Escape, Mods::default());
    assert!(!shown(&rt, "ctx-menu"));

    // The filter field has the keyboard: the list's menu stays shut.
    rt.update(|s, _| drop(s.act(Action::OpenFilter)));
    settle(&mut rt);
    assert!(rt.state.text_focused());
    press(&mut rt, Key::Function(10), SHIFT);
    assert!(!shown(&rt, "ctx-menu") && rt.state.files.menu.is_none());
    // Nor over a sheet.
    rt.update(|s, _| drop(s.act(Action::CloseFilter)));
    rt.update(|s, _| s.apply(ServiceEvent::Log { host: prod(), line: "x".into() }));
    rt.update(|s, _| drop(s.act(Action::OpenLog)));
    settle(&mut rt);
    assert!(rt.state.ui.sheets.is_open());
    press(&mut rt, Key::ContextMenu, Mods::default());
    assert!(rt.state.files.menu.is_none());
}

#[test]
fn the_native_menu_has_the_same_items_and_shortcut_hints() {
    let mut rt = runtime();
    rt.update(|s, _| {
        s.act(Action::File(crate::state::FileAction::Copy));
    });
    let entries = rt.state.menu(&MenuKind::Row("index.html".into()));
    let menu = crate::views::menu_popup(&entries);
    let ids: Vec<String> = menu
        .entries()
        .iter()
        .map(|e| match e {
            PopupEntry::Item(i) => i.id().to_owned(),
            _ => "—".to_owned(),
        })
        .collect();
    assert_eq!(ids, ["open", "download", "—", "copy", "cut", "paste", "copy-path", "—", "new-folder", "rename", "delete"]);
    let copy = menu.find("copy").unwrap();
    assert_eq!(copy.key_equivalent_shortcut().map(ToString::to_string), "mod+c".parse::<rune::Shortcut>().ok().map(|s| s.to_string()));
    assert!(!menu.find("rename").unwrap().is_enabled() || rt.state.conn().unwrap().selection.len() == 1);
    let empty = crate::views::menu_popup(&rt.state.menu(&MenuKind::Empty));
    assert!(empty.find("hidden").unwrap().is_checked());
}
