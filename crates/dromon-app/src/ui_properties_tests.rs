//! The Properties sheet driven headless through the real view, handlers and `on_key`, on the
//! demo fixture (which walks folders at once): opened from the row menu and the keyboard, its
//! fields, Calculate's total, and the ways it closes. A walk under way, its cancelling and stale
//! answers are `state/properties_tests.rs`'s.

use dromon_core::HostId;
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::state::{Action, App};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

fn runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| {
        *s = scenes::base();
        s.act(Action::SelectHost(HostId("prod".into())));
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

fn shown(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

fn text_of(rt: &Runtime<App>, k: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).and_then(|e| e.text).unwrap_or_else(|| panic!("no text {k:?}"))
}

fn label_of(rt: &Runtime<App>, k: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).and_then(|e| e.label).unwrap_or_default()
}

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

/// Properties' chord on this platform: ⌘I on macOS, Alt+Return elsewhere.
fn properties_key(rt: &mut Runtime<App>) {
    if cfg!(target_os = "macos") {
        press(rt, Key::Command('i'), Mods { cmd: true, logo: true, ..Mods::default() });
    } else {
        press(rt, Key::Enter, Mods { alt: true, ..Mods::default() });
    }
}

#[test]
fn properties_opens_from_the_rows_menu_and_shows_what_the_listing_knows() {
    let mut rt = runtime();
    let row = at(&rt, "rows/index.html");
    rt.pointer_move(row.0, row.1);
    rt.secondary_down(row.0, row.1);
    rt.secondary_up(row.0, row.1);
    settle(&mut rt);
    click(&mut rt, "ctx-properties");
    assert!(shown(&rt, "properties"), "the sheet opens");
    assert_eq!(label_of(&rt, "properties"), "index.html properties");
    let fields: Vec<String> = ["sheet-title", "prop-kind-value", "prop-location-value", "prop-size-value", "prop-modified-value", "prop-permissions-value", "prop-owner-value"]
        .iter()
        .map(|k| text_of(&rt, k))
        .collect();
    assert_eq!(fields, ["index.html", "File", "/var/www/site", "14.8 KB (14,812 bytes)", "Nov 24, 2026 at 18:00", "rw-r--r-- (644)", "—"]);
    assert!(!shown(&rt, "prop-calculate"), "a file's size is known");
    // Escape closes it; the keyboard opens it again, and Return (Close is the default) closes it.
    press(&mut rt, Key::Escape, Mods::default());
    assert!(!shown(&rt, "properties"));
    properties_key(&mut rt);
    assert!(shown(&rt, "properties"), "the chord opens it for the selected row");
    press(&mut rt, Key::Enter, Mods::default());
    assert!(!shown(&rt, "properties"));
    assert!(rt.state.ui.sheets.open().is_none());
}

#[test]
fn calculate_shows_a_folders_total() {
    let mut rt = runtime();
    click(&mut rt, "rows/releases");
    properties_key(&mut rt);
    assert_eq!(text_of(&rt, "prop-kind-value"), "Folder");
    assert_eq!(label_of(&rt, "prop-calculate"), "Calculate");
    click(&mut rt, "prop-calculate");
    assert_eq!(text_of(&rt, "prop-size-value"), "303 KB (303,365 bytes)");
    assert_eq!(text_of(&rt, "prop-size-counts"), "3 files, 4 folders");
    assert!(!shown(&rt, "prop-size-note"), "every folder was read");
    assert_eq!(label_of(&rt, "prop-calculate"), "Recalculate");
    click(&mut rt, "prop-close");
    assert!(!shown(&rt, "properties"));
}

#[test]
fn a_folder_that_cant_be_read_is_noted() {
    let mut rt = runtime();
    rt.update(|s, _| drop(s.act(Action::GoTo(dromon_core::RemotePath::new("/var")))));
    settle(&mut rt);
    click(&mut rt, "rows/lib");
    properties_key(&mut rt);
    click(&mut rt, "prop-calculate");
    assert_eq!(text_of(&rt, "prop-size-counts"), "0 files, 1 folder");
    assert_eq!(text_of(&rt, "prop-size-note"), "1 folder couldn't be read");
}

#[test]
fn the_chord_reaches_properties_from_the_filter_field() {
    let mut rt = runtime();
    click(&mut rt, "rows/index.html");
    rt.update(|s, _| drop(s.act(Action::OpenFilter)));
    settle(&mut rt);
    assert!(rt.state.text_focused());
    properties_key(&mut rt);
    assert!(shown(&rt, "properties"), "the field hands the chord on");
    assert_eq!(text_of(&rt, "sheet-title"), "index.html");
}
