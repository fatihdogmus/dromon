//! Settings: defaults, clamping, what each change does at once, what's remembered, prefs from
//! before Settings; and the Settings page (`page.rs`): what opens and leaves it, its sections,
//! the folder view it leaves as it was, and a change reaching another window's page.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use dromon_core::HostId;

use super::*;
use crate::fixture;
use crate::state::frames::Frame;
use crate::state::page::{self, PageEvent};
use crate::state::{Action, News, Page, PaletteAction, Peers, Recorder, Saved, SettingsSection, Shortcut, Store, WindowId};

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
fn the_page_moves_by_its_step() {
    use SettingsSection::{Appearance as Look, Browsing, Transfers};
    let on = Page::Settings;
    assert_eq!(page::step(Page::Files, PageEvent::OpenSettings), on(Look), "it opens on Appearance");
    assert_eq!(page::step(on(Transfers), PageEvent::OpenSettings), on(Transfers), "opening it again keeps it");
    assert_eq!(page::step(on(Look), PageEvent::Section(Browsing)), on(Browsing));
    assert_eq!(page::step(Page::Files, PageEvent::Section(Browsing)), Page::Files, "no section without the page");
    assert_eq!(page::step(on(Browsing), PageEvent::Leave), Page::Files);
    assert_eq!(page::step(Page::Files, PageEvent::Leave), Page::Files);
    assert_eq!(Page::default(), Page::Files);
    assert_eq!(SettingsSection::ALL.map(SettingsSection::title), ["Appearance", "Transfers", "Browsing"]);
}

/// Pick a palette row by its key, the query typed first (an empty query may not list it).
fn pick(s: &mut App, query: &str, key: &str) {
    s.act(Action::Palette(PaletteAction::Toggle));
    s.ui.palette.edit = rune::TextEdit::new(query);
    s.act(Action::Palette(PaletteAction::Edited));
    s.act(Action::Palette(PaletteAction::Pick(key.into())));
}

#[test]
fn every_entry_point_opens_the_page_and_opening_again_keeps_it() {
    let appearance = Page::Settings(SettingsSection::Appearance);
    let mut s = fixture::app(fixture::demo());
    s.act(Action::OpenSettings);
    assert_eq!(s.ui.page, appearance, "the gear and the app menu's Settings…");
    s.act(Action::SettingsSection(SettingsSection::Transfers));
    s.act(Action::OpenSettings);
    s.shortcut(Shortcut::Settings, 0.0);
    assert_eq!(s.ui.page, Page::Settings(SettingsSection::Transfers), "no toggle, and the section stays");

    let mut s = fixture::app(fixture::demo());
    s.shortcut(Shortcut::Settings, 0.0);
    assert_eq!(s.ui.page, appearance, "⌘, / Ctrl+,");

    let mut s = fixture::app(fixture::demo());
    pick(&mut s, "settings", "pal-settings");
    assert_eq!(s.ui.page, appearance, "the palette's Settings…");
    assert!(!s.ui.palette.is_open());
}

#[test]
fn cmd_comma_opens_the_page_under_whatever_is_modal_here() {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::QuickConnect);
    s.shortcut(Shortcut::Settings, 0.0);
    assert!(s.ui.quick.open, "the popover stays");
    assert!(s.on_settings());
}

/// What the window's folder view is: host, folder, history, selection, filter, rows.
fn folder_view(s: &App) -> (Option<HostId>, String, bool, Vec<String>, bool, Vec<String>) {
    let c = s.conn().expect("a host on screen");
    let selected = c.selection.iter().map(str::to_owned).collect();
    let rows = c.row_names().into_iter().map(str::to_owned).collect();
    (s.active.clone(), c.cwd.to_string(), c.history.can_back(), selected, c.filter.is_on(), rows)
}

/// prod, back from releases, index.html and robots.txt selected, the filter on "t".
fn browsing() -> App {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::SelectHost(HostId("prod".into())));
    s.act(Action::Open("releases".into()));
    s.act(Action::GoUp);
    s.act(Action::OpenFilter);
    s.ui.filter_edit = rune::TextEdit::new("t");
    s.act(Action::FilterChanged("t".into()));
    s.act(Action::Click { name: "index.html".into(), extend: false, toggle: false });
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: true });
    s
}

#[test]
fn back_and_escape_leave_the_page_for_the_folder_view_as_it_was() {
    let mut s = browsing();
    let before = folder_view(&s);
    assert_eq!(before.3.len(), 2);
    s.act(Action::OpenSettings);
    // The folder view's keys wait while the page is up: nothing moves, opens or downloads.
    let sent = Recorder::default();
    s.backend = Box::new(sent.clone());
    for sc in [
        Shortcut::Move { delta: 1, extend: false },
        Shortcut::Open,
        Shortcut::Back,
        Shortcut::Up,
        Shortcut::SelectAll,
        Shortcut::Download,
        Shortcut::ToggleHidden,
        Shortcut::Filter,
        Shortcut::Type("a".into()),
    ] {
        assert_eq!(s.shortcut(sc.clone(), 0.0), Vec::new(), "{sc:?}");
    }
    assert!(sent.sent().is_empty(), "{:?}", sent.sent());
    assert!(s.on_settings());
    assert_eq!(folder_view(&s), before);
    // Escape leaves the page (it doesn't clear the selection, as it would on the list).
    s.shortcut(Shortcut::Escape, 0.0);
    assert_eq!(s.ui.page, Page::Files);
    assert_eq!(folder_view(&s), before);
    // The back button.
    s.act(Action::OpenSettings);
    s.act(Action::LeaveSettings);
    assert_eq!(s.ui.page, Page::Files);
    assert_eq!(folder_view(&s), before);
}

#[test]
fn a_host_picked_anywhere_leaves_the_page_and_goes_there() {
    let nas = HostId("nas".into());
    // The sidebar (or the active host again: its folder view, as it was).
    let mut s = browsing();
    let before = folder_view(&s);
    s.act(Action::OpenSettings);
    s.act(Action::SelectHost(HostId("prod".into())));
    assert_eq!((s.ui.page, folder_view(&s)), (Page::Files, before));
    s.act(Action::OpenSettings);
    s.act(Action::SelectHost(nas.clone()));
    assert_eq!((s.ui.page, s.active.clone()), (Page::Files, Some(nas.clone())));
    // ⌘1 … ⌘9.
    let mut s = browsing();
    s.act(Action::OpenSettings);
    let n = s.sidebar_order().iter().position(|h| *h == nas).unwrap() + 1;
    s.shortcut(Shortcut::Host(n), 0.0);
    assert_eq!((s.ui.page, s.active.clone()), (Page::Files, Some(nas.clone())));
    // The palette: any row but Settings… is about the folder view.
    let mut s = browsing();
    s.act(Action::OpenSettings);
    pick(&mut s, "settings", "pal-settings");
    assert!(s.on_settings(), "Settings… keeps it");
    pick(&mut s, "nas", "pal-host-nas");
    assert_eq!((s.ui.page, s.active.clone()), (Page::Files, Some(nas)));
    // A host that isn't there changes nothing.
    let mut s = browsing();
    s.act(Action::OpenSettings);
    s.act(Action::SelectHost(HostId("nope".into())));
    assert!(s.on_settings());
}

#[test]
fn escape_closes_the_palette_or_the_popover_before_the_page() {
    let mut s = browsing();
    s.act(Action::OpenSettings);
    s.shortcut(Shortcut::Palette, 0.0);
    assert!(s.ui.palette.is_open(), "⌘K works on the page");
    s.shortcut(Shortcut::Escape, 0.0);
    assert!(!s.ui.palette.is_open() && s.on_settings());
    s.shortcut(Shortcut::QuickConnect, 0.0);
    assert!(s.ui.quick.open, "so does ⌘⇧K");
    s.shortcut(Shortcut::Escape, 0.0);
    assert!(!s.ui.quick.open && s.on_settings());
    s.shortcut(Shortcut::Escape, 0.0);
    assert!(!s.on_settings());
}

#[test]
fn opening_the_page_closes_what_floats_over_the_folder_view() {
    let mut s = browsing();
    s.act(Action::ToggleTransfers);
    s.ui.settings_page.error = Some("no portal".into());
    s.act(Action::OpenSettings);
    assert!(!s.transfers.open, "the transfers popover hangs from the toolbar, which the page covers");
    assert_eq!(s.ui.settings_page.error, None);
    // The folder picker's error goes with the next change.
    s.ui.settings_page.error = Some("no portal".into());
    set(&mut s, SettingsChange::TransfersAtOnce(4));
    assert_eq!(s.ui.settings_page.error, None);
}

/// Peers that keep the windows ⌘N opens, the news told and the themes the frames were given.
#[derive(Clone, Default)]
struct Windows {
    opened: Rc<RefCell<Vec<App>>>,
    news: Rc<RefCell<Vec<(WindowId, News)>>>,
    themes: Rc<RefCell<Vec<Appearance>>>,
}

impl Peers for Windows {
    fn can_open(&self) -> bool {
        true
    }
    fn open_window(&self, _: WindowId, app: App) {
        self.opened.borrow_mut().push(app);
    }
    fn tell(&self, from: WindowId, news: News) {
        self.news.borrow_mut().push((from, news));
    }
    fn frame(&self, _: WindowId) -> Option<Frame> {
        None
    }
    fn closed(&self, _: WindowId) {}
    fn theme(&self, theme: Appearance) {
        self.themes.borrow_mut().push(theme);
    }
}

impl Windows {
    /// Hands the news to `to`, as `crate::windows` does.
    fn deliver(&self, to: &mut App) {
        for (from, news) in std::mem::take(&mut *self.news.borrow_mut()) {
            if from != to.window {
                to.hear(news);
            }
        }
    }
}

#[test]
fn each_window_has_its_own_page_and_a_change_shows_on_every_one() {
    let mut a = fixture::app(fixture::demo());
    let peers = Windows::default();
    a.set_peers(Rc::new(peers.clone()));
    a.act(Action::OpenSettings);
    a.act(Action::NewWindow);
    let mut b = peers.opened.borrow_mut().pop().expect("⌘N opened a window");
    assert_eq!(b.ui.page, Page::Files, "a new window opens on its folder view");
    b.act(Action::OpenSettings);
    b.act(Action::SettingsSection(SettingsSection::Transfers));
    assert_eq!(a.ui.page, Page::Settings(SettingsSection::Appearance), "a's page is its own");

    // A change on a's page reaches b's, which shows it, on the section it was on.
    set(&mut a, SettingsChange::TransfersAtOnce(6));
    peers.deliver(&mut b);
    assert_eq!(b.settings_view().settings.transfers_at_once, 6);
    assert_eq!(b.ui.page, Page::Settings(SettingsSection::Transfers));
    // And back: the theme from b's page repaints a, and the windows' frames follow it once.
    set(&mut b, SettingsChange::Theme(Appearance::Light));
    peers.deliver(&mut a);
    assert_eq!((a.settings.theme, a.ui.appearance), (Appearance::Light, Appearance::Light));
    set(&mut b, SettingsChange::Density(Density::Compact));
    assert_eq!(*peers.themes.borrow(), [Appearance::Light], "only a theme change reaches the frames");
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
