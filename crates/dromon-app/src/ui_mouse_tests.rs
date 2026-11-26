//! The mouse's thumb buttons (Back, Forward) walk the main window's folder history, driven
//! headless through the real view's root `on_button`.

use dromon_core::HostId;
use rune::{Env, PointerButton, Runtime};

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

/// Presses and releases a thumb button over the element `key`.
fn thumb(rt: &mut Runtime<App>, key: &str, button: PointerButton) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.button_down(x, y, button);
    rt.button_up(x, y, button);
    rt.frame(1.0 / 60.0);
}

fn cwd(rt: &Runtime<App>) -> String {
    rt.state.conn().map(|c| c.cwd.to_string()).unwrap_or_default()
}

#[test]
fn back_and_forward_buttons_walk_the_folder_history() {
    let mut rt = runtime();
    click(&mut rt, "host-prod");
    assert_eq!(rt.state.active, Some(HostId("prod".into())));
    assert_eq!(cwd(&rt), "/var/www/site");
    double_click(&mut rt, "rows/releases");
    assert_eq!(cwd(&rt), "/var/www/site/releases");
    assert!(rt.find("rows/20261125-1348").is_some());

    // Over the status bar, then over a row: nothing there claims the button.
    thumb(&mut rt, "status-summary", PointerButton::Back);
    assert_eq!(cwd(&rt), "/var/www/site");
    assert!(rt.find("rows/index.html").is_some(), "the parent's rows are on screen");
    thumb(&mut rt, "rows/index.html", PointerButton::Forward);
    assert_eq!(cwd(&rt), "/var/www/site/releases");
    assert!(rt.find("rows/20261125-1348").is_some(), "the subfolder's rows are on screen");

    // At the end of the trail Forward does nothing; other extra buttons do nothing either.
    thumb(&mut rt, "status-summary", PointerButton::Forward);
    assert_eq!(cwd(&rt), "/var/www/site/releases");
    thumb(&mut rt, "status-summary", PointerButton::Other(7));
    assert_eq!(cwd(&rt), "/var/www/site/releases");
}

/// Prod opened one folder down with a step back taken, so Forward has somewhere to go; then the
/// Settings page on top.
fn on_the_page() -> Runtime<App> {
    let mut rt = runtime();
    click(&mut rt, "host-prod");
    double_click(&mut rt, "rows/releases");
    thumb(&mut rt, "status-summary", PointerButton::Back);
    assert_eq!(cwd(&rt), "/var/www/site");
    click(&mut rt, "sidebar-settings");
    assert!(rt.state.on_settings());
    rt
}

#[test]
fn the_back_button_leaves_the_settings_page_and_keeps_the_folder_history() {
    let mut rt = on_the_page();
    let history = rt.state.conn().unwrap().history.clone();
    assert!(history.can_forward(), "the folder view has a step forward to take");

    thumb(&mut rt, "settings-back", PointerButton::Back);
    assert!(!rt.state.on_settings(), "the page is gone");
    assert_eq!(cwd(&rt), "/var/www/site");
    assert_eq!(rt.state.conn().unwrap().history, history);
}

#[test]
fn the_forward_button_does_nothing_on_the_settings_page() {
    let mut rt = on_the_page();
    let history = rt.state.conn().unwrap().history.clone();

    thumb(&mut rt, "settings-back", PointerButton::Forward);
    assert!(rt.state.on_settings());
    assert_eq!(cwd(&rt), "/var/www/site");
    assert_eq!(rt.state.conn().unwrap().history, history);
}
