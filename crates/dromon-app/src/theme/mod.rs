//! Design tokens from the plan's §6: colour roles (§6.3), type (§6.4), spacing, radii and
//! elevation (§6.5) and motion (§6.6). Views read colours from [`Tokens`] only, never from
//! literals, so the light theme, a different accent and the accessibility variants (§6.9,
//! [`Access`]) are one switch away.

mod fonts;
pub mod motion;
pub mod type_;

#[cfg(test)]
#[path = "contrast_tests.rs"]
mod contrast_tests;

pub use fonts::{FAMILIES, FONTS};

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use rune::Color;
use rune::color::hex;

/// One accent, defined once for both appearances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accent {
    /// The letter the comparison uses (`--accent a|b|c`, `scripts/accents.sh`).
    pub id: &'static str,
    pub name: &'static str,
    pub dark: &'static str,
    pub light: &'static str,
}

/// The three M4 candidates (§6.3). The light values are darker than the plan's first draft
/// (`#0E9F6E`, `#0F8F85`, `#13896F`): lowered in OKLCH lightness (by 0.093, 0.053 and 0.032,
/// red clipped to the sRGB gamut) until white labels on the accent, accent text on a hovered
/// popover row and folder glyphs on a pressed row pass (`contrast_tests.rs`).
pub const SPEARMINT: Accent = Accent { id: "a", name: "Spearmint", dark: "#3DDC97", light: "#008051" };
pub const TEAL: Accent = Accent { id: "b", name: "Teal", dark: "#2DD4BF", light: "#007E74" };
pub const SEAFOAM: Accent = Accent { id: "c", name: "Seafoam", dark: "#6EE7C8", light: "#007F66" };
pub const ACCENTS: [Accent; 3] = [SPEARMINT, TEAL, SEAFOAM];

/// The default accent, Spearmint (A), until the user picks one of [`ACCENTS`].
pub const ACCENT: Accent = SPEARMINT;

/// Index into [`ACCENTS`] of the accent this process paints with (`--accent`).
static CHOSEN: AtomicUsize = AtomicUsize::new(0);

/// Paints with the accent named `id` (`a`, `b`, `c`, or its name) from now on: the comparison
/// renders (`--accent`, `scripts/accents.sh`). Unknown names are an error naming the choices.
pub fn choose_accent(id: &str) -> Result<Accent, String> {
    let i = ACCENTS
        .iter()
        .position(|a| a.id.eq_ignore_ascii_case(id) || a.name.eq_ignore_ascii_case(id))
        .ok_or_else(|| format!("--accent takes a, b or c (Spearmint, Teal, Seafoam), not {id:?}"))?;
    CHOSEN.store(i, Ordering::Relaxed);
    Ok(ACCENTS[i])
}

/// The accent this process paints with: [`ACCENT`] unless `--accent` chose another.
pub fn accent() -> Accent {
    ACCENTS[CHOSEN.load(Ordering::Relaxed).min(ACCENTS.len() - 1)]
}

/// The OS display preferences the views follow (§6.6, §6.9), in one place. `views::view`
/// reads them from Rune each time the view runs (so a change in the system settings applies
/// live), ORs in what a scene or test forces on (`UiState::access`), and hands them to every
/// view in `Paint::access`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Access {
    /// Reduce motion (§6.6): slides and scales become a short opacity cross-fade and `settle`
    /// loses its bounce. Rune applies it to every transition, spring, sequence and loop of a
    /// commit that reduces motion; views read this flag for motion they choose themselves
    /// ([`motion::settle`]).
    pub reduce_motion: bool,
    /// Reduce transparency (§6.5): surfaces are opaque ([`Access::solid`]).
    pub reduce_transparency: bool,
    /// Increase contrast (§6.9): stronger borders and opaque surfaces.
    pub increase_contrast: bool,
}

impl Access {
    /// Every preference either side has on.
    pub fn or(self, o: Access) -> Access {
        Access {
            reduce_motion: self.reduce_motion || o.reduce_motion,
            reduce_transparency: self.reduce_transparency || o.reduce_transparency,
            increase_contrast: self.increase_contrast || o.increase_contrast,
        }
    }

    /// Whether filled surfaces are opaque: reduce transparency, or increased contrast.
    pub fn solid(self) -> bool {
        self.reduce_transparency || self.increase_contrast
    }
}

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
    /// Folder glyphs: the accent at 70% (80% in light).
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
    /// The fill of controls that hold something: text fields, the segmented control's track,
    /// the transfer rows' glyph tiles, the list states' buttons. The `hover` wash, or an opaque
    /// grey when surfaces are solid ([`Access::solid`]).
    pub field: Color,
    /// The reconnecting banner's wash: the warning at 7%, or that wash made opaque over the
    /// canvas when surfaces are solid.
    pub banner: Color,
}

impl Tokens {
    /// The hairline round a popover, menu, sheet or toast (§6.5): `border` in dark; in light
    /// the shadow carries the separation and the edge drops to `border-subtle`.
    pub fn elevated_edge(&self) -> Color {
        if self.dark { self.border } else { self.border_subtle }
    }
}

fn c(s: &str) -> Color {
    hex(s).unwrap_or_else(|| panic!("bad colour token {s}"))
}

/// `top` drawn over the opaque `bottom` as the renderer blends (in sRGB), as one opaque colour.
pub fn flatten(top: Color, bottom: Color) -> Color {
    let enc = rune::color::srgb_encode;
    let dec = rune::color::srgb_decode;
    let m = |a: f32, b: f32| dec(enc(b) + (enc(a) - enc(b)) * top.a);
    Color { r: m(top.r, bottom.r), g: m(top.g, bottom.g), b: m(top.b, bottom.b), a: 1.0 }
}

/// The tokens of one appearance with `accent`, adjusted for `access` (§6.9): increased
/// contrast strengthens every border a step and a half, and solid surfaces make the washes
/// that something sits on opaque.
pub fn build(dark: bool, accent: Accent, access: Access) -> Tokens {
    let mut t = if dark { dark_tokens(accent) } else { light_tokens(accent) };
    if access.increase_contrast {
        let ink = if dark { Color::WHITE } else { Color::BLACK };
        let (subtle, border, strong) = if dark { (0.16, 0.24, 0.34) } else { (0.16, 0.26, 0.38) };
        t.border_subtle = ink.alpha(subtle);
        t.border = ink.alpha(border);
        t.border_strong = ink.alpha(strong);
    }
    if access.solid() {
        t.field = if dark { c("#252B2B") } else { c("#EFF2F1") };
        t.banner = flatten(t.banner, t.canvas);
    }
    t
}

fn dark_tokens(accent: Accent) -> Tokens {
    let a = c(accent.dark);
    let white = Color::WHITE;
    let warning = c("#F5B94A");
    Tokens {
        dark: true,
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
        // Contrast audit (`contrast_tests.rs`), OKLCH lightness only: text-2 +0.019 (was
        // #A3ABA9), text-3 +0.095 (was #6B7472), danger +0.006 (was #FF6B6B), so dates and
        // hidden files on a selected, pressed row and Delete in a pressed menu still read.
        text_2: c("#A9B1AF"),
        text_3: c("#87908E"),
        hover: white.alpha(0.04),
        press: white.alpha(0.07),
        selection: a.alpha(0.16),
        focus: a,
        danger: c("#FF6F6F"),
        warning,
        folder: a.alpha(0.7),
        skeleton: white.alpha(0.06),
        active: white.alpha(0.06),
        shadow: Color::BLACK.alpha(0.45),
        sheet_shadow: Color::BLACK.alpha(0.55),
        scrim: Color::BLACK.alpha(0.32),
        on_accent: c("#0C0E0E"),
        field: white.alpha(0.04),
        banner: warning.alpha(0.07),
    }
}

fn light_tokens(accent: Accent) -> Tokens {
    let a = c(accent.light);
    let black = Color::BLACK;
    // Contrast audit (`contrast_tests.rs`), OKLCH lightness only: warning −0.053 (was
    // #B7791F), text-2 −0.025 (was #5B6563), text-3 −0.091 (was #8C9593), danger −0.057 (was
    // #D64545).
    let warning = c("#A66900");
    Tokens {
        dark: false,
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
        text_2: c("#545E5C"),
        text_3: c("#717A78"),
        hover: black.alpha(0.035),
        press: black.alpha(0.06),
        selection: a.alpha(0.12),
        focus: a,
        danger: c("#C23034"),
        warning,
        folder: a.alpha(0.8),
        skeleton: black.alpha(0.06),
        active: black.alpha(0.055),
        shadow: black.alpha(0.12),
        sheet_shadow: black.alpha(0.18),
        scrim: black.alpha(0.12),
        on_accent: c("#FFFFFF"),
        field: black.alpha(0.035),
        banner: warning.alpha(0.07),
    }
}

/// Every combination [`tokens_for`] hands out, built once: accent × appearance × increased
/// contrast × solid.
fn all() -> &'static [Tokens] {
    static ALL: OnceLock<Vec<Tokens>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v = Vec::with_capacity(ACCENTS.len() * 8);
        for accent in ACCENTS {
            for dark in [false, true] {
                for increase_contrast in [false, true] {
                    for reduce_transparency in [false, true] {
                        v.push(build(dark, accent, Access { reduce_motion: false, reduce_transparency, increase_contrast }));
                    }
                }
            }
        }
        v
    })
}

/// The tokens for one appearance under `access`, in the chosen accent ([`accent`]).
pub fn tokens_for(dark: bool, access: Access) -> &'static Tokens {
    let i = CHOSEN.load(Ordering::Relaxed).min(ACCENTS.len() - 1);
    &all()[i * 8 + usize::from(dark) * 4 + usize::from(access.increase_contrast) * 2 + usize::from(access.reduce_transparency)]
}

/// The tokens for one appearance, with no accessibility preference on.
pub fn tokens(dark: bool) -> &'static Tokens {
    tokens_for(dark, Access::default())
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
        assert_eq!(tokens(false).accent.hex(), "#008051ff");
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

    #[test]
    fn accents_are_picked_by_letter_or_name() {
        assert_eq!(ACCENT, SPEARMINT, "Spearmint stays the default until the user picks");
        assert_eq!(ACCENTS.map(|a| a.id), ["a", "b", "c"]);
        assert!(choose_accent("d").is_err());
        // The other lookups leave the process on the default.
        assert_eq!(choose_accent("A"), Ok(SPEARMINT));
        assert_eq!(accent(), SPEARMINT);
    }

    #[test]
    fn access_variants_are_cached_and_change_only_what_they_name() {
        for dark in [false, true] {
            let plain = tokens_for(dark, Access::default());
            let more = tokens_for(dark, Access { increase_contrast: true, ..Access::default() });
            let solid = tokens_for(dark, Access { reduce_transparency: true, ..Access::default() });
            let motion = tokens_for(dark, Access { reduce_motion: true, ..Access::default() });
            assert!(std::ptr::eq(plain, motion), "reduce motion changes no colour");
            assert!(more.border.a > plain.border_strong.a, "increased contrast: border is past border-strong");
            assert!(more.border_subtle.a >= plain.border.a);
            assert_eq!((more.field.a, more.banner.a), (1.0, 1.0), "increased contrast is solid too");
            assert_eq!((solid.field.a, solid.banner.a), (1.0, 1.0));
            assert_eq!(solid.border, plain.border, "reduce transparency keeps the borders");
            assert_eq!(plain.field, plain.hover);
            assert!(plain.banner.a < 1.0);
        }
    }
}
