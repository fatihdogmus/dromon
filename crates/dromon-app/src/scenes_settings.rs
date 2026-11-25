//! Scenes of Settings (§6.8, §8): the list at Compact density, and the Settings window. A
//! headless run renders no second window, so these draw the window's view in the main window
//! (`UiState::settings_preview`, `views::settings::preview`) at its real size.

use std::path::PathBuf;

use crate::state::{Action, App, Density, SettingsChange};
use crate::theme::Appearance;

fn set(s: &mut App, change: SettingsChange) {
    s.act(Action::Setting(change));
}

/// The listing scene with 24 pt rows.
pub fn listing_compact(s: &mut App) {
    super::listing(s);
    set(s, SettingsChange::Density(Density::Compact));
}

/// The Settings window at its defaults (§8), over the welcome state.
pub fn settings(s: &mut App) {
    super::welcome(s);
    s.downloads = PathBuf::from("/home/me/Downloads");
    s.ui.settings_preview = true;
}

/// Every setting off its default, in the light theme, over the compact listing it makes: a
/// chosen folder (with Reset), 5 transfers at once, hidden files off.
pub fn settings_light(s: &mut App) {
    listing_compact(s);
    s.downloads = PathBuf::from("/home/me/Downloads");
    set(s, SettingsChange::Theme(Appearance::Light));
    set(s, SettingsChange::DownloadFolder(Some(PathBuf::from("/srv/backups/incoming"))));
    set(s, SettingsChange::TransfersAtOnce(5));
    set(s, SettingsChange::ShowHidden(false));
    s.ui.settings_preview = true;
}
