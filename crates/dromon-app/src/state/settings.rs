//! Settings (§6.8, §8): theme, list density, the download folder, transfers at once per host,
//! and whether hidden files show by default.
//!
//! The values live in each main window's state ([`App::settings`]), the same in all of them.
//! The Settings window is one more Rune window with its own state (`views/settings.rs`): the
//! app tells it what to show through a [`SettingsWindow`] after every change, and it sends each
//! change back as [`Action::Setting`](super::Action::Setting) to one main window
//! (`settings_window.rs` carries both ways over the windows' `AppHandle`s). A change applies
//! there at once, is remembered in `prefs.json` ([`Saved::settings`](super::Saved::settings))
//! and reaches the other main windows as [`News::Settings`](super::News::Settings).

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{App, Command, TransferCommand};
use crate::theme::{Appearance, size};

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;

/// Transfers at once per host by default (§8), the engines' own starting value.
pub const AT_ONCE_DEFAULT: usize = 3;
/// What Transfers at once can be set to.
pub const AT_ONCE: RangeInclusive<usize> = 1..=8;

/// How tall the file list's rows are (§8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    /// 28 pt rows.
    #[default]
    Comfortable,
    /// 24 pt rows.
    Compact,
}

impl Density {
    /// The file list's row height: the rows, their virtualization, the skeleton and the new
    /// folder's row.
    pub fn row(self) -> f32 {
        match self {
            Density::Comfortable => size::ROW,
            Density::Compact => size::ROW_COMPACT,
        }
    }
}

/// Every setting, as remembered. A field missing from the file takes its default (§8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Appearance,
    pub density: Density,
    /// The folder downloads go to; `None` (or empty in the file) is the OS Downloads folder.
    pub download_folder: Option<PathBuf>,
    /// Files running at once per host, within [`AT_ONCE`].
    pub transfers_at_once: usize,
    /// Show dotfiles in hosts opened from now on (⌘⇧. still switches them for the session).
    pub show_hidden: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Appearance::System,
            density: Density::Comfortable,
            download_folder: None,
            transfers_at_once: AT_ONCE_DEFAULT,
            show_hidden: true,
        }
    }
}

impl Settings {
    /// The settings with values a file (or a caller) got wrong put back in range: transfers at
    /// once clamped to [`AT_ONCE`], an empty or relative download folder back to the default
    /// (a relative one would depend on where the app was started).
    pub fn clamped(mut self) -> Settings {
        self.transfers_at_once = self.transfers_at_once.clamp(*AT_ONCE.start(), *AT_ONCE.end());
        self.download_folder = self.download_folder.filter(|p| p.is_absolute());
        self
    }

    /// These settings with `change` made, in range.
    pub fn with(&self, change: SettingsChange) -> Settings {
        let mut s = self.clone();
        match change {
            SettingsChange::Theme(theme) => s.theme = theme,
            SettingsChange::Density(density) => s.density = density,
            SettingsChange::DownloadFolder(folder) => s.download_folder = folder,
            SettingsChange::TransfersAtOnce(n) => s.transfers_at_once = n,
            SettingsChange::ShowHidden(show) => s.show_hidden = show,
        }
        s.clamped()
    }
}

/// One control of the Settings window, set to a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsChange {
    Theme(Appearance),
    Density(Density),
    /// "Choose…" picked a folder; `None` is "Reset" (the OS Downloads folder).
    DownloadFolder(Option<PathBuf>),
    TransfersAtOnce(usize),
    ShowHidden(bool),
}

/// What the Settings window shows: the settings, and the folder "default" stands for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub settings: Settings,
    /// The OS Downloads folder, where downloads go while no folder is chosen.
    pub default_folder: PathBuf,
}

impl SettingsView {
    /// Where downloads go.
    pub fn folder(&self) -> &Path {
        self.settings.download_folder.as_deref().unwrap_or(&self.default_folder)
    }

    /// The download folder as the window writes it: under the home folder as `~/…`.
    pub fn folder_label(&self, home: Option<&Path>) -> String {
        let folder = self.folder();
        match home.and_then(|h| folder.strip_prefix(h).ok()) {
            Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
            None => folder.display().to_string(),
        }
    }
}

/// The Settings window as the app sees it: opened (or brought forward) on ⌘, and the menu, and
/// told the values after every change, so it never shows anything but the app's own.
pub trait SettingsWindow {
    /// Open the window, or bring it forward if it's open, showing `view`.
    fn open(&mut self, view: SettingsView);
    /// The values changed: show `view` (whether the window is open or not).
    fn show(&mut self, view: SettingsView);
    /// The same Settings window, for another main window's state (there's one for the app).
    fn another(&self) -> Box<dyn SettingsWindow>;
}

/// No window: scenes, tests and headless runs, where a second window never opens.
pub struct NoWindow;

impl SettingsWindow for NoWindow {
    fn open(&mut self, _: SettingsView) {}
    fn show(&mut self, _: SettingsView) {}
    fn another(&self) -> Box<dyn SettingsWindow> {
        Box::new(NoWindow)
    }
}

impl App {
    /// What the Settings window shows now.
    pub fn settings_view(&self) -> SettingsView {
        SettingsView { settings: self.settings.clone(), default_folder: self.downloads.clone() }
    }

    /// Where a download goes now: the folder chosen in Settings, else the OS Downloads folder.
    pub fn download_folder(&self) -> PathBuf {
        self.settings.download_folder.clone().unwrap_or_else(|| self.downloads.clone())
    }

    /// The real Settings window is there (the app started): it shows the settings from now on.
    pub fn attach_settings_window(&mut self, mut window: Box<dyn SettingsWindow>) {
        window.show(self.settings_view());
        self.settings_window = window;
    }

    /// The settings remembered last time (`remember`). Prefs from before Settings existed have
    /// no section: the defaults, with ⌘⇧.'s last state (which used to be remembered) as the
    /// hidden-files default.
    pub(super) fn restore_settings(&mut self) {
        let saved = self.saved().settings.clone();
        let settings = match saved {
            Some(s) => s.clamped(),
            None => Settings { show_hidden: self.saved().show_hidden.unwrap_or(true), ..Settings::default() },
        };
        self.apply_settings(settings, true);
    }

    /// ⌘, (Ctrl+, elsewhere), the app menu's Settings…, the palette.
    pub(super) fn open_settings(&mut self) {
        let view = self.settings_view();
        self.settings_window.open(view);
    }

    /// A control in the Settings window: applied at once, remembered, shown back, and told to
    /// the other main windows (which apply it too).
    pub(super) fn change_setting(&mut self, change: SettingsChange) {
        let new = self.settings.with(change);
        if new != self.settings {
            self.apply_settings(new, true);
            let settings = self.settings.clone();
            self.save(|s| {
                s.settings = Some(settings.clone());
                // The old ⌘⇧. state is the section's now.
                s.show_hidden = None;
            });
            self.tell(super::News::Settings(settings));
        }
        let view = self.settings_view();
        self.settings_window.show(view);
    }

    /// Another window's settings change (already remembered and told to the engine there).
    pub(super) fn adopt_settings(&mut self, settings: Settings) {
        if settings != self.settings {
            self.apply_settings(settings, false);
        }
    }

    /// Makes `new` the settings and carries out what changed: the theme repaints the windows,
    /// density the list (its views read [`Density::row`]), the download folder is read by the
    /// next download, the engine hears the new limit (`engine`: from this window), and the
    /// hidden-files default is the one hosts opened from now on start with (the ones open keep
    /// theirs).
    fn apply_settings(&mut self, new: Settings, engine: bool) {
        let old = std::mem::replace(&mut self.settings, new);
        let s = &self.settings;
        if old.theme != s.theme {
            self.ui.appearance = s.theme;
        }
        if old.show_hidden != s.show_hidden {
            self.show_hidden_default = s.show_hidden;
        }
        if engine && old.transfers_at_once != s.transfers_at_once {
            let n = s.transfers_at_once;
            self.send(Command::Transfer(TransferCommand::PerHost(n)));
        }
    }
}
