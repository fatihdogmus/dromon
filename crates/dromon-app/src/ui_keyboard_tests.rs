//! Keyboard reachability (§6.9): Tab and Shift+Tab walk every control of each main state once
//! per cycle, in reading order, with the focus ring drawn on each; popovers, menus and sheets
//! keep Tab inside until they close, and Escape hands focus back to what opened them. Inside a
//! composite the arrows move (hosts, tabs, menu items, the list's rows), Return or Space acts.

use rune::{ElementInfo, Key, KeyEvent, KeyPlatform, Mods, Runtime};

use crate::scenes;
use crate::state::{Action, App, FileAction, MenuKind};
use crate::state::{Page, SettingsSection};
use crate::theme::{self, Access, Appearance};
use crate::ui_a11y_tests::{
    ITEMS, Node, interactive, needs_sign_in, scene, scene_runtime, settings_browsing, settle, tree,
};

const SHIFT: Mods = Mods { shift: true, cmd: false, alt: false, ctrl: false, logo: false };

fn press<S: 'static>(rt: &mut Runtime<S>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
    rt.frame(0.5);
}

fn tab<S: 'static>(rt: &mut Runtime<S>, back: bool) {
    press(rt, Key::Tab, if back { SHIFT } else { Mods::default() });
}

fn focused<S: 'static>(rt: &Runtime<S>) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.focused && !e.exiting)
}

fn focused_key<S: 'static>(rt: &Runtime<S>) -> String {
    focused(rt).map(|e| e.key.unwrap_or(e.path)).unwrap_or_default()
}

/// Fields that show focus by their caret (and, for these, by being the whole of what's open:
/// the palette's query, the inline name field's accent box).
const CARET_ONLY: &[&str] = &["palette-input", "edit-input"];

/// Whether the focused element shows it: a 2 pt ring in the focus colour drawn on it or inside
/// it (a row's), a menu item's highlight, or for [`CARET_ONLY`] fields the caret.
fn shows_focus<S: 'static>(rt: &Runtime<S>, dark: bool) -> bool {
    let Some(e) = focused(rt) else { return false };
    if e.hover > 0.99 && matches!(e.role, "menuitem" | "menuitemcheckbox") {
        return true;
    }
    if e.key.as_deref().is_some_and(|k| CARET_ONLY.contains(&k)) {
        return e.caret.is_some();
    }
    // The draw list holds sRGB-encoded, premultiplied colours; tokens are linear.
    let c = theme::tokens_for(dark, Access::default()).focus;
    let enc = |x: f32| if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    let want = [enc(c.r) * c.a, enc(c.g) * c.a, enc(c.b) * c.a, c.a];
    let [x, y, w, h] = e.rect;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    rt.instances.iter().any(|i| {
        let ring = i.params[1] == rune::render::KIND_RECT && i.fill == [0.0; 4] && (i.params[0] - 2.0).abs() < 0.01;
        let colour = i.border.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.02);
        let [rx, ry, rw, rh] = i.rect;
        let (rcx, rcy) = (rx + rw / 2.0, ry + rh / 2.0);
        // On the element (around it or around its field's box), or inside it (a row of the rows).
        let around = cx >= rx && cx <= rx + rw && cy >= ry && cy <= ry + rh && rw <= w + 40.0 && rh <= h + 16.0;
        let inside = rcx >= x && rcx <= x + w && rcy >= y && rcy <= y + h;
        ring && colour && (around || inside)
    })
}

/// One Tab cycle from where focus is: the keys focus visits, each once, until it comes back
/// round to the first. Checks the ring at each stop.
fn cycle<S: 'static>(rt: &mut Runtime<S>, back: bool, dark: bool, scene: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..200 {
        tab(rt, back);
        let k = focused_key(rt);
        assert!(!k.is_empty(), "{scene}: Tab left nothing focused after {seen:?}");
        if seen.first() == Some(&k) {
            return seen;
        }
        assert!(!seen.contains(&k), "{scene}: {k} visited twice in one cycle: {seen:?}");
        assert!(shows_focus(rt, dark), "{scene}: no focus ring on {k} ({:?})", focused(rt).map(|e| (e.role, e.rect)));
        seen.push(k);
    }
    panic!("{scene}: Tab never came round: {seen:?}");
}

/// Composite containers: the keyboard reaches their items through one stop.
const COMPOSITES: &[&str] = &["list-box", "grid", "row-group", "menu", "tab-list", "radio-group"];

/// The interactive, enabled nodes of `nodes` that no Tab stop in `visited` reaches: neither
/// visited, nor an item of a composite that one of them is in or is.
fn unreached(nodes: &[Node], visited: &[String]) -> Vec<String> {
    let inside = |i: usize, root: usize| {
        let mut p = Some(i);
        while let Some(j) = p {
            if j == root {
                return true;
            }
            p = nodes[j].parent;
        }
        false
    };
    let visited_ix: Vec<usize> =
        nodes.iter().enumerate().filter(|(_, n)| visited.contains(&n.key)).map(|(i, _)| i).collect();
    nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| interactive(n) && !n.has_state("disabled") && !n.key.is_empty() && !visited.contains(&n.key))
        .filter(|(i, n)| {
            // Its composite: the nearest one around it.
            let mut p = nodes[*i].parent;
            while let Some(j) = p {
                if COMPOSITES.contains(&nodes[j].role.as_str()) {
                    let controlled = nodes.iter().any(|f| {
                        visited.contains(&f.key)
                            && f.raw["controls"]
                                .as_array()
                                .is_some_and(|c| c.iter().any(|x| x.as_str() == Some(nodes[j].key.as_str())))
                    });
                    return !(ITEMS.contains(&n.role.as_str())
                        && (visited_ix.iter().any(|&v| inside(v, j)) || controlled));
                }
                p = nodes[j].parent;
            }
            true
        })
        .map(|(_, n)| n.key.clone())
        .collect()
}

/// Walks Tab and Shift+Tab through `scene`: each control once a cycle, in the same order both
/// ways, each with its ring, none left out, none outside `trap` when a modal is up.
fn walk(scene: &str, start: fn(&mut App), trap: Option<&str>) -> Vec<String> {
    let mut rt = scene_runtime(start);
    let dark = rt.state.ui.appearance != Appearance::Light;
    let nodes = tree(&rt);
    let forward = cycle(&mut rt, false, dark, scene);
    // Shift+Tab from the last stop walks the same stops the other way.
    let mut backward = cycle(&mut rt, true, dark, scene);
    backward.reverse();
    let rotate = |v: &[String], first: &str| -> Vec<String> {
        let at = v.iter().position(|k| k == first).unwrap_or(0);
        v[at..].iter().chain(&v[..at]).cloned().collect()
    };
    assert_eq!(rotate(&backward, &forward[0]), forward, "{scene}: Shift+Tab walks the same stops back");
    let missed = unreached(&nodes, &forward);
    assert!(missed.is_empty(), "{scene}: Tab never reaches {missed:?} (visited {forward:?})");
    if let Some(modal) = trap {
        let root =
            rt.elements().into_iter().find(|e| e.key.as_deref() == Some(modal) && !e.exiting).expect("the modal").path;
        for k in &forward {
            let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k.as_str())).unwrap();
            assert!(e.path.starts_with(&root), "{scene}: Tab left {modal} for {k}");
        }
    }
    forward
}

#[test]
fn tab_walks_the_window_in_reading_order() {
    let order = walk("listing", scenes::listing, None);
    let expected = [
        "host-prod",
        "sb-quick-connect",
        "sidebar-settings",
        "tb-upload",
        "crumb-0",
        "crumb-1",
        "crumb-2",
        "tb-filter",
        "tb-transfers",
        "tb-palette",
        "col-Name",
        "col-Size",
        "col-Modified",
        "rows",
        "status-conn",
    ];
    assert_eq!(order, expected);
    // Back and forward join the toolbar once there's history; the filter's scope switch (one
    // stop, on its chosen segment), the field and its clear button replace the Filter button.
    let order = walk("filter", scenes::filter, None);
    let at = order.iter().position(|k| k == "tb-filter-input").expect("the field");
    assert_eq!(order[(at + 1) % order.len()], "tb-filter-clear", "{order:?}");
    assert_eq!(order[(at + order.len() - 1) % order.len()], "tb-scope-folder", "{order:?}");
    assert_eq!(order[(at + order.len() - 2) % order.len()], "crumb-2", "{order:?}");
    let order = walk("welcome", scenes::welcome, None);
    assert_eq!(
        order,
        ["host-prod", "sb-quick-connect", "sidebar-settings", "tb-transfers", "tb-palette", "welcome-quick-connect"]
    );
}

#[test]
fn every_main_state_is_reachable_by_tab() {
    let states: &[scenes::Scene] = &[
        ("connecting", scenes::connecting),
        ("empty-folder", scenes::empty_folder),
        ("permission-denied", scenes::permission_denied),
        ("reconnecting", scenes::reconnecting),
        ("toolbar-ring", scenes::toolbar_ring),
        ("delete-failed", scenes::delete_failed),
        ("folder-vanished", scenes::folder_vanished),
        ("big-folder", scenes::big_folder),
        ("light", scenes::light),
        ("cut-rows", scenes::cut_rows),
    ];
    for (name, start) in states {
        let order = walk(name, *start, None);
        println!("{name}: {}", order.join(" → "));
    }
    // A reconnect that needs the user: the banner's Sign in is a Tab stop, after the toolbar,
    // before the status bar (the dimmed list between them is inert while it reconnects).
    let order = walk("reconnect-sign-in", needs_sign_in, None);
    assert_eq!(
        order,
        [
            "host-prod",
            "sb-quick-connect",
            "sidebar-settings",
            "tb-filter",
            "tb-transfers",
            "tb-palette",
            "banner-sign-in",
            "status-conn"
        ]
    );
    let plain = walk("reconnecting", scenes::reconnecting, None);
    assert!(!plain.iter().any(|k| k == "banner-sign-in"), "a plain reconnect has no Sign in: {plain:?}");
    // A failed delete's toast can be dismissed from the keyboard.
    let order = walk("delete-failed", scenes::delete_failed, None);
    assert!(order.iter().any(|k| k.starts_with("toast-op-") && k.ends_with("-dismiss")), "{order:?}");
}

#[test]
fn popovers_menus_and_sheets_keep_tab_inside() {
    // A scene with a modal up, and the modal's key.
    type Modal = (&'static str, fn(&mut App), &'static str);
    let modals: &[Modal] = &[
        ("transfers", scenes::transfers, "transfers"),
        ("transfers-uploads", scenes::transfers_uploads, "transfers"),
        ("auth-password", scenes::auth_password, "sheet"),
        ("auth-password-error", scenes::auth_password_error, "sheet"),
        ("host-key", scenes::host_key, "sheet"),
        ("confirm", scenes::confirm, "sheet"),
        ("connection-log", scenes::connection_log, "sheet"),
        ("transfer-conflict", scenes::transfer_conflict, "conflict"),
        ("quit-confirm", scenes::quit_confirm, "quit"),
        ("delete-confirm", scenes::delete_confirm, "delete"),
        ("add-to-config", scenes::add_to_config, "add-config"),
        ("quick-connect", scenes::quick_connect, "quick"),
        ("quick-connect-more", scenes::quick_connect_more, "quick"),
        ("quick-connect-error", scenes::quick_connect_error, "quick"),
        ("palette", scenes::palette, "palette"),
        ("palette-path", scenes::palette_path, "palette"),
        ("menu-row", scenes::menu_row, "ctx-menu"),
        ("menu-empty", scenes::menu_empty, "ctx-menu"),
        ("host-menu", scenes::host_menu, "ctx-menu"),
        ("add-menu", scenes::add_menu, "ctx-menu"),
    ];
    for (name, start, modal) in modals {
        let order = walk(name, *start, Some(modal));
        println!("{name}: {}", order.join(" → "));
    }
    // The transfers popover: its tab, then each row's pause, cancel, retry and reveal.
    let order = walk("transfers", scenes::transfers, Some("transfers"));
    for k in ["tr-tab-downloads", "tr-pause-9", "tr-cancel-9", "tr-resume-5", "tr-retry-3", "tr-1", "tr-clear"] {
        assert!(order.iter().any(|o| o == k), "{k} in {order:?}");
    }
}

#[test]
fn the_settings_page_walks_by_tab_too() {
    // The sidebar, Back, the section list (one stop, the chosen section), then its controls;
    // each segmented control is one stop, at its chosen segment.
    let head = ["host-prod", "sb-quick-connect", "sidebar-settings", "settings-back"];
    let order = walk("settings", scene("settings"), None);
    assert_eq!(order[..4], head);
    assert_eq!(order[4..], ["settings-tab-appearance", "theme-system", "density-comfortable"]);
    // Light, from the listing: its connected hosts come first in the sidebar.
    let order = walk("settings-light", scene("settings-light"), None);
    assert_eq!(order[order.len() - 3..], ["settings-tab-appearance", "theme-light", "density-compact"]);
    let order = walk("settings-transfers", scene("settings-transfers"), None);
    assert_eq!(
        order[order.len() - 4..],
        ["settings-tab-transfers", "folder-choose", "transfers-fewer", "transfers-more"]
    );
    let order = walk("settings-browsing", settings_browsing, None);
    assert_eq!(order[4..], ["settings-tab-browsing", "hidden-toggle"]);
}

#[test]
fn the_settings_sections_move_by_arrows_and_tab_goes_on_to_the_controls() {
    let mut rt = scene_runtime(scene("settings"));
    tab_to(&mut rt, "settings-tab-appearance");
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(focused_key(&rt), "settings-tab-transfers");
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Transfers), "the section follows focus");
    assert!(shows_focus(&rt, true), "with its ring");
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Browsing));
    press(&mut rt, Key::Up, Mods::default());
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Transfers));
    // Tab goes on to the section's controls; Space presses them.
    tab(&mut rt, false);
    assert_eq!(focused_key(&rt), "folder-choose");
    tab(&mut rt, false);
    tab(&mut rt, false);
    assert_eq!(focused_key(&rt), "transfers-more");
    press(&mut rt, Key::Text(" ".into()), Mods::default());
    assert_eq!(rt.state.settings.transfers_at_once, 4);
    // Escape from a control leaves the page; the arrows and Return there never reach the list.
    press(&mut rt, Key::Enter, Mods::default());
    press(&mut rt, Key::Down, Mods::default());
    assert!(rt.state.on_settings());
    press(&mut rt, Key::Escape, Mods::default());
    assert_eq!(rt.state.ui.page, Page::Files);
}

#[test]
fn the_gear_takes_the_keyboard() {
    let mut rt = listing();
    tab_to(&mut rt, "sidebar-settings");
    press(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Appearance), "Return presses it");
    assert_eq!(rt.state.conn().unwrap().cwd.as_str(), "/var/www/site", "rather than opening the list's selection");
    tab_to(&mut rt, "settings-back");
    press(&mut rt, Key::Text(" ".into()), Mods::default());
    assert_eq!(rt.state.ui.page, Page::Files, "Space presses Back");
}

/// Focuses `key` by Tab from where focus is.
fn tab_to(rt: &mut Runtime<App>, key: &str) {
    for _ in 0..60 {
        tab(rt, false);
        if focused_key(rt) == key {
            return;
        }
    }
    panic!("Tab never reached {key}");
}

fn listing() -> Runtime<App> {
    scene_runtime(scenes::listing)
}

#[test]
fn the_sidebar_moves_by_arrows_and_return_connects() {
    let mut rt = listing();
    tab_to(&mut rt, "host-prod");
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(focused_key(&rt), "host-nas");
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(focused_key(&rt), "host-staging", "↓ crosses into the next section");
    assert_eq!(rt.state.active.as_ref().map(|h| h.as_str()), Some("prod"), "moving doesn't connect");
    press(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.active.as_ref().map(|h| h.as_str()), Some("staging"), "Return connects");
    assert!(shows_focus(&rt, true), "the host keeps its ring");
    // Space too.
    press(&mut rt, Key::Up, Mods::default());
    press(&mut rt, Key::Text(" ".into()), Mods::default());
    assert_eq!(rt.state.active.as_ref().map(|h| h.as_str()), Some("nas"));
}

#[test]
fn breadcrumbs_and_toolbar_buttons_act_from_the_keyboard() {
    let mut rt = listing();
    tab_to(&mut rt, "crumb-1");
    press(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.conn().unwrap().cwd.as_str(), "/var");
    // Return presses a focused toolbar button, rather than opening the list's selection.
    let mut rt = listing();
    tab_to(&mut rt, "tb-palette");
    press(&mut rt, Key::Enter, Mods::default());
    assert!(rt.state.ui.palette.is_open());
    assert_eq!(rt.state.conn().unwrap().cwd.as_str(), "/var/www/site");
}

#[test]
fn column_headers_sort_on_space_and_return() {
    use dromon_core::{SortDir, SortKey};
    let mut rt = listing();
    tab_to(&mut rt, "col-Size");
    press(&mut rt, Key::Text(" ".into()), Mods::default());
    let sort = rt.state.conn().unwrap().sort;
    assert_eq!((sort.key, sort.dir), (SortKey::Size, SortDir::Descending));
    press(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.conn().unwrap().sort.dir, SortDir::Ascending);
    assert_eq!(focused_key(&rt), "col-Size", "the header keeps focus");
}

#[test]
fn the_rows_take_arrows_typing_and_return() {
    let mut rt = listing();
    tab_to(&mut rt, "rows");
    let names = |rt: &Runtime<App>| rt.state.conn().unwrap().selection.iter().map(str::to_owned).collect::<Vec<_>>();
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(names(&rt), ["package-lock.json"]);
    assert_eq!(focused_key(&rt), "rows", "the rows keep the keyboard");
    let nodes = tree(&rt);
    let rows = nodes.iter().find(|n| n.key == "rows").unwrap();
    assert_eq!(rows.raw["activeDescendant"].as_str(), Some("package-lock.json"), "a reader follows the cursor");
    press(&mut rt, Key::Text("rob".into()), Mods::default());
    assert_eq!(names(&rt), ["robots.txt"], "type-to-select works with the rows focused");
    press(&mut rt, Key::Home, Mods::default());
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(names(&rt), ["assets"]);
    press(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.conn().unwrap().cwd.as_str(), "/var/www/site/assets");
}

/// Opens with `open` (keys from `trigger` focused), checks focus went inside `modal`, then
/// Escape closes it and focus is back on `trigger`.
fn round_trip(start: fn(&mut App), trigger: &str, open: Key, modal: &str) {
    let mut rt = scene_runtime(start);
    tab_to(&mut rt, trigger);
    press(&mut rt, open, Mods::default());
    settle(&mut rt);
    let root = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(modal) && !e.exiting)
        .unwrap_or_else(|| panic!("{modal} opens"))
        .path;
    let inside = focused(&rt).unwrap_or_else(|| panic!("{modal}: nothing focused"));
    assert!(inside.path.starts_with(&root), "{modal}: focus went inside ({:?})", inside.key);
    press(&mut rt, Key::Escape, Mods::default());
    settle(&mut rt);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some(modal) || e.exiting), "{modal} closes on Escape");
    assert_eq!(focused_key(&rt), trigger, "{modal}: focus is back on what opened it");
    assert!(shows_focus(&rt, true), "{modal}: with its ring");
}

#[test]
fn escape_closes_and_focus_returns_to_the_opener() {
    let space = || Key::Text(" ".into());
    round_trip(scenes::toolbar_ring, "tb-transfers", space(), "transfers");
    round_trip(scenes::listing, "sb-quick-connect", Key::Enter, "quick");
    round_trip(scenes::listing, "tb-palette", Key::Enter, "palette");
    round_trip(scenes::listing, "status-conn", space(), "sheet");
    // A sheet the list's keys opened gives the keyboard back to the rows.
    let delete = if KeyPlatform::current() == KeyPlatform::Mac { Key::Backspace } else { Key::Delete };
    let mut rt = listing();
    tab_to(&mut rt, "rows");
    let mods = if KeyPlatform::current() == KeyPlatform::Mac {
        Mods { cmd: true, logo: true, ..Mods::default() }
    } else {
        Mods::default()
    };
    press(&mut rt, delete, mods);
    assert!(rt.state.ui.sheets.is_open(), "the delete sheet");
    press(&mut rt, Key::Escape, Mods::default());
    settle(&mut rt);
    assert!(!rt.state.ui.sheets.is_open());
    assert_eq!(focused_key(&rt), "rows");
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// The pointer's way stays as it was: a click presses a control without taking the keyboard
/// from the list, so typing still selects rows.
#[test]
fn clicks_leave_the_keyboard_where_it_was() {
    let mut rt = listing();
    click(&mut rt, "tb-transfers");
    assert!(rt.state.transfers.open);
    press(&mut rt, Key::Escape, Mods::default());
    settle(&mut rt);
    assert!(!rt.state.transfers.open);
    assert!(focused(&rt).is_none_or(|e| e.key.as_deref() == Some("rows")), "{:?}", focused(&rt).map(|e| e.key));
    press(&mut rt, Key::Text("rob".into()), Mods::default());
    assert_eq!(rt.state.conn().unwrap().selection.iter().collect::<Vec<_>>(), ["robots.txt"]);
    // A click on a row gives the rows the keyboard, without a ring.
    click(&mut rt, "rows/assets");
    assert_eq!(focused_key(&rt), "rows");
    assert!(!focused(&rt).unwrap().focus_visible, "no ring for a click");
    press(&mut rt, Key::Text("ser".into()), Mods::default());
    assert_eq!(rt.state.conn().unwrap().selection.iter().collect::<Vec<_>>(), ["server.log"]);
}

#[test]
fn the_drawn_menu_moves_by_arrows_and_return_picks() {
    let mut rt = listing();
    rt.update(|s, _| {
        drop(s.act(Action::File(FileAction::OpenMenu {
            kind: MenuKind::Row("index.html".into()),
            at: Some((560.0, 330.0)),
        })))
    });
    settle(&mut rt);
    press(&mut rt, Key::Down, Mods::default());
    let first = focused_key(&rt);
    assert_eq!(first, "ctx-open", "↓ highlights the first item");
    press(&mut rt, Key::Down, Mods::default());
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(focused_key(&rt), "ctx-terminal");
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(focused_key(&rt), "ctx-copy", "the separator is skipped");
    assert!(shows_focus(&rt, true), "the item is highlighted");
    press(&mut rt, Key::Enter, Mods::default());
    assert!(rt.state.files.menu.is_none(), "Return picks and closes");
    assert!(rt.state.clip().is_some(), "Copy ran");
}

/// The add button is a menu button: Space or Return opens its menu on its first item, ↓ walks the
/// items, Escape closes it and the keyboard is back on the button, with its ring.
#[test]
fn the_add_menu_opens_from_the_keyboard_and_escape_returns_to_the_button() {
    for open in [Key::Text(" ".into()), Key::Enter] {
        let mut rt = listing();
        tab_to(&mut rt, "tb-upload");
        press(&mut rt, open.clone(), Mods::default());
        settle(&mut rt);
        assert_eq!(rt.state.files.menu.as_ref().map(|m| &m.kind), Some(&MenuKind::Add), "{open:?} opens it");
        assert_eq!(rt.state.conn().unwrap().cwd.as_str(), "/var/www/site", "{open:?} doesn't reach the list");
        assert_eq!(focused_key(&rt), "ctx-upload-files", "{open:?}: the keyboard is on the first item");
        press(&mut rt, Key::Down, Mods::default());
        assert_eq!(focused_key(&rt), "ctx-upload-folder");
        press(&mut rt, Key::Down, Mods::default());
        assert_eq!(focused_key(&rt), "ctx-new-folder");
        assert!(shows_focus(&rt, true), "the item is highlighted");
        press(&mut rt, Key::Escape, Mods::default());
        settle(&mut rt);
        assert!(rt.state.files.menu.is_none(), "Escape closes it");
        assert_eq!(focused_key(&rt), "tb-upload", "focus is back on the button");
        assert!(shows_focus(&rt, true), "with its ring");
    }
}

#[test]
fn the_transfers_tabs_switch_by_arrows() {
    use crate::state::Tab;
    let mut rt = scene_runtime(scenes::transfers);
    tab_to(&mut rt, "tr-tab-downloads");
    press(&mut rt, Key::Right, Mods::default());
    assert_eq!(focused_key(&rt), "tr-tab-uploads");
    assert_eq!(rt.state.transfers.tab, Tab::Uploads, "a tab follows focus");
}
