//! What a main window shows beside its sidebar (§6.8): the folder view, or the Settings page in
//! its place. One window holds both, so Settings never opens a second one.
//!
//! The page is a [`Page`], moved by the pure [`step`]. ⌘, (Ctrl+, elsewhere), the palette's
//! "Settings…", the macOS app menu's "Settings…" and the sidebar's gear open it on Appearance;
//! opening it while it's shown keeps it (and its section). The back button and Escape leave it,
//! and so does picking a host (the sidebar, ⌘1 … ⌘9, the palette): the host's folder view comes
//! back as it was. Leaving touches nothing else: the folder, history, selection and filter are
//! the window's state, and the view keeps the folder view mounted (inert, hidden) under the page,
//! so its scroll position stays too (`views/mod.rs`).
//!
//! Each window has its own page; a change made on it applies app-wide as before
//! (`settings.rs`: [`Action::Setting`], remembered, [`News::Settings`](super::News::Settings) to
//! the other windows, whose pages read the same values).

use std::path::PathBuf;

use super::{Action, App, Effect, PaletteAction, Shortcut};

/// The Settings page's sections, in the order its list shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsSection {
    /// Theme and density.
    #[default]
    Appearance,
    /// The download folder and transfers at once.
    Transfers,
    /// Hidden files by default.
    Browsing,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 3] = [SettingsSection::Appearance, SettingsSection::Transfers, SettingsSection::Browsing];

    pub fn title(self) -> &'static str {
        match self {
            SettingsSection::Appearance => "Appearance",
            SettingsSection::Transfers => "Transfers",
            SettingsSection::Browsing => "Browsing",
        }
    }

    /// The key of its entry in the section list.
    pub fn key(self) -> &'static str {
        match self {
            SettingsSection::Appearance => "settings-tab-appearance",
            SettingsSection::Transfers => "settings-tab-transfers",
            SettingsSection::Browsing => "settings-tab-browsing",
        }
    }
}

/// What the window's content area shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    /// The toolbar, the list (or the welcome state) and the status bar.
    #[default]
    Files,
    /// The Settings page, on this section.
    Settings(SettingsSection),
}

impl Page {
    /// The section on screen while the Settings page is.
    pub fn settings(self) -> Option<SettingsSection> {
        match self {
            Page::Files => None,
            Page::Settings(section) => Some(section),
        }
    }
}

/// What moves the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEvent {
    /// ⌘,, the menus, the palette, the gear.
    OpenSettings,
    /// Back, Escape, a host picked.
    Leave,
    /// A section picked in the list.
    Section(SettingsSection),
}

/// The page's state machine.
pub fn step(page: Page, ev: PageEvent) -> Page {
    match (page, ev) {
        (Page::Files, PageEvent::OpenSettings) => Page::Settings(SettingsSection::default()),
        (Page::Settings(section), PageEvent::OpenSettings) => Page::Settings(section),
        (_, PageEvent::Leave) => Page::Files,
        (Page::Settings(_), PageEvent::Section(section)) => Page::Settings(section),
        (Page::Files, PageEvent::Section(_)) => Page::Files,
    }
}

/// The Settings page's own state, besides the values it shows (`App::settings_view`).
#[derive(Clone, Debug, Default)]
pub struct SettingsPage {
    /// The home folder, to write the download folder as `~/…`; `None` (scenes, tests, headless
    /// runs) writes paths whole.
    pub home: Option<PathBuf>,
    /// The folder picker couldn't open (Linux without the desktop portal); the next change or
    /// opening the page again clears it.
    pub error: Option<String>,
}

impl App {
    /// Whether the Settings page is on screen.
    pub fn on_settings(&self) -> bool {
        self.ui.page.settings().is_some()
    }

    /// Opens the Settings page, or keeps it. From the folder view, what floats over it goes
    /// (the transfers popover, the drawn menu); everything else of it stays as it is.
    pub(super) fn open_settings(&mut self) {
        if !self.on_settings() {
            self.transfers.open = false;
            self.files.menu = None;
            self.ui.type_select.reset();
            self.ui.settings_page.error = None;
        }
        self.ui.page = step(self.ui.page, PageEvent::OpenSettings);
    }

    /// Back to the folder view the window had.
    pub(super) fn leave_settings(&mut self) {
        self.ui.page = step(self.ui.page, PageEvent::Leave);
    }

    /// A section picked in the page's list.
    pub(super) fn show_section(&mut self, section: SettingsSection) {
        self.ui.page = step(self.ui.page, PageEvent::Section(section));
    }

    /// A shortcut while the Settings page is on screen: Escape leaves it, the palette, quick
    /// connect and ⌘1 … ⌘9 work as anywhere; the folder view's keys wait for it.
    pub(super) fn settings_shortcut(&mut self, sc: Shortcut) -> Vec<Effect> {
        let action = match sc {
            Shortcut::Escape => Action::LeaveSettings,
            Shortcut::Palette => Action::Palette(PaletteAction::Toggle),
            Shortcut::QuickConnect => Action::QuickConnect,
            Shortcut::Host(n) => match n.checked_sub(1).and_then(|i| self.sidebar_order().get(i).cloned()) {
                Some(host) => Action::SelectHost(host),
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        self.act(action)
    }
}
