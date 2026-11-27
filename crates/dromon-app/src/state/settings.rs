//! Settings (§6.8, §8): theme, list density, the download folder, transfers at once per host,
//! whether hidden files show by default, the terminal app "Open in Terminal" uses, whether
//! Dromon checks for updates by itself, and live updates.
//!
//! The values live in each main window's state ([`App::settings`]), the same in all of them.
//! Each main window shows them on its Settings page (`page.rs`, `views/settings.rs`), which
//! reads them from its state and sends each change as [`Action::Setting`](super::Action::Setting).
//! A change applies there at once, is remembered in `prefs.json`
//! ([`Saved::settings`](super::Saved::settings)) and reaches the other main windows as
//! [`News::Settings`](super::News::Settings), whose pages show it too. The windows' frames follow
//! the theme through [`Peers::theme`](super::Peers::theme).

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

/// The terminal app "Open in Terminal" opens ssh in: the platform's own choice, or one of those
/// found on this machine (`crate::terminal` finds and starts them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Terminal {
    /// macOS's Terminal.app.
    #[serde(rename = "terminal")]
    TerminalApp,
    #[serde(rename = "iterm2")]
    Iterm2,
    Ghostty,
    #[serde(rename = "wezterm")]
    WezTerm,
    Kitty,
    Alacritty,
    WindowsTerminal,
    /// Windows: ssh in a console window of its own.
    Console,
    GnomeTerminal,
    Konsole,
    Foot,
    Xterm,
    /// macOS: Terminal; Windows: Windows Terminal, else a console window; Linux: `$TERMINAL`,
    /// then `x-terminal-emulator`, then the first known one installed. A name the file has that
    /// this version doesn't know reads as this too (serde's `other` is the last variant's).
    #[default]
    #[serde(other)]
    System,
}

impl Terminal {
    /// Its name on the Settings page.
    pub fn label(self) -> &'static str {
        match self {
            Terminal::System => "System default",
            Terminal::TerminalApp => "Terminal",
            Terminal::Iterm2 => "iTerm2",
            Terminal::Ghostty => "Ghostty",
            Terminal::WezTerm => "WezTerm",
            Terminal::Kitty => "kitty",
            Terminal::Alacritty => "Alacritty",
            Terminal::WindowsTerminal => "Windows Terminal",
            Terminal::Console => "Console window",
            Terminal::GnomeTerminal => "GNOME Terminal",
            Terminal::Konsole => "Konsole",
            Terminal::Foot => "foot",
            Terminal::Xterm => "XTerm",
        }
    }

    /// The key of its choice on the Settings page.
    pub fn key(self) -> &'static str {
        match self {
            Terminal::System => "terminal-system",
            Terminal::TerminalApp => "terminal-terminal",
            Terminal::Iterm2 => "terminal-iterm2",
            Terminal::Ghostty => "terminal-ghostty",
            Terminal::WezTerm => "terminal-wezterm",
            Terminal::Kitty => "terminal-kitty",
            Terminal::Alacritty => "terminal-alacritty",
            Terminal::WindowsTerminal => "terminal-windows-terminal",
            Terminal::Console => "terminal-console",
            Terminal::GnomeTerminal => "terminal-gnome-terminal",
            Terminal::Konsole => "terminal-konsole",
            Terminal::Foot => "terminal-foot",
            Terminal::Xterm => "terminal-xterm",
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
    /// Where "Open in Terminal" opens ssh.
    pub terminal: Terminal,
    /// Check GitHub for a newer Dromon at launch and once a day (`update.rs`).
    pub check_updates: bool,
    /// Watch the folder on screen live where the server can (`service/live.rs`); off: polls only.
    pub live_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Appearance::System,
            density: Density::Comfortable,
            download_folder: None,
            transfers_at_once: AT_ONCE_DEFAULT,
            show_hidden: true,
            terminal: Terminal::System,
            check_updates: true,
            live_updates: true,
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
            SettingsChange::Terminal(terminal) => s.terminal = terminal,
            SettingsChange::CheckUpdates(on) => s.check_updates = on,
            SettingsChange::LiveUpdates(on) => s.live_updates = on,
        }
        s.clamped()
    }
}

/// One control of the Settings page, set to a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsChange {
    Theme(Appearance),
    Density(Density),
    /// "Choose…" picked a folder; `None` is "Reset" (the OS Downloads folder).
    DownloadFolder(Option<PathBuf>),
    TransfersAtOnce(usize),
    ShowHidden(bool),
    Terminal(Terminal),
    CheckUpdates(bool),
    LiveUpdates(bool),
}

/// What the Settings page shows: the settings, and the folder "default" stands for.
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

    /// The download folder as the page writes it: under the home folder as `~/…`.
    pub fn folder_label(&self, home: Option<&Path>) -> String {
        let folder = self.folder();
        match home.and_then(|h| folder.strip_prefix(h).ok()) {
            Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
            None => folder.display().to_string(),
        }
    }
}

impl App {
    /// What the Settings page shows now.
    pub fn settings_view(&self) -> SettingsView {
        SettingsView { settings: self.settings.clone(), default_folder: self.downloads.clone() }
    }

    /// Where a download goes now: the folder chosen in Settings, else the OS Downloads folder.
    pub fn download_folder(&self) -> PathBuf {
        self.settings.download_folder.clone().unwrap_or_else(|| self.downloads.clone())
    }

    /// The settings remembered last time (`remember`). Prefs from before Settings existed have
    /// no section: the defaults, with ⌘⇧.'s last state (which used to be remembered) as the
    /// hidden-files default. The service hears whether to check for updates by itself: the
    /// launch check comes from here, so only a run that remembers its settings (the app, never
    /// a headless run or a test on `NoStore` without `remember`) checks.
    pub(super) fn restore_settings(&mut self) {
        let saved = self.saved().settings.clone();
        let settings = match saved {
            Some(s) => s.clamped(),
            None => Settings { show_hidden: self.saved().show_hidden.unwrap_or(true), ..Settings::default() },
        };
        self.apply_settings(settings, true);
        self.auto_updates(self.settings.check_updates);
    }

    /// A control on the Settings page: applied at once, remembered, and told to the other main
    /// windows (which apply it too). The page reads the values back from the state.
    pub(super) fn change_setting(&mut self, change: SettingsChange) {
        self.ui.settings_page.error = None;
        let new = self.settings.with(change);
        if new != self.settings {
            let theme = self.settings.theme;
            // The service's updater is one for every window: told once, from here.
            if new.check_updates != self.settings.check_updates {
                self.auto_updates(new.check_updates);
            }
            self.apply_settings(new, true);
            let settings = self.settings.clone();
            self.save(|s| {
                s.settings = Some(settings.clone());
                // The old ⌘⇧. state is the section's now.
                s.show_hidden = None;
            });
            // The windows' own frames (title bars, the macOS appearance) follow the theme too.
            if settings.theme != theme {
                self.common.peers().theme(settings.theme);
            }
            self.tell(super::News::Settings(settings));
        }
    }

    /// Another window's settings change (already remembered and told to the engine there).
    pub(super) fn adopt_settings(&mut self, settings: Settings) {
        if settings != self.settings {
            self.apply_settings(settings, false);
        }
    }

    /// Makes `new` the settings and carries out what changed: the theme repaints the windows,
    /// density the list (its views read [`Density::row`]), the download folder is read by the
    /// next download, the engine hears the new limit and the service whether to watch folders
    /// live (`engine`: from this window), and the hidden-files default is the one hosts opened
    /// from now on start with (the ones open keep theirs).
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
        let on = self.settings.live_updates;
        if engine && old.live_updates != on {
            self.send(Command::LiveUpdates { on });
        }
    }
}
