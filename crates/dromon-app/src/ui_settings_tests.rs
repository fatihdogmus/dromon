//! The Settings page driven headless in the real main window (`app()`, its view, handlers and
//! `on_key`): what opens it (the gear, ⌘,), the sidebar beside it, its controls and the folder
//! picker, a change from another window, and leaving it for the folder view as it was.

use std::path::PathBuf;

use dromon_core::HostId;
use rune::{FileDialogKind, Key, KeyEvent, Mods, Runtime};

use crate::scenes;
use crate::state::{Action, App, Density, News, Page, Settings, SettingsChange, SettingsSection};
use crate::theme::{self, Appearance};
use crate::ui_a11y_tests::{scene_runtime, settle, tree};

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

/// ⌘ on macOS, Ctrl elsewhere.
fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

fn text_of(rt: &Runtime<App>, key: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text).unwrap_or_default()
}

fn fill_of(rt: &Runtime<App>, key: &str) -> rune::Color {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .map(|e| e.fill)
        .unwrap_or_else(|| panic!("no element {key:?}"))
}

fn folder(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

/// The page on `section`, opened from the welcome state.
fn page(section: SettingsSection) -> Runtime<App> {
    let mut rt = scene_runtime(scenes::welcome);
    rt.update(move |s, tx| {
        crate::views::run(s, Action::OpenSettings, tx);
        crate::views::run(s, Action::SettingsSection(section), tx);
    });
    settle(&mut rt);
    rt
}

#[test]
fn the_gear_shows_the_page_in_place_of_the_folder_view_beside_the_sidebar() {
    let mut rt = scene_runtime(scenes::listing);
    click(&mut rt, "sidebar-settings");
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Appearance));
    for key in [
        "settings-back",
        "settings-title",
        "settings-tab-appearance",
        "settings-tab-transfers",
        "settings-tab-browsing",
        "theme-system",
        "density-comfortable",
    ] {
        assert!(rt.find(key).is_some(), "{key}");
    }
    assert_eq!(text_of(&rt, "settings-title"), "Settings");
    // It covers the toolbar, the list and the status bar: everything right of the sidebar.
    let (rect, _) = rt.presented("settings").expect("the page");
    let sidebar = rt.state.ui.sidebar_width;
    assert_eq!((rect[0], rect[1], rect[2], rect[3]), (sidebar, 0.0, 1080.0 - sidebar, 680.0));
    // The sidebar stays; the folder view is out of readers' reach while it waits underneath.
    let nodes = tree(&rt);
    for key in ["host-prod", "host-nas", "sb-quick-connect", "sidebar-settings", "settings-sections"] {
        assert!(nodes.iter().any(|n| n.key == key), "{key} is in the tree");
    }
    for key in ["toolbar", "file-grid", "rows", "status-conn"] {
        assert!(!nodes.iter().any(|n| n.key == key), "{key} is hidden under the page");
    }
    assert_eq!(fill_of(&rt, "settings"), theme::tokens(true).surface);
    // The gear again keeps it.
    click(&mut rt, "sidebar-settings");
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Appearance));
}

#[test]
fn cmd_comma_shows_the_page_and_again_keeps_it() {
    let mut rt = scene_runtime(scenes::listing);
    press(&mut rt, Key::Command(','), primary());
    assert!(rt.find("settings").is_some());
    press(&mut rt, Key::Command(','), primary());
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Appearance), "no toggle");
}

#[test]
fn back_and_escape_leave_for_the_folder_view_scrolled_where_it_was() {
    // The big folder, scrolled to IMG_05000.jpg in the middle and selecting it.
    let mut rt = scene_runtime(scenes::big_folder);
    let row = "rows/IMG_05000.jpg";
    let at = rt.find(row).expect("the row on screen");
    let selected = |rt: &Runtime<App>| rt.state.conn().unwrap().selection.iter().map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(selected(&rt), ["IMG_05000.jpg"]);
    press(&mut rt, Key::Command(','), primary());
    assert!(rt.state.on_settings());
    press(&mut rt, Key::Escape, Mods::default());
    assert_eq!(rt.state.ui.page, Page::Files);
    assert!(rt.find("settings").is_none(), "the page is gone");
    assert_eq!(rt.find(row), Some(at), "the list is where it was scrolled");
    assert_eq!(selected(&rt), ["IMG_05000.jpg"]);
    // The back button.
    press(&mut rt, Key::Command(','), primary());
    click(&mut rt, "settings-back");
    assert_eq!(rt.state.ui.page, Page::Files);
    assert_eq!(rt.find(row), Some(at));
    assert_eq!(selected(&rt), ["IMG_05000.jpg"]);
}

#[test]
fn a_host_in_the_sidebar_leaves_the_page_and_goes_there() {
    let mut rt = scene_runtime(scenes::listing);
    click(&mut rt, "sidebar-settings");
    click(&mut rt, "host-nas");
    assert_eq!(rt.state.ui.page, Page::Files);
    assert_eq!(rt.state.active, Some(HostId("nas".into())));
    assert!(rt.find("settings").is_none());
    assert!(rt.find("toolbar").is_some());
}

#[test]
fn the_sections_list_picks_what_the_panel_shows() {
    let mut rt = page(SettingsSection::Appearance);
    assert!(rt.find("folder-choose").is_none(), "Transfers' controls are on their own section");
    click(&mut rt, "settings-tab-transfers");
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Transfers));
    for key in ["folder-choose", "transfers-value"] {
        assert!(rt.find(key).is_some(), "{key}");
    }
    assert!(rt.find("theme-system").is_none());
    click(&mut rt, "settings-tab-browsing");
    assert!(rt.find("hidden-toggle").is_some());
    assert!(rt.find("folder-choose").is_none());
}

#[test]
fn theme_applies_at_once() {
    let mut rt = page(SettingsSection::Appearance);
    // The system is dark (the runtime's Env): System paints dark.
    click(&mut rt, "theme-light");
    assert_eq!((rt.state.settings.theme, rt.state.ui.appearance), (Appearance::Light, Appearance::Light));
    assert_eq!(fill_of(&rt, "settings"), theme::tokens(false).surface, "the page repaints with the window");
    click(&mut rt, "theme-system");
    assert_eq!(rt.state.ui.appearance, Appearance::System);
    assert_eq!(fill_of(&rt, "settings"), theme::tokens(true).surface);
}

#[test]
fn density_segments_set_the_lists_rows() {
    let mut rt = page(SettingsSection::Appearance);
    click(&mut rt, "density-compact");
    assert_eq!(rt.state.settings.density, Density::Compact);
    click(&mut rt, "density-comfortable");
    assert_eq!(rt.state.settings.density, Density::Comfortable);
}

#[test]
fn choose_picks_a_folder_and_reset_goes_back_to_downloads() {
    let mut rt = page(SettingsSection::Transfers);
    assert_eq!(text_of(&rt, "folder-path"), rt.state.downloads.display().to_string());
    assert_eq!(text_of(&rt, "folder-default"), "Default");
    assert!(rt.find("folder-reset").is_none(), "nothing to reset at the default");
    click(&mut rt, "folder-choose");
    let (kind, options) = rt.file_dialog().expect("the folder picker is up");
    assert_eq!(kind, FileDialogKind::Folder);
    assert_eq!(options.directory.as_ref(), Some(&rt.state.downloads), "it starts in the folder in use");
    // Cancelled: nothing changes.
    rt.resolve_file_dialog(None).unwrap();
    settle(&mut rt);
    assert_eq!(rt.state.settings.download_folder, None);

    click(&mut rt, "folder-choose");
    rt.resolve_file_dialog(Some(folder("picked"))).unwrap();
    settle(&mut rt);
    assert_eq!(rt.state.settings.download_folder, Some(folder("picked")));
    assert_eq!(rt.state.download_folder(), folder("picked"));
    assert_eq!(text_of(&rt, "folder-path"), folder("picked").display().to_string());
    assert!(rt.find("folder-default").is_none());
    assert!(rt.state.on_settings(), "the page stays after the picker");

    click(&mut rt, "folder-reset");
    assert_eq!(rt.state.settings.download_folder, None);
    assert!(rt.find("folder-reset").is_none());
    assert_eq!(text_of(&rt, "folder-default"), "Default");
}

#[test]
fn the_pickers_error_shows_until_the_next_change() {
    let mut rt = page(SettingsSection::Transfers);
    rt.update(|s, _| s.ui.settings_page.error = Some("The folder picker isn't available: no portal".into()));
    settle(&mut rt);
    assert_eq!(text_of(&rt, "folder-error"), "The folder picker isn't available: no portal");
    assert!(rt.find("folder-path").is_none());
    click(&mut rt, "transfers-more");
    assert!(rt.find("folder-error").is_none());
    assert!(rt.find("folder-path").is_some());
}

#[test]
fn the_stepper_counts_within_one_to_eight() {
    let mut rt = page(SettingsSection::Transfers);
    for _ in 0..7 {
        click(&mut rt, "transfers-more");
    }
    assert_eq!(rt.state.settings.transfers_at_once, 8, "+ stops at 8");
    assert_eq!(text_of(&rt, "transfers-value"), "8");
    for _ in 0..9 {
        click(&mut rt, "transfers-fewer");
    }
    assert_eq!(rt.state.settings.transfers_at_once, 1, "− stops at 1");
    assert_eq!(text_of(&rt, "transfers-value"), "1");
}

#[test]
fn the_switch_sets_the_hidden_files_default() {
    let mut rt = page(SettingsSection::Browsing);
    click(&mut rt, "hidden-toggle");
    assert!(!rt.state.settings.show_hidden && !rt.state.show_hidden_default);
    click(&mut rt, "hidden-toggle");
    assert!(rt.state.settings.show_hidden);
}

/// A newer version found, which this copy installs itself.
fn found(s: &mut App) {
    use crate::state::update::{Method, Offer, Package, UpdateEvent, Version};
    let offer = Offer {
        version: Version::parse("9.9.9").unwrap(),
        page: "https://github.com/fatihdogmus/dromon/releases/tag/9.9.9".into(),
        method: Method::Install(Package::Dmg),
    };
    s.apply(crate::state::ServiceEvent::Update(UpdateEvent::Available(offer)));
}

#[test]
fn general_checks_for_updates_and_says_what_it_found() {
    let mut rt = page(SettingsSection::General);
    assert_eq!(text_of(&rt, "update-status"), format!("You have Dromon {}", env!("CARGO_PKG_VERSION")));
    // The demo's backend finds nothing newer, with no network.
    click(&mut rt, "update-check");
    assert_eq!(text_of(&rt, "update-status"), format!("Dromon {} is up to date", env!("CARGO_PKG_VERSION")));
    click(&mut rt, "updates-toggle");
    assert!(!rt.state.settings.check_updates);
    click(&mut rt, "updates-toggle");
    assert!(rt.state.settings.check_updates);
    // A newer version: the line says so and Update and Release notes come beside Check now.
    rt.update(|s, _| found(s));
    settle(&mut rt);
    assert_eq!(text_of(&rt, "update-status"), "Dromon 9.9.9 is available.");
    assert!(rt.find("settings-update-install").is_some() && rt.find("settings-update-notes").is_some());
    let offences = crate::ui_a11y_tests::audit(&tree(&rt));
    assert!(offences.is_empty(), "{offences:?}");
}

#[test]
fn the_update_toast_offers_update_later_and_release_notes() {
    let mut rt = scene_runtime(scenes::listing);
    rt.update(|s, _| found(s));
    settle(&mut rt);
    assert_eq!(text_of(&rt, "toast-update-text"), "Dromon 9.9.9 is available");
    for key in ["toast-update-install", "toast-update-later", "toast-update-notes"] {
        assert!(rt.find(key).is_some(), "{key}");
    }
    let offences = crate::ui_a11y_tests::audit(&tree(&rt));
    assert!(offences.is_empty(), "{offences:?}");
    click(&mut rt, "toast-update-notes");
    assert_eq!(rt.take_url_requests(), ["https://github.com/fatihdogmus/dromon/releases/tag/9.9.9"]);
    click(&mut rt, "toast-update-later");
    assert!(rt.find("toast-update").is_none(), "Later hides it");
}

#[test]
fn the_page_shows_a_change_made_in_another_window() {
    let mut rt = page(SettingsSection::Transfers);
    assert_eq!(text_of(&rt, "transfers-value"), "3");
    // Another window's page changed it: the news reaches this window (`windows.rs`).
    let settings = Settings { transfers_at_once: 6, theme: Appearance::Light, ..Settings::default() };
    rt.update(move |s, _| s.hear(News::Settings(settings)));
    settle(&mut rt);
    assert_eq!(text_of(&rt, "transfers-value"), "6");
    assert_eq!(fill_of(&rt, "settings"), theme::tokens(false).surface);
    assert_eq!(rt.state.ui.page, Page::Settings(SettingsSection::Transfers), "on the section it was on");
}

#[test]
fn compact_density_makes_the_list_rows_24_points() {
    let mut rt = scene_runtime(scenes::welcome);
    click(&mut rt, "host-prod");
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
