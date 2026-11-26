//! Scenes of Settings (§6.8, §8): the list at Compact density, and the Settings page in the
//! main window, on Appearance and on Transfers. Paths show whole (no `~`, no home folder in a
//! scene), so the pictures are the same for every user.

use std::path::PathBuf;

use crate::state::{Action, App, Density, SettingsChange, SettingsSection};
use crate::theme::Appearance;

fn set(s: &mut App, change: SettingsChange) {
    s.act(Action::Setting(change));
}

/// The listing scene with 24 pt rows.
pub fn listing_compact(s: &mut App) {
    super::listing(s);
    set(s, SettingsChange::Density(Density::Compact));
}

/// The Settings page at its defaults (§8), on Appearance, from the welcome state.
pub fn settings(s: &mut App) {
    super::welcome(s);
    s.downloads = PathBuf::from("/home/me/Downloads");
    s.act(Action::OpenSettings);
}

/// Every setting off its default, in the light theme, from the compact listing it makes: a
/// chosen folder, 5 transfers at once, hidden files off; the page on Appearance.
pub fn settings_light(s: &mut App) {
    listing_compact(s);
    s.downloads = PathBuf::from("/home/me/Downloads");
    set(s, SettingsChange::Theme(Appearance::Light));
    set(s, SettingsChange::DownloadFolder(Some(PathBuf::from("/srv/backups/incoming"))));
    set(s, SettingsChange::TransfersAtOnce(5));
    set(s, SettingsChange::ShowHidden(false));
    s.act(Action::OpenSettings);
}

/// The page on Transfers at the defaults (the Downloads folder, 3 at once), opened from the
/// listing, which waits under it.
pub fn settings_transfers(s: &mut App) {
    super::listing(s);
    s.downloads = PathBuf::from("/home/me/Downloads");
    s.act(Action::OpenSettings);
    s.act(Action::SettingsSection(SettingsSection::Transfers));
}
