//! The real app driven headless (Rune's `Runtime`): pointer and keys in, state and elements
//! out. Same view, handlers and `on_key` as the window, on the demo fixture and its clock.

use dromon_core::{HostId, SortDir, SortKey};
use rune::{Env, Key, KeyEvent, Mods, PhysicalInput, Runtime};

use crate::state::App;
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

fn runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| *s = scenes::base());
    rt.frame(0.0);
    rt
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn double_click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.05);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn hold(rt: &mut Runtime<App>, mods: Mods) {
    rt.physical_input(PhysicalInput::Modifiers(mods));
}

fn key(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

/// ⌘ on macOS, Ctrl elsewhere, as the platform reports the primary modifier.
fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

fn cwd(rt: &Runtime<App>) -> String {
    rt.state.conn().map(|c| c.cwd.to_string()).unwrap_or_default()
}

fn selected(rt: &Runtime<App>) -> Vec<String> {
    let c = rt.state.conn().unwrap();
    c.row_names().into_iter().filter(|n| c.selection.contains(n)).map(str::to_owned).collect()
}

fn text_of(rt: &Runtime<App>, key: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text).unwrap_or_default()
}

fn connect_prod(rt: &mut Runtime<App>) {
    click(rt, "host-prod");
    assert_eq!(rt.state.active, Some(HostId("prod".into())));
    assert_eq!(cwd(rt), "/var/www/site");
}

#[test]
fn welcome_then_clicking_a_host_lists_it() {
    let mut rt = runtime();
    assert!(rt.find("welcome").is_some());
    connect_prod(&mut rt);
    assert!(rt.find("welcome").is_none());
    assert!(rt.find("rows/index.html").is_some());
    assert_eq!(text_of(&rt, "status-summary"), "17 items");
}

#[test]
fn double_click_opens_a_folder_and_back_returns() {
    let mut rt = runtime();
    connect_prod(&mut rt);
    double_click(&mut rt, "rows/releases");
    assert_eq!(cwd(&rt), "/var/www/site/releases");
    assert!(rt.find("rows/20261125-1348").is_some());

    click(&mut rt, "tb-back");
    assert_eq!(cwd(&rt), "/var/www/site");
    click(&mut rt, "tb-forward");
    assert_eq!(cwd(&rt), "/var/www/site/releases");

    // ⌘[ and ⌘] (Ctrl elsewhere) do the same from the keyboard, and ⌘↑ goes up.
    key(&mut rt, Key::Command('['), primary());
    assert_eq!(cwd(&rt), "/var/www/site");
    key(&mut rt, Key::Command(']'), primary());
    assert_eq!(cwd(&rt), "/var/www/site/releases");
    key(&mut rt, Key::Up, primary());
    assert_eq!(cwd(&rt), "/var/www/site");
    assert_eq!(selected(&rt), ["releases"], "up selects the folder it came out of");
}

#[test]
fn return_and_breadcrumbs_navigate() {
    let mut rt = runtime();
    connect_prod(&mut rt);
    click(&mut rt, "rows/assets");
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(cwd(&rt), "/var/www/site/assets");
    // Crumbs: host (/), var, www, site, assets.
    click(&mut rt, "crumb-2");
    assert_eq!(cwd(&rt), "/var/www");
    click(&mut rt, "crumb-0");
    assert_eq!(cwd(&rt), "/");
}

#[test]
fn header_clicks_toggle_the_sort() {
    let mut rt = runtime();
    connect_prod(&mut rt);
    click(&mut rt, "col-Size");
    let c = rt.state.conn().unwrap();
    assert_eq!((c.sort.key, c.sort.dir), (SortKey::Size, SortDir::Descending));
    let names = c.row_names();
    let first_file = names.iter().position(|n| !c.find(n).unwrap().kind.is_dir_like()).unwrap();
    assert_eq!(names[first_file], "backup-2026-11-01.tar.gz", "largest file first, after the folders");

    click(&mut rt, "col-Size");
    let c = rt.state.conn().unwrap();
    assert_eq!(c.sort.dir, SortDir::Ascending);
    assert_eq!(c.row_names().last().copied(), Some("backup-2026-11-01.tar.gz"));

    click(&mut rt, "col-Name");
    assert_eq!(rt.state.conn().unwrap().sort.key, SortKey::Name);
}

#[test]
fn shift_click_selects_a_range_and_cmd_click_toggles() {
    let mut rt = runtime();
    connect_prod(&mut rt);
    click(&mut rt, "rows/app.min.js");
    assert_eq!(selected(&rt), ["app.min.js"]);

    hold(&mut rt, Mods { shift: true, ..Mods::default() });
    click(&mut rt, "rows/index.html");
    assert_eq!(selected(&rt), ["app.min.js", "backup-2026-11-01.tar.gz", "favicon.ico", "index.html"]);

    hold(&mut rt, primary());
    click(&mut rt, "rows/favicon.ico");
    assert_eq!(selected(&rt), ["app.min.js", "backup-2026-11-01.tar.gz", "index.html"]);
    assert_eq!(text_of(&rt, "status-summary"), "17 items · 3 selected, 18.5 MB");

    hold(&mut rt, Mods::default());
    click(&mut rt, "rows/server.log");
    assert_eq!(selected(&rt), ["server.log"]);
}

#[test]
fn arrows_and_type_to_select() {
    let mut rt = runtime();
    connect_prod(&mut rt);
    key(&mut rt, Key::Down, Mods::default());
    assert_eq!(selected(&rt), [".well-known"]);
    key(&mut rt, Key::Down, Mods { shift: true, ..Mods::default() });
    assert_eq!(selected(&rt), [".well-known", "assets"]);
    key(&mut rt, Key::Text("p".into()), Mods::default());
    assert_eq!(selected(&rt), ["package-lock.json"]);
}

#[test]
fn hidden_files_toggle_from_the_keyboard() {
    let mut rt = runtime();
    connect_prod(&mut rt);
    assert!(rt.find("rows/.env").is_some());
    key(&mut rt, Key::Command('.'), Mods { shift: true, ..primary() });
    assert!(rt.find("rows/.env").is_none());
    assert!(rt.find("rows/.well-known").is_none());
    assert_eq!(text_of(&rt, "status-summary"), "14 items");
    key(&mut rt, Key::Command('.'), Mods { shift: true, ..primary() });
    assert!(rt.find("rows/.env").is_some());
}

#[test]
fn filter_narrows_as_you_type_and_escape_closes_it() {
    let mut rt = runtime();
    connect_prod(&mut rt);
    key(&mut rt, Key::Command('f'), primary());
    rt.frame(1.0 / 60.0);
    assert!(rt.find("tb-filter-input").is_some());
    key(&mut rt, Key::Text("json".into()), Mods::default());
    let c = rt.state.conn().unwrap();
    assert_eq!(c.row_names(), ["package-lock.json", "package.json"]);
    assert_eq!(text_of(&rt, "status-summary"), "2 of 17 items");
    // ↓ still moves the list's selection from inside the field.
    key(&mut rt, Key::Down, Mods::default());
    assert_eq!(selected(&rt), ["package-lock.json"]);
    key(&mut rt, Key::Escape, Mods::default());
    assert!(rt.find("tb-filter-input").is_none());
    assert_eq!(rt.state.conn().unwrap().row_count(), 17);
}

#[test]
fn the_big_folder_mounts_only_a_window_of_rows() {
    let mut rt = runtime();
    rt.update(|s, _| scenes::big_folder(s));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.conn().unwrap().row_count(), 10_000);
    let mounted = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("IMG_"))).count();
    assert!(mounted > 0 && mounted < 400, "mounted {mounted} rows");
    assert!(rt.find("rows/IMG_05000.jpg").is_some(), "scrolled to the middle");
    assert!(rt.find("rows/IMG_00001.jpg").is_none());
}

/// The cursor row moved by the keyboard is scrolled into the list's viewport (the viewport
/// `list` holds the `rows` stack: a reveal scoped to `rows` alone scrolls nothing).
#[test]
fn keys_that_move_the_cursor_scroll_its_row_into_view() {
    let mut rt = runtime();
    rt.update(|s, _| scenes::big_folder(s));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let on_screen = |rt: &Runtime<App>| {
        let c = rt.state.conn().unwrap();
        let cursor = c.selection.cursor().unwrap().to_owned();
        let at = rt.find(&format!("rows/{cursor}"));
        assert!(at.is_some_and(|(_, y)| y > 0.0 && y < SIZE.1), "{cursor} is at {at:?}, outside 0..{}", SIZE.1);
    };
    key(&mut rt, Key::Home, Mods::default());
    rt.frame(0.5);
    on_screen(&rt);
    key(&mut rt, Key::End, Mods::default());
    rt.frame(0.5);
    on_screen(&rt);
    key(&mut rt, Key::Home, Mods::default());
    for _ in 0..40 {
        key(&mut rt, Key::Down, Mods::default());
    }
    rt.frame(0.5);
    on_screen(&rt);
}

#[test]
fn the_welcome_screen_offers_the_host_opened_last_time() {
    let mut rt = runtime();
    assert!(rt.find("welcome-last-host").is_none(), "nothing remembered: no offer");
    rt.update(|s, _| {
        let saved = crate::state::Saved { last_host: Some("prod".into()), ..Default::default() };
        s.remember(saved, Box::new(crate::state::NoStore));
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.active, None, "offered, not connected");
    click(&mut rt, "welcome-last-host");
    assert_eq!(rt.state.active, Some(HostId("prod".into())));
    assert_eq!(cwd(&rt), "/var/www/site");
}

#[test]
fn sidebar_resizes_within_its_bounds() {
    let mut rt = runtime();
    let (x, y) = rt.find("sb-grip").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_move(x + 200.0, y);
    rt.pointer_up(x + 200.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.ui.sidebar_width, 320.0);
    let (x, y) = rt.find("sb-grip").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_move(x - 300.0, y);
    rt.pointer_up(x - 300.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.ui.sidebar_width, 200.0);
}
