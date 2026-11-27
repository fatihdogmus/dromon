//! The context menus' entries, which are enabled when, and what their items do.

use dromon_core::{ConnStatus, HostId, RemotePath};

use super::*;
use crate::fixture;
use crate::state::settings::Terminal;
use crate::state::{Action, App, PaletteAction, ServiceEvent, SettingsChange, Shortcut};

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
            ("terminal", true),
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
    for id in ["open", "download", "terminal", "paste", "new-folder", "rename", "delete"] {
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
    assert_eq!(
        shape(&s, &empty),
        [("new-folder", true), ("paste", false), ("—", false), ("terminal", true), ("refresh", true), ("hidden", true)]
    );
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
    assert_eq!(
        shape(&s, &empty),
        [
            ("new-folder", false),
            ("paste", false),
            ("—", false),
            ("terminal", false),
            ("refresh", false),
            ("hidden", true)
        ]
    );
}

#[test]
fn the_add_menu_uploads_files_or_a_folder_or_makes_a_folder() {
    let mut s = demo();
    let add = MenuKind::Add;
    assert_eq!(shape(&s, &add), [("upload-files", true), ("upload-folder", true), ("new-folder", true)]);
    let items: Vec<MenuItem> = s.menu(&add).into_iter().filter_map(|e| e.item().cloned()).collect();
    let shown: Vec<_> = items.iter().map(|i| (i.title, i.shortcut)).collect();
    assert_eq!(
        shown,
        [("Upload files…", Some("mod+u")), ("Upload folder…", None), ("New folder", Some("mod+shift+n"))],
        "the same chords as ⌘U and ⌘⇧N"
    );
    assert_eq!(s.menu_action(&add, "upload-files"), Some(Action::PickUpload));
    assert_eq!(s.menu_action(&add, "upload-folder"), Some(Action::PickUploadFolder));
    assert_eq!(s.menu_action(&add, "new-folder"), Some(Action::File(FileAction::NewFolder)));
    // Picking runs it, as ⌘U and the palette do, and closes the drawn menu.
    s.act(Action::File(FileAction::OpenMenu { kind: add.clone(), at: Some((120.0, 46.0)) }));
    let fx = s.act(Action::File(FileAction::Pick { kind: add.clone(), id: "upload-files".into() }));
    assert_eq!(fx, s.act(Action::PickUpload));
    assert_eq!(fx, [Effect::PickUpload { folder: "site".into() }]);
    assert!(s.files.menu.is_none());
    let fx = s.act(Action::File(FileAction::Pick { kind: add.clone(), id: "upload-folder".into() }));
    assert_eq!(fx, [Effect::PickUploadFolder { folder: "site".into() }]);
    s.act(Action::File(FileAction::Pick { kind: add.clone(), id: "new-folder".into() }));
    assert!(s.files.edit.field().is_some(), "the new folder's field");
    // Reconnecting: all three are off and do nothing.
    s.act(Action::File(FileAction::EditCancel));
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    assert_eq!(shape(&s, &add), [("upload-files", false), ("upload-folder", false), ("new-folder", false)]);
    for id in ["upload-files", "upload-folder", "new-folder"] {
        assert_eq!(s.menu_action(&add, id), None, "{id}");
    }
    // No host on screen: off too.
    let welcome = fixture::app(fixture::demo());
    assert!(shape(&welcome, &add).iter().all(|(_, on)| !on));
}

/// The effect every "Open in Terminal" asks for, on prod (an ssh-config host) in `folder`.
fn terminal_at(s: &App, folder: &str) -> Vec<Effect> {
    vec![Effect::OpenTerminal {
        window: s.window,
        terminal: s.settings.terminal,
        host: s.hosts.get(&prod()).unwrap().clone(),
        folder: RemotePath::new(folder),
    }]
}

#[test]
fn open_in_terminal_opens_the_right_folder_from_every_entry_point() {
    let mut s = demo();
    let site = "/var/www/site";
    // A folder row's menu: that folder; a file row's: the folder on screen; the empty area's too.
    let pick = |s: &mut App, kind: MenuKind| s.act(Action::File(FileAction::Pick { kind, id: "terminal".into() }));
    assert_eq!(pick(&mut s, MenuKind::Row("assets".into())), terminal_at(&s, "/var/www/site/assets"));
    assert_eq!(pick(&mut s, MenuKind::Row("index.html".into())), terminal_at(&s, site));
    assert_eq!(pick(&mut s, MenuKind::Empty), terminal_at(&s, site));
    // ⌘⇧T: the cursor row if it's a folder, else the folder on screen.
    s.act(Action::Click { name: "assets".into(), extend: false, toggle: false });
    assert_eq!(s.shortcut(Shortcut::OpenTerminal, 0.0), terminal_at(&s, "/var/www/site/assets"));
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    assert_eq!(s.shortcut(Shortcut::OpenTerminal, 0.0), terminal_at(&s, site));
    s.act(Action::Escape);
    assert_eq!(s.conn().unwrap().selection.cursor(), None);
    assert_eq!(s.shortcut(Shortcut::OpenTerminal, 0.0), terminal_at(&s, site));
    // The palette's row, as the shortcut.
    s.act(Action::Click { name: "assets".into(), extend: false, toggle: false });
    s.act(Action::Palette(PaletteAction::Toggle));
    s.ui.palette.edit = rune::TextEdit::new("terminal");
    s.act(Action::Palette(PaletteAction::Edited));
    let row = s.palette_items().into_iter().find(|i| i.key == "pal-terminal").unwrap();
    assert_eq!((row.title.as_str(), row.shortcut), ("Open in Terminal", Some("mod+shift+t")));
    let fx = s.act(Action::Palette(PaletteAction::Pick("pal-terminal".into())));
    assert_eq!(fx, [vec![Effect::Blur], terminal_at(&s, "/var/www/site/assets")].concat());
    assert!(!s.ui.palette.is_open());
    // The terminal Settings chose goes along.
    s.act(Action::Setting(SettingsChange::Terminal(Terminal::Kitty)));
    assert!(matches!(&pick(&mut s, MenuKind::Empty)[..], [Effect::OpenTerminal { terminal: Terminal::Kitty, .. }]));
    // The menu items show the shortcut that does the same.
    let item = s.menu(&MenuKind::Empty).into_iter().filter_map(|e| e.item().cloned()).find(|i| i.id == "terminal");
    assert_eq!(item.map(|i| (i.title, i.shortcut)), Some(("Open in Terminal", Some("mod+shift+t"))));
}

#[test]
fn open_in_terminal_does_nothing_while_the_host_isnt_connected() {
    let mut s = demo();
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    assert!(s.act(Action::OpenTerminal(None)).is_empty());
    assert!(s.act(Action::OpenTerminal(Some("assets".into()))).is_empty());
    assert!(s.shortcut(Shortcut::OpenTerminal, 0.0).is_empty());
    assert_eq!(s.menu_action(&MenuKind::Empty, "terminal"), None, "disabled");
    s.act(Action::Palette(PaletteAction::Toggle));
    s.ui.palette.edit = rune::TextEdit::new("terminal");
    s.act(Action::Palette(PaletteAction::Edited));
    assert!(!s.palette_items().iter().any(|i| i.key == "pal-terminal"), "not listed");
    // No host on screen.
    let mut welcome = fixture::app(fixture::demo());
    assert!(welcome.shortcut(Shortcut::OpenTerminal, 0.0).is_empty());
}

#[test]
fn a_terminal_that_couldnt_open_says_so_in_a_toast() {
    let mut s = demo();
    s.act(Action::TerminalFailed("kitty isn't installed".into()));
    let toasts = s.toasts();
    assert_eq!(toasts.len(), 1);
    assert_eq!(toasts[0].text, "Couldn't open a terminal: kitty isn't installed");
    assert!(toasts[0].dismissible());
}

#[test]
fn a_native_menu_only_selects_the_row() {
    let mut s = demo();
    s.act(Action::File(FileAction::OpenMenu { kind: MenuKind::Row("README.md".into()), at: None }));
    assert!(s.files.menu.is_none(), "the platform shows it");
    assert!(s.conn().unwrap().selection.contains("README.md"));
}
