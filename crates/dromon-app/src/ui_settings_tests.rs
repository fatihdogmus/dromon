//! The Settings window driven headless: its own app and runtime (`settings_window::pane_app`),
//! wired to a main `App` as the real window is (each change goes to the main state as
//! `Action::Setting`), and the main window's side: ⌘, and the list's density.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use dromon_core::HostId;
use rune::{Env, FileDialogKind, Key, KeyEvent, Mods, Runtime};

use crate::settings_window::pane_app;
use crate::state::{Action, App, Density, SettingsChange, SettingsView, SettingsWindow};
use crate::theme::{self, Appearance};
use crate::views::settings::{self, Pane};
use crate::{app, scenes};

struct Rig {
    rt: Runtime<Pane>,
    main: Rc<RefCell<App>>,
}

/// The window on the demo's main state, showing what it says.
fn rig() -> Rig {
    let main = Rc::new(RefCell::new(scenes::base()));
    let to_main = main.clone();
    let mut pane = Pane::new(move |change| {
        to_main.borrow_mut().act(Action::Setting(change));
    });
    pane.home = None;
    pane.view = main.borrow().settings_view();
    let mut rt = pane_app(pane).headless_runtime(settings::SIZE, Env::new(true, 1.0));
    rt.frame(0.0);
    Rig { rt, main }
}

fn click(rt: &mut Runtime<Pane>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// A frame, then one a second later: fills and layout have landed.
fn settle(rt: &mut Runtime<Pane>) {
    rt.frame(1.0 / 60.0);
    rt.frame(1.0);
}

fn text_of(rt: &Runtime<Pane>, key: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text).unwrap_or_default()
}

fn fill_of<S>(rt: &Runtime<S>, key: &str) -> rune::Color {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.fill).unwrap_or_else(|| panic!("no element {key:?}"))
}

fn folder(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

#[test]
fn the_window_shows_every_section_at_the_defaults() {
    let Rig { rt, main } = rig();
    for key in ["section-appearance", "section-transfers", "section-browsing", "theme-system", "density-comfortable", "folder-choose", "transfers-value", "hidden-toggle"] {
        assert!(rt.find(key).is_some(), "{key}");
    }
    assert!(rt.find("folder-reset").is_none(), "nothing to reset at the default");
    assert_eq!(text_of(&rt, "folder-path"), main.borrow().downloads.display().to_string());
    assert_eq!(text_of(&rt, "folder-default"), "Default");
    assert_eq!(text_of(&rt, "transfers-value"), "3");
}

#[test]
fn theme_applies_to_both_windows_at_once() {
    let Rig { mut rt, main } = rig();
    // The system is dark (the runtime's Env): System paints dark.
    assert_eq!(fill_of(&rt, "settings"), theme::tokens(true).surface);
    click(&mut rt, "theme-light");
    assert_eq!(main.borrow().settings.theme, Appearance::Light);
    assert_eq!(main.borrow().ui.appearance, Appearance::Light);
    assert_eq!(fill_of(&rt, "settings"), theme::tokens(false).surface, "this window repaints too");
    click(&mut rt, "theme-system");
    assert_eq!(main.borrow().ui.appearance, Appearance::System);
    assert_eq!(fill_of(&rt, "settings"), theme::tokens(true).surface);
}

#[test]
fn density_segments_set_the_lists_rows() {
    let Rig { mut rt, main } = rig();
    click(&mut rt, "density-compact");
    assert_eq!(main.borrow().settings.density, Density::Compact);
    click(&mut rt, "density-comfortable");
    assert_eq!(main.borrow().settings.density, Density::Comfortable);
}

#[test]
fn choose_picks_a_folder_and_reset_goes_back_to_downloads() {
    let Rig { mut rt, main } = rig();
    click(&mut rt, "folder-choose");
    let (kind, options) = rt.file_dialog().expect("the folder picker is up");
    assert_eq!(kind, FileDialogKind::Folder);
    assert_eq!(options.directory.as_ref(), Some(&main.borrow().downloads), "it starts in the folder in use");
    // Cancelled: nothing changes.
    rt.resolve_file_dialog(None).unwrap();
    settle(&mut rt);
    assert_eq!(main.borrow().settings.download_folder, None);

    click(&mut rt, "folder-choose");
    rt.resolve_file_dialog(Some(folder("picked"))).unwrap();
    settle(&mut rt);
    assert_eq!(main.borrow().settings.download_folder, Some(folder("picked")));
    assert_eq!(main.borrow().download_folder(), folder("picked"));
    assert_eq!(text_of(&rt, "folder-path"), folder("picked").display().to_string());
    assert!(rt.find("folder-default").is_none());

    click(&mut rt, "folder-reset");
    assert_eq!(main.borrow().settings.download_folder, None);
    assert!(rt.find("folder-reset").is_none());
    assert_eq!(text_of(&rt, "folder-default"), "Default");
}

#[test]
fn the_stepper_counts_within_one_to_eight() {
    let Rig { mut rt, main } = rig();
    for _ in 0..7 {
        click(&mut rt, "transfers-more");
    }
    assert_eq!(main.borrow().settings.transfers_at_once, 8, "+ stops at 8");
    assert_eq!(text_of(&rt, "transfers-value"), "8");
    for _ in 0..9 {
        click(&mut rt, "transfers-fewer");
    }
    assert_eq!(main.borrow().settings.transfers_at_once, 1, "− stops at 1");
    assert_eq!(text_of(&rt, "transfers-value"), "1");
}

#[test]
fn the_switch_sets_the_hidden_files_default() {
    let Rig { mut rt, main } = rig();
    click(&mut rt, "hidden-toggle");
    assert!(!main.borrow().settings.show_hidden && !main.borrow().show_hidden_default);
    click(&mut rt, "hidden-toggle");
    assert!(main.borrow().settings.show_hidden);
}

#[test]
fn tab_reaches_the_controls_and_space_presses_them() {
    let Rig { mut rt, main } = rig();
    let focused = |rt: &Runtime<Pane>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    let mut seen = Vec::new();
    for _ in 0..12 {
        rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        if let Some(k) = focused(&rt) {
            if k == "hidden-toggle" {
                break;
            }
            seen.push(k);
        }
    }
    assert_eq!(focused(&rt).as_deref(), Some("hidden-toggle"), "visited {seen:?}");
    for key in ["theme-system", "density-comfortable", "folder-choose", "transfers-more"] {
        assert!(seen.iter().any(|k| k == key), "{key} not in {seen:?}");
    }
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(!main.borrow().settings.show_hidden);
}

#[test]
fn the_window_shows_what_the_main_window_says() {
    let Rig { mut rt, main } = rig();
    // A value set elsewhere (another window, a launch) arrives as the main state's view.
    let view = {
        let mut m = main.borrow_mut();
        m.act(Action::Setting(SettingsChange::TransfersAtOnce(6)));
        m.settings_view()
    };
    rt.update(move |p, _| p.view = view);
    rt.frame(1.0 / 60.0);
    assert_eq!(text_of(&rt, "transfers-value"), "6");
}

#[test]
fn escape_and_cmd_w_close_the_window() {
    let ev = |key, mods| KeyEvent { key, mods };
    let cmd = Mods { cmd: true, ..Mods::default() };
    assert!(settings::closes(&ev(Key::Escape, Mods::default())));
    assert!(settings::closes(&ev(Key::Command('w'), cmd)));
    assert!(!settings::closes(&ev(Key::Command('w'), Mods { shift: true, ..cmd })));
    assert!(!settings::closes(&ev(Key::Text("w".into()), Mods::default())));
}

/// Notes each `open`.
#[derive(Clone, Default)]
struct Opened(Rc<RefCell<usize>>);

impl SettingsWindow for Opened {
    fn open(&mut self, _: SettingsView) {
        *self.0.borrow_mut() += 1;
    }
    fn show(&mut self, _: SettingsView) {}
}

fn main_runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime((1080.0, 680.0), Env::new(true, 1.0));
    rt.update(|s, _| *s = scenes::base());
    rt.frame(0.0);
    rt
}

#[test]
fn cmd_comma_in_the_main_window_opens_settings() {
    let mut rt = main_runtime();
    let opened = Opened::default();
    let window = opened.clone();
    rt.update(move |s, _| s.attach_settings_window(Box::new(window)));
    rt.frame(0.0);
    let mac = cfg!(target_os = "macos");
    let primary = Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() };
    rt.key(KeyEvent { key: Key::Command(','), mods: primary });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Command(','), mods: primary });
    rt.frame(1.0 / 60.0);
    assert_eq!(*opened.0.borrow(), 2, "again brings the one window forward");
}

#[test]
fn compact_density_makes_the_list_rows_24_points() {
    let mut rt = main_runtime();
    let (x, y) = rt.find("host-prod").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.active, Some(HostId("prod".into())));
    let height = |rt: &Runtime<App>, key: &str| rt.presented(key).map(|(r, _)| r[3]);
    assert_eq!(height(&rt, "rows/index.html"), Some(28.0));
    rt.update(|s, tx| crate::views::run(s, Action::Setting(SettingsChange::Density(Density::Compact)), tx));
    rt.frame(1.0);
    assert_eq!(height(&rt, "rows/index.html"), Some(24.0));
    // The rows below moved up with it: every row is 24 apart.
    let (a, b) = (rt.find("rows/index.html").unwrap().1, rt.find("rows/package-lock.json").unwrap().1);
    assert_eq!(b - a, 24.0);
}
