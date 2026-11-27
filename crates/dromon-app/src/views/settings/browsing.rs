//! The Settings page's Browsing section: Show hidden files by default, a switch.

use rune::El;

use super::{section, setting, sub, switch};
use crate::state::{SettingsChange, SettingsView};
use crate::views::Paint;
use crate::views::widgets::chord;

pub(super) fn browsing(v: &SettingsView, p: Paint) -> El {
    let hidden_sub = format!("For servers opened from now on; {} switches them", chord("mod+shift+period"));
    let hidden =
        switch(p, "hidden-toggle", "Show hidden files by default", v.settings.show_hidden, SettingsChange::ShowHidden);
    section(p, "Browsing", vec![setting(p, "Show hidden files by default", Some(sub(p, &hidden_sub)), hidden)])
}
