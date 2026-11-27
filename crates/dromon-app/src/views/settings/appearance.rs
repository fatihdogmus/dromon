//! The Settings page's Appearance section: Theme and Density, segmented.

use rune::El;

use super::{section, segmented, setting, sub};
use crate::state::{Density, SettingsChange, SettingsView};
use crate::theme::Appearance;
use crate::views::Paint;

pub(super) fn appearance(v: &SettingsView, p: Paint) -> El {
    let s = &v.settings;
    let theme_control = segmented(
        p,
        "theme",
        "Theme",
        72.0,
        &[
            (Appearance::System, "System", "theme-system"),
            (Appearance::Dark, "Dark", "theme-dark"),
            (Appearance::Light, "Light", "theme-light"),
        ],
        s.theme,
        SettingsChange::Theme,
    );
    let density_control = segmented(
        p,
        "density",
        "Density",
        100.0,
        &[
            (Density::Comfortable, "Comfortable", "density-comfortable"),
            (Density::Compact, "Compact", "density-compact"),
        ],
        s.density,
        SettingsChange::Density,
    );
    section(
        p,
        "Appearance",
        vec![
            setting(p, "Theme", None, theme_control),
            setting(p, "Density", Some(sub(p, "How tall the rows of the file list are")), density_control),
        ],
    )
}
