//! The brand kit in the app (`assets/brand/`, rebuilt by `cargo brand`): the app icon the
//! window and taskbar show on Windows and Linux, and the marks the views draw: the simple mark
//! beside the app's name in the sidebar band, the full mark on the Welcome screen. Each is the
//! kit's own file, never a redrawing, in its colour for the theme (BRAND.md: mint on dark, deep
//! mint on light). `cargo brand` copies them into this crate's `assets/brand/`, since `cargo dev`
//! builds a copy of the crates alone, and a test checks they still match the kit.
//!
//! A mark rasterizes from its SVG on Rune's decoder workers, once per theme and device scale,
//! and stays for the run (a few small images). Its pixels wake the windows that show them once
//! they're ready, so the view needn't run again. Headless runs and tests wait for the decode
//! instead, so a snapshot is the same on every run.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use rune::{El, Image, ImageCache, SurfaceFit, el};

use super::Paint;

#[cfg(test)]
#[path = "brand_tests.rs"]
mod tests;

/// The app's icon while it runs on Windows and Linux (`rune::set_app_icon_choice`): the kit's
/// 256 px app icon. macOS keeps the bundle's `Dromon.icns`.
#[cfg(any(not(target_os = "macos"), test))]
pub const APP_ICON: &[u8] = include_bytes!("../../assets/brand/dromon-256.png");

/// A mark of the brand kit, drawn at a fixed size in points: the kit's 3:2 drawing on whole
/// points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    /// The simple mark (four oars, no seams or shields), which stays crisp at 48 px and less.
    Simple,
    /// The full mark, never under 120 px wide.
    Full,
}

impl Mark {
    /// Width and height in points.
    pub fn size(self) -> (f32, f32) {
        match self {
            Mark::Simple => (27.0, 18.0),
            Mark::Full => (144.0, 96.0),
        }
    }

    /// The kit's SVG of it for a dark or a light theme.
    fn svg(self, dark: bool) -> &'static [u8] {
        match (self, dark) {
            (Mark::Simple, true) => include_bytes!("../../assets/brand/dromon-mark-simple.svg"),
            (Mark::Simple, false) => include_bytes!("../../assets/brand/dromon-mark-simple-on-light.svg"),
            (Mark::Full, true) => include_bytes!("../../assets/brand/dromon-mark.svg"),
            (Mark::Full, false) => include_bytes!("../../assets/brand/dromon-mark-on-light.svg"),
        }
    }
}

/// Rune's decoder for the marks, and every mark rasterized so far by mark, theme (dark) and
/// device scale (its bits). No waker: an image's pixels wake the windows that show it.
struct Marks {
    cache: ImageCache,
    drawn: Mutex<HashMap<(Mark, bool, u32), Image>>,
}

static MARKS: LazyLock<Marks> =
    LazyLock::new(|| Marks { cache: ImageCache::new(Arc::new(|| {})), drawn: Mutex::new(HashMap::new()) });

/// `mark` for a dark or light theme at `scale` device pixels per point, rasterized at the size
/// it draws at. Headless runs and tests wait until it's decoded; the app never does.
fn image(mark: Mark, dark: bool, scale: f32) -> Image {
    let image = {
        let mut drawn = MARKS.drawn.lock().unwrap_or_else(|e| e.into_inner());
        drawn
            .entry((mark, dark, scale.to_bits()))
            .or_insert_with(|| MARKS.cache.svg(mark.svg(dark), mark.size().0, scale))
            .clone()
    };
    if cfg!(test) || rune::tooling::headless_run().is_some() {
        let _ = image.wait();
    }
    image
}

/// `mark` in the theme's colour, keyed `key`: decoration, hidden from screen readers.
pub fn mark(key: &'static str, mark: Mark, p: Paint) -> El {
    let (w, h) = mark.size();
    let image = image(mark, p.t.dark, p.scale);
    el().key(key).size(w, h).a11y_hidden().pixels_fit(image.pixels(), SurfaceFit::Fit)
}
