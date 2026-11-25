//! Design tokens from the plan's §6: colour roles (§6.3), type (§6.4), spacing, radii and
//! elevation (§6.5) and motion (§6.6). Views read colours from [`Tokens`] only, never from
//! literals, so the light theme and a different accent are one switch away.

mod fonts;
pub mod motion;
pub mod type_;

pub use fonts::{FAMILIES, FONTS};

use std::sync::OnceLock;

use rune::Color;
use rune::color::hex;

/// One accent, defined once for both appearances. Swapping the brand colour is changing
/// [`ACCENT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accent {
    pub name: &'static str,
    pub dark: &'static str,
    pub light: &'static str,
}

/// Variant A, Spearmint. B (`#2DD4BF` / `#0F8F85`) and C (`#6EE7C8` / `#13896F`) are the other
/// candidates, picked in M4.
pub const ACCENT: Accent = Accent { name: "Spearmint", dark: "#3DDC97", light: "#0E9F6E" };

/// Spacing on the 4 pt grid.
pub mod space {
    pub const S1: f32 = 4.0;
    pub const S2: f32 = 8.0;
    pub const S3: f32 = 12.0;
    pub const S4: f32 = 16.0;
    pub const S6: f32 = 24.0;
    pub const S8: f32 = 32.0;
}

/// Corner radii (§6.5).
pub mod radius {
    /// Rows, buttons and fields.
    pub const ROW: f32 = 6.0;
    /// Popovers and menus.
    pub const POPOVER: f32 = 10.0;
    /// Sheets, dialogs, the palette.
    pub const SHEET: f32 = 12.0;
}

/// Fixed metrics of the single-window layout (§6.2).
pub mod size {
    pub const TOOLBAR: f32 = 44.0;
    pub const HEADER: f32 = 28.0;
    pub const ROW: f32 = 28.0;
    pub const STATUS: f32 = 24.0;
    pub const BANNER: f32 = 32.0;
    pub const SIDEBAR: f32 = 240.0;
    pub const SIDEBAR_MIN: f32 = 200.0;
    pub const SIDEBAR_MAX: f32 = 320.0;
    pub const ICON: f32 = 16.0;
    /// The Size and Modified columns; Name takes the rest.
    pub const COL_SIZE: f32 = 88.0;
    pub const COL_MODIFIED: f32 = 132.0;
}

/// The semantic colour roles of one appearance.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub dark: bool,
    pub accent: Color,
    pub canvas: Color,
    pub sidebar: Color,
    pub surface: Color,
    pub raised: Color,
    pub sheet: Color,
    pub border_subtle: Color,
    pub border: Color,
    pub border_strong: Color,
    pub text: Color,
    pub text_2: Color,
    pub text_3: Color,
    pub hover: Color,
    pub press: Color,
    pub selection: Color,
    pub focus: Color,
    pub danger: Color,
    pub warning: Color,
    /// Folder glyphs: the accent at 70%.
    pub folder: Color,
    /// Skeleton bars while a listing loads.
    pub skeleton: Color,
    /// The sidebar row of the host on screen (neutral, so the accent stays rare).
    pub active: Color,
    pub shadow: Color,
    /// Sheets sit deeper than popovers (§6.5: `0 16 48`).
    pub sheet_shadow: Color,
    /// The veil over the window behind a sheet.
    pub scrim: Color,
    /// Text and glyphs on an accent fill (primary buttons).
    pub on_accent: Color,
}

fn c(s: &str) -> Color {
    hex(s).unwrap_or_else(|| panic!("bad colour token {s}"))
}

fn build(dark: bool, accent: Accent) -> Tokens {
    if dark {
        let a = c(accent.dark);
        let white = Color::WHITE;
        Tokens {
            dark,
            accent: a,
            canvas: c("#0C0E0E"),
            sidebar: c("#101313"),
            surface: c("#141717"),
            raised: c("#1A1E1E"),
            sheet: c("#1F2424"),
            border_subtle: white.alpha(0.06),
            border: white.alpha(0.09),
            border_strong: white.alpha(0.14),
            text: c("#ECEFEE"),
            text_2: c("#A3ABA9"),
            text_3: c("#6B7472"),
            hover: white.alpha(0.04),
            press: white.alpha(0.07),
            selection: a.alpha(0.16),
            focus: a,
            danger: c("#FF6B6B"),
            warning: c("#F5B94A"),
            folder: a.alpha(0.7),
            skeleton: white.alpha(0.06),
            active: white.alpha(0.06),
            shadow: Color::BLACK.alpha(0.45),
            sheet_shadow: Color::BLACK.alpha(0.55),
            scrim: Color::BLACK.alpha(0.32),
            on_accent: c("#0C0E0E"),
        }
    } else {
        let a = c(accent.light);
        let black = Color::BLACK;
        Tokens {
            dark,
            accent: a,
            canvas: c("#FFFFFF"),
            sidebar: c("#F5F7F6"),
            surface: c("#FAFBFB"),
            raised: c("#FFFFFF"),
            sheet: c("#FFFFFF"),
            border_subtle: black.alpha(0.06),
            border: black.alpha(0.09),
            border_strong: black.alpha(0.14),
            text: c("#111515"),
            text_2: c("#5B6563"),
            text_3: c("#8C9593"),
            hover: black.alpha(0.035),
            press: black.alpha(0.06),
            selection: a.alpha(0.12),
            focus: a,
            danger: c("#D64545"),
            warning: c("#B7791F"),
            folder: a.alpha(0.8),
            skeleton: black.alpha(0.06),
            active: black.alpha(0.055),
            shadow: black.alpha(0.12),
            sheet_shadow: black.alpha(0.18),
            scrim: black.alpha(0.12),
            on_accent: c("#FFFFFF"),
        }
    }
}

/// The tokens for one appearance, built once.
pub fn tokens(dark: bool) -> &'static Tokens {
    static DARK: OnceLock<Tokens> = OnceLock::new();
    static LIGHT: OnceLock<Tokens> = OnceLock::new();
    if dark { DARK.get_or_init(|| build(true, ACCENT)) } else { LIGHT.get_or_init(|| build(false, ACCENT)) }
}

/// Which appearance the user asked for (§8: System by default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Dark,
    Light,
}

impl Appearance {
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Appearance::System => system_dark,
            Appearance::Dark => true,
            Appearance::Light => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_is_one_token_in_both_themes() {
        assert_eq!(tokens(true).accent.hex(), "#3ddc97ff");
        assert_eq!(tokens(false).accent.hex(), "#0e9f6eff");
        assert_eq!(tokens(true).focus, tokens(true).accent);
        assert!((tokens(true).selection.a - 0.16).abs() < 1e-6);
        assert!((tokens(false).selection.a - 0.12).abs() < 1e-6);
    }

    #[test]
    fn appearance_override_beats_the_system() {
        assert!(Appearance::System.is_dark(true));
        assert!(!Appearance::Light.is_dark(true));
        assert!(Appearance::Dark.is_dark(false));
    }
}
