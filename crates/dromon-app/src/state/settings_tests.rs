//! Settings: defaults, clamping, what each change does at once, what's remembered, prefs from
//! before Settings, and what the window is told.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use dromon_core::HostId;

use super::*;
use crate::fixture;
use crate::state::{Action, Recorder, Saved, Shortcut, Store};

/// A store that keeps every snapshot it was handed.
#[derive(Clone, Default)]
struct Recorded(Rc<RefCell<Vec<Saved>>>);

impl Store for Recorded {
    fn save(&mut self, saved: &Saved) {
        self.0.borrow_mut().push(saved.clone());
    }
}

impl Recorded {
    fn last(&self) -> Saved {
        self.0.borrow().last().cloned().expect("something was saved")
    }
    fn count(&self) -> usize {
        self.0.borrow().len()
    }
}

/// A window that notes what it was asked: `(opened, view)` per call.
#[derive(Clone, Default)]
struct Window(Rc<RefCell<Vec<(bool, SettingsView)>>>);

impl SettingsWindow for Window {
    fn open(&mut self, view: SettingsView) {
        self.0.borrow_mut().push((true, view));
    }
    fn show(&mut self, view: SettingsView) {
        self.0.borrow_mut().push((false, view));
    }
}

impl Window {
    fn calls(&self) -> Vec<(bool, SettingsView)> {
        self.0.borrow().clone()
    }
}

/// An absolute folder on every platform.
fn folder(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

fn app_with(saved: Saved) -> (App, Recorded) {
    let mut s = fixture::app(fixture::demo());
    let store = Recorded::default();
    s.remember(saved, Box::new(store.clone()));
    (s, store)
}

fn set(s: &mut App, change: SettingsChange) {
    s.act(Action::Setting(change));
}

#[test]
fn the_defaults_are_the_plans() {
    let d = Settings::default();
    assert_eq!(d.theme, Appearance::System);
    assert_eq!((d.density, d.density.row()), (Density::Comfortable, 28.0));
    assert_eq!(Density::Compact.row(), 24.0);
    assert_eq!(d.download_folder, None);
    assert_eq!(d.transfers_at_once, 3);
    assert!(d.show_hidden);
    let s = fixture::app(fixture::demo());
    assert_eq!(s.settings, d);
    assert_eq!(s.download_folder(), s.downloads, "no folder chosen: the OS Downloads folder");
    // Prefs that never had Settings start there too.
    let (s, _) = app_with(Saved::default());
    assert_eq!(s.settings, d);
    assert!(s.show_hidden_default);
}

#[test]
fn values_out_of_range_are_put_back() {
    let wild = Settings { transfers_at_once: 0, download_folder: Some(PathBuf::from("")), ..Settings::default() };
    let c = wild.clamped();
    assert_eq!((c.transfers_at_once, c.download_folder), (1, None));
    let c = Settings { transfers_at_once: 99, download_folder: Some(PathBuf::from("relative/dir")), ..Settings::default() }.clamped();
    assert_eq!((c.transfers_at_once, c.download_folder), (8, None));
    let s = Settings::default();
    assert_eq!(s.with(SettingsChange::TransfersAtOnce(9)).transfers_at_once, 8);
    assert_eq!(s.with(SettingsChange::TransfersAtOnce(0)).transfers_at_once, 1);
    assert_eq!(s.with(SettingsChange::DownloadFolder(Some(folder("in")))).download_folder, Some(folder("in")));

    // From the file too: a launch never trusts what it reads.
    let saved = Saved { settings: Some(Settings { transfers_at_once: 40, ..Settings::default() }), ..Saved::default() };
    let (s, _) = app_with(saved);
    assert_eq!(s.settings.transfers_at_once, 8);
}

#[test]
fn settings_round_trip_through_the_prefs_file_format() {
    let settings = Settings {
        theme: Appearance::Light,
        density: Density::Compact,
        download_folder: Some(folder("dl")),
        transfers_at_once: 6,
        show_hidden: false,
    };
    let saved = Saved { settings: Some(settings), ..Saved::default() };
    let json = serde_json::to_value(&saved).unwrap();
    let section = &json["settings"];
    assert_eq!((section["theme"].as_str(), section["density"].as_str()), (Some("light"), Some("compact")));
    assert_eq!(section["transfers_at_once"], 6);
    let back: Saved = serde_json::from_value(json).unwrap();
    assert_eq!(back, saved);
    // A section missing some keys (another version) takes the defaults for those alone.
    let partial: Saved = serde_json::from_str(r#"{"settings": {"density": "compact"}}"#).unwrap();
    assert_eq!(partial.settings, Some(Settings { density: Density::Compact, ..Settings::default() }));
}

#[test]
fn prefs_from_before_settings_keep_the_hidden_files_choice() {
    // ⌘⇧. used to be remembered: its last state becomes the default.
    let old: Saved = serde_json::from_str(r#"{"show_hidden": false, "sidebar_width": 260.0}"#).unwrap();
    assert_eq!(old.settings, None);
    let (mut s, store) = app_with(old);
    assert!(!s.settings.show_hidden && !s.show_hidden_default);
    assert_eq!(s.settings, Settings { show_hidden: false, ..Settings::default() });
    assert_eq!(store.count(), 0, "reading old prefs writes nothing");
    // The first change writes the section, which takes over from the old key.
    set(&mut s, SettingsChange::Density(Density::Compact));
    let saved = store.last();
    assert_eq!(saved.show_hidden, None);
    assert_eq!(saved.settings, Some(Settings { show_hidden: false, density: Density::Compact, ..Settings::default() }));
    // The next launch reads the section.
    let (s, _) = app_with(saved);
    assert_eq!((s.settings.density, s.show_hidden_default), (Density::Compact, false));
}

#[test]
fn every_change_is_remembered_and_unchanged_values_write_nothing() {
    let (mut s, store) = app_with(Saved::default());
    set(&mut s, SettingsChange::Theme(Appearance::Dark));
    set(&mut s, SettingsChange::TransfersAtOnce(5));
    set(&mut s, SettingsChange::ShowHidden(false));
    set(&mut s, SettingsChange::DownloadFolder(Some(folder("x"))));
    let n = store.count();
    assert_eq!(n, 4);
    let remembered = store.last().settings.unwrap();
    assert_eq!(remembered, s.settings);
    assert_eq!((remembered.theme, remembered.transfers_at_once, remembered.show_hidden), (Appearance::Dark, 5, false));
    set(&mut s, SettingsChange::Theme(Appearance::Dark));
    set(&mut s, SettingsChange::TransfersAtOnce(12));
    set(&mut s, SettingsChange::TransfersAtOnce(8));
    assert_eq!(store.count(), n + 1, "the same theme writes nothing; 12 and 8 are both 8");
}

#[test]
fn the_theme_repaints_at_once() {
    let mut s = fixture::app(fixture::demo());
    set(&mut s, SettingsChange::Theme(Appearance::Light));
    assert_eq!(s.ui.appearance, Appearance::Light);
    set(&mut s, SettingsChange::Theme(Appearance::System));
    assert_eq!(s.ui.appearance, Appearance::System, "System follows the OS again");
}

#[test]
fn new_downloads_go_to_the_chosen_folder_and_reset_goes_back() {
    let mut s = fixture::app(fixture::demo());
    let sent = Recorder::default();
    s.act(Action::SelectHost(HostId("prod".into())));
    s.backend = Box::new(sent.clone());
    let into = |sent: &Recorder| match sent.sent().last() {
        Some(Command::Transfer(TransferCommand::Start(dromon_transfer::JobRequest::Download { into, .. }))) => into.clone(),
        other => panic!("no download: {other:?}"),
    };
    set(&mut s, SettingsChange::DownloadFolder(Some(folder("chosen"))));
    s.act(Action::Open("index.html".into()));
    assert_eq!(into(&sent), folder("chosen"));
    set(&mut s, SettingsChange::DownloadFolder(None));
    s.act(Action::Open("index.html".into()));
    assert_eq!(into(&sent), s.downloads);
}

#[test]
fn transfers_at_once_reaches_the_engine_once_per_change() {
    let mut s = fixture::app(fixture::demo());
    let sent = Recorder::default();
    s.backend = Box::new(sent.clone());
    set(&mut s, SettingsChange::TransfersAtOnce(5));
    set(&mut s, SettingsChange::TransfersAtOnce(5));
    set(&mut s, SettingsChange::Theme(Appearance::Dark));
    set(&mut s, SettingsChange::TransfersAtOnce(1));
    assert_eq!(
        sent.sent(),
        [Command::Transfer(TransferCommand::PerHost(5)), Command::Transfer(TransferCommand::PerHost(1))]
    );
}

#[test]
fn a_remembered_limit_is_sent_at_launch_and_the_default_is_not() {
    let mut s = fixture::app(fixture::demo());
    let sent = Recorder::default();
    s.backend = Box::new(sent.clone());
    s.remember(Saved::default(), Box::new(Recorded::default()));
    assert!(sent.sent().is_empty(), "the engines start at 3");
    let saved = Saved { settings: Some(Settings { transfers_at_once: 6, ..Settings::default() }), ..Saved::default() };
    let mut s = fixture::app(fixture::demo());
    s.backend = Box::new(sent.clone());
    s.remember(saved, Box::new(Recorded::default()));
    assert_eq!(sent.sent(), [Command::Transfer(TransferCommand::PerHost(6))]);
}

#[test]
fn the_hidden_files_default_is_for_hosts_opened_from_now_on() {
    let mut s = fixture::app(fixture::demo());
    let prod = HostId("prod".into());
    let nas = HostId("nas".into());
    s.act(Action::SelectHost(prod.clone()));
    set(&mut s, SettingsChange::ShowHidden(false));
    assert!(s.conns[&prod].show_hidden, "the host already open keeps its dotfiles");
    s.act(Action::SelectHost(nas.clone()));
    assert!(!s.conns[&nas].show_hidden);
    // ⌘⇧. still switches them all for the session, without touching the setting.
    s.act(Action::ToggleHidden);
    assert!(s.conns[&prod].show_hidden && s.conns[&nas].show_hidden);
    assert!(!s.settings.show_hidden);
    // A change of another setting leaves the session's switch alone.
    set(&mut s, SettingsChange::Density(Density::Compact));
    assert!(s.show_hidden_default);
}

#[test]
fn the_window_opens_on_the_apps_values_and_is_told_every_change() {
    let mut s = fixture::app(fixture::demo());
    let window = Window::default();
    s.attach_settings_window(Box::new(window.clone()));
    assert_eq!(window.calls(), [(false, s.settings_view())], "it shows the values from the start");
    s.act(Action::OpenSettings);
    s.shortcut(Shortcut::Settings, 0.0);
    set(&mut s, SettingsChange::TransfersAtOnce(20));
    let calls = window.calls();
    assert_eq!(calls.len(), 4);
    assert!(calls[1].0 && calls[2].0, "⌘, again opens (brings forward) the one window");
    assert_eq!((calls[3].0, calls[3].1.settings.transfers_at_once), (false, 8), "it shows the app's clamped value");
    assert_eq!(calls[3].1.default_folder, s.downloads);
}

#[test]
fn cmd_comma_opens_settings_over_whatever_is_modal_here() {
    let mut s = fixture::app(fixture::demo());
    let window = Window::default();
    s.attach_settings_window(Box::new(window.clone()));
    s.act(Action::QuickConnect);
    s.shortcut(Shortcut::Settings, 0.0);
    assert!(s.ui.quick.open, "the popover stays");
    assert!(window.calls().last().is_some_and(|(opened, _)| *opened));
}

#[test]
fn the_folder_label_shortens_the_home_folder() {
    let home = folder("home-me");
    let mut v = SettingsView { settings: Settings::default(), default_folder: home.join("Downloads") };
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(v.folder_label(Some(&home)), format!("~{sep}Downloads"));
    assert_eq!(v.folder_label(None), home.join("Downloads").display().to_string());
    v.settings.download_folder = Some(folder("elsewhere"));
    assert_eq!(v.folder(), folder("elsewhere"));
    assert_eq!(v.folder_label(Some(&home)), folder("elsewhere").display().to_string());
    v.settings.download_folder = Some(home.clone());
    assert_eq!(v.folder_label(Some(&home)), "~");
}
