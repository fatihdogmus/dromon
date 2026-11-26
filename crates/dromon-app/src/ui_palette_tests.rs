//! The ⌘K palette, ⌘L, ⌘1–9 and ⌘⇧J driven headless through the real view and `on_key`, on the
//! demo fixture: keys, typing, clicks, focus and motion.

use dromon_core::{ConnStatus, HostId, RemotePath};
use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};

use crate::state::App;
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

fn runtime(reduced: bool) -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.set_system_reduced_motion(reduced);
    rt.update(|s, _| *s = scenes::base());
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// ⌘ on macOS, Ctrl elsewhere, as the platform reports the primary modifier.
fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

fn chord(rt: &mut Runtime<App>, c: char, shift: bool) {
    rt.key(KeyEvent { key: Key::Command(c), mods: Mods { shift, ..primary() } });
    settle(rt);
}

fn key(rt: &mut Runtime<App>, k: Key) {
    rt.key(KeyEvent { key: k, mods: Mods::default() });
    settle(rt);
}

fn type_text(rt: &mut Runtime<App>, s: &str) {
    key(rt, Key::Text(s.into()));
}

fn element(rt: &Runtime<App>, k: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

fn shown(rt: &Runtime<App>, k: &str) -> bool {
    element(rt, k).is_some()
}

/// The palette's rows on screen, in order (the sidebar's hosts are options too).
fn rows(rt: &Runtime<App>) -> Vec<String> {
    let Some(list) = element(rt, "palette-list") else { return Vec::new() };
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting && e.role == "option" && e.path.starts_with(&list.path))
        .filter_map(|e| e.key)
        .collect()
}

fn cwd(rt: &Runtime<App>) -> String {
    rt.state.conn().map(|c| c.cwd.to_string()).unwrap_or_default()
}

fn on_prod(rt: &mut Runtime<App>) {
    click(rt, "host-prod");
    assert_eq!(cwd(rt), "/var/www/site");
}

#[test]
fn cmd_k_opens_it_focused_and_escape_gives_the_keyboard_back_to_the_list() {
    let mut rt = runtime(false);
    on_prod(&mut rt);
    chord(&mut rt, 'k', false);
    assert!(shown(&rt, "palette"));
    assert_eq!(rt.presented_focus_key(), Some("palette-input"), "the field has the keyboard");
    let panel = element(&rt, "palette").unwrap();
    assert_eq!(panel.role, "dialog");
    assert_eq!(panel.rect[2], 560.0, "560 pt wide");
    assert_eq!(panel.rect[1], (SIZE.1 / 3.0).round(), "its top a third of the way down, at a whole point");
    assert_eq!(
        rows(&rt)[..3],
        ["pal-host-prod", "pal-host-staging", "pal-host-nas"],
        "hosts first, as the sidebar has them"
    );
    assert_eq!(element(&rt, "pal-host-staging").unwrap().label.as_deref(), Some("staging"));
    // Its shortcut, as this platform writes it.
    let hint = rt
        .elements()
        .into_iter()
        .find(|e| e.path.contains("pal-host-prod/shortcut") && e.text.is_some())
        .and_then(|e| e.text);
    assert_eq!(hint.as_deref(), Some(if cfg!(target_os = "macos") { "⌘1" } else { "Ctrl+1" }));
    key(&mut rt, Key::Escape);
    assert!(!shown(&rt, "palette"));
    assert_eq!(rt.presented_focus_key(), None, "nothing holds the keyboard: the list has it");
    key(&mut rt, Key::Down);
    assert_eq!(rt.state.conn().unwrap().selection.cursor(), Some(".well-known"), "↓ moves the list again");
}

#[test]
fn the_toolbar_hint_opens_it_and_a_click_outside_closes_it() {
    let mut rt = runtime(false);
    click(&mut rt, "tb-palette");
    assert!(shown(&rt, "palette"));
    assert_eq!(rt.presented_focus_key(), Some("palette-input"));
    click(&mut rt, "sb-quick-connect");
    assert!(!shown(&rt, "palette"), "a press outside closes it");
    assert!(!rt.state.ui.quick.open, "and goes no further");
}

#[test]
fn typing_a_host_and_return_connects_it() {
    let mut rt = runtime(false);
    chord(&mut rt, 'k', false);
    type_text(&mut rt, "stag");
    assert_eq!(rows(&rt), ["pal-host-staging"]);
    key(&mut rt, Key::Enter);
    assert!(!shown(&rt, "palette"));
    assert_eq!(rt.state.active, Some(HostId("staging".into())));
    assert_eq!(rt.state.conn().unwrap().status, ConnStatus::Ready);
    assert_eq!(rt.presented_focus_key(), None);
}

#[test]
fn arrows_move_the_cursor_and_a_click_runs_a_row() {
    let mut rt = runtime(false);
    chord(&mut rt, 'k', false);
    key(&mut rt, Key::Down);
    key(&mut rt, Key::Down);
    assert_eq!(rt.state.ui.palette.cursor, 2);
    assert_eq!(rt.presented_focus_key(), Some("palette-input"), "the keyboard stays in the field");
    key(&mut rt, Key::Up);
    assert_eq!(rt.state.ui.palette.cursor, 1);
    click(&mut rt, "pal-host-nas");
    assert_eq!(rt.state.active, Some(HostId("nas".into())));
    assert!(!shown(&rt, "palette"));
}

#[test]
fn an_action_runs_and_takes_the_keyboard_it_needs() {
    let mut rt = runtime(false);
    on_prod(&mut rt);
    chord(&mut rt, 'k', false);
    type_text(&mut rt, "new f");
    assert_eq!(rows(&rt)[0], "pal-new-folder");
    key(&mut rt, Key::Enter);
    assert!(!shown(&rt, "palette"));
    assert!(shown(&rt, "new-folder"), "the new folder's row");
    assert_eq!(rt.presented_focus_key(), Some("edit-input"), "its name field has the keyboard");
    key(&mut rt, Key::Escape);
    // From the filter field too: ⌘K works there, and Filter gives the keyboard back to it.
    chord(&mut rt, 'f', false);
    assert_eq!(rt.presented_focus_key(), Some("tb-filter-input"));
    chord(&mut rt, 'k', false);
    assert_eq!(rt.presented_focus_key(), Some("palette-input"), "⌘K from the filter field");
    type_text(&mut rt, "filter");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.presented_focus_key(), Some("tb-filter-input"));
}

#[test]
fn cmd_l_types_a_path_and_return_goes_there() {
    let mut rt = runtime(false);
    on_prod(&mut rt);
    chord(&mut rt, 'l', false);
    assert!(shown(&rt, "palette"));
    assert_eq!(element(&rt, "palette").unwrap().label.as_deref(), Some("Go to folder"));
    assert_eq!(rt.state.ui.palette.edit.text, "/var/www/site");
    assert_eq!(rt.presented_focus_key(), Some("palette-input"));
    // The path is all selected: typing replaces it.
    type_text(&mut rt, "releases");
    assert_eq!(rt.state.ui.palette.edit.text, "releases");
    assert_eq!(element(&rt, "pal-go").and_then(|e| e.label).as_deref(), Some("Go to /var/www/site/releases"));
    key(&mut rt, Key::Enter);
    assert!(!shown(&rt, "palette"));
    assert_eq!(cwd(&rt), "/var/www/site/releases");
    assert!(rt.find("rows/20261125-1348").is_some());
    // ~ is the session's home; a folder that isn't there shows its state.
    chord(&mut rt, 'l', false);
    type_text(&mut rt, "~/nope");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.conn().unwrap().cwd, RemotePath::new("/var/www/site/nope"));
    assert!(shown(&rt, "message") && shown(&rt, "state-go-up"), "the not-found state");
    assert!(rt.elements().iter().any(|e| !e.exiting && e.text.as_deref() == Some("This folder no longer exists")));
}

#[test]
fn cmd_digits_switch_hosts_and_cmd_shift_j_shows_transfers() {
    let mut rt = runtime(false);
    chord(&mut rt, '3', false);
    assert_eq!(rt.state.active, Some(HostId("nas".into())), "⌘3: the sidebar's third host");
    chord(&mut rt, '2', false);
    assert_eq!(rt.state.active, Some(HostId("prod".into())), "nas went up to Connected: prod is second");
    chord(&mut rt, 'j', true);
    assert!(shown(&rt, "transfers"), "⌘⇧J shows the transfers popover");
    chord(&mut rt, 'j', true);
    assert!(!shown(&rt, "transfers"), "and hides it");
    // Not from a text field.
    chord(&mut rt, 'f', false);
    chord(&mut rt, '1', false);
    chord(&mut rt, 'l', false);
    assert_eq!(rt.state.active, Some(HostId("prod".into())));
    assert!(!shown(&rt, "palette"));
}

#[test]
fn it_scales_in_from_its_centre_or_only_fades_under_reduce_motion() {
    for reduced in [false, true] {
        let mut rt = runtime(reduced);
        rt.key(KeyEvent { key: Key::Command('k'), mods: primary() });
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        let early = element(&rt, "palette").expect("the palette");
        assert!(early.opacity < 1.0, "reduced {reduced}: fading in ({})", early.opacity);
        if reduced {
            assert_eq!(early.scale, 1.0, "no scale under Reduce Motion");
        } else {
            assert!(early.scale < 1.0 && early.scale >= 0.96, "scaling in: {}", early.scale);
        }
        settle(&mut rt);
        let end = element(&rt, "palette").unwrap();
        assert_eq!((end.opacity, end.scale), (1.0, 1.0), "reduced {reduced}");
    }
}
