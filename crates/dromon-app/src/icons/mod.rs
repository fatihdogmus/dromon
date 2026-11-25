//! Dromon's icon set (§6.7), drawn on a 16 pt grid. Every icon has two bodies, because a stroke
//! can't be pixel-exact at both scales:
//!
//! - **1× bodies**: 1 pt strokes whose straight horizontal and vertical centre lines sit on
//!   half-pixels (`x.5`), so each is exactly 1 device pixel wide.
//! - **2× bodies** (scale ≥ 1.5): 1.5 pt strokes whose straight centre lines sit on
//!   quarter-pixels (`x.25` / `x.75`), so each is exactly 3 device pixels wide with both edges
//!   on pixel boundaries.
//!
//! Straight edges are pixel-exact at both scales, verified by a pixel test (`tests.rs`);
//! diagonals, curves and dots can't be and are drawn to match. Views pick the body with
//! `IconPair::at(cx.env().scale)` and place the 16 pt element on whole points, which Rune
//! rasterises into the glyph atlas at the device scale and tints with the element's colour
//! (`icon(FOLDER.at(scale), 16.0).color(..)`). The two bodies are named apart (`…@1x`, `…@2x`)
//! because the name keys the atlas.
//!
//! The source of truth is these SVG bodies; `assets/icons/` will hold exported files later.

use rune::Icon;

#[cfg(test)]
mod tests;

/// An icon drawn for both device scales.
#[derive(Debug)]
pub struct IconPair {
    pub x1: Icon,
    pub x2: Icon,
}

impl IconPair {
    /// The body for a device scale: the 2× one from 1.5 up.
    pub fn at(&self, scale: f32) -> &Icon {
        if scale < 1.5 { &self.x1 } else { &self.x2 }
    }
}

/// Wraps path data in the set's stroke style at `$width` pt.
macro_rules! stroked {
    ($name:expr, $view:literal, $width:literal, $body:expr) => {
        Icon {
            name: $name,
            view: $view,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width=""#,
                $width,
                r#"" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        }
    };
}

/// A 16 pt icon's pair: the 1 pt body, then the 1.5 pt body.
macro_rules! pair {
    ($name:literal, $b1:expr, $b2:expr) => {
        IconPair {
            x1: stroked!(concat!($name, "@1x"), 16.0, "1", $b1),
            x2: stroked!(concat!($name, "@2x"), 16.0, "1.5", $b2),
        }
    };
}

/// The page outline and folded corner shared by the file icons.
macro_rules! page_1x {
    () => {
        r#"<path d="M3.5 1.5H9.5L12.5 4.5V14.5H3.5V1.5Z"/><path d="M9.5 1.5V4.5H12.5"/>"#
    };
}
macro_rules! page_2x {
    () => {
        r#"<path d="M3.25 1.25H9.75L12.75 4.25V14.75H3.25V1.25Z"/><path d="M9.75 1.25V4.25H12.75"/>"#
    };
}

pub static FOLDER: IconPair = pair!(
    "dromon-folder",
    r#"<path d="M2.5 12.5V3.5H6.25L8.25 5.5H13.5V12.5H2.5Z" fill="currentColor" fill-opacity="0.16"/>"#,
    r#"<path d="M2.25 12.75V3.25H6.25L7.75 4.75H13.75V12.75H2.25Z" fill="currentColor" fill-opacity="0.16"/>"#
);
pub static FOLDER_OPEN: IconPair = pair!(
    "dromon-folder-open",
    r#"<path d="M2.5 12.5V3.5H6.25L8.25 5.5H12.5V7.5M2.5 12.5L4.5 7.5H14.5L12.5 12.5H2.5Z"/>"#,
    r#"<path d="M2.25 12.75V3.25H6.25L7.75 4.75H12.75V7.25M2.25 12.75L4.25 7.25H14.75L12.75 12.75H2.25Z"/>"#
);
pub static FILE: IconPair = pair!("dromon-file", page_1x!(), page_2x!());
pub static FILE_CODE: IconPair = pair!(
    "dromon-file-code",
    concat!(page_1x!(), r#"<path d="M6.5 8L5.25 9.5L6.5 11M9.5 8L10.75 9.5L9.5 11"/>"#),
    concat!(page_2x!(), r#"<path d="M6.5 8L5.25 9.5L6.5 11M9.5 8L10.75 9.5L9.5 11"/>"#)
);
pub static FILE_TEXT: IconPair = pair!(
    "dromon-file-text",
    concat!(page_1x!(), r#"<path d="M6 8.5H10M6 10.5H10M6 12.5H8"/>"#),
    concat!(page_2x!(), r#"<path d="M6 7.75H10M6 10.25H10M6 12.75H8"/>"#)
);
pub static FILE_IMAGE: IconPair = pair!(
    "dromon-file-image",
    concat!(page_1x!(), r#"<path d="M3.5 12.5L6.5 9.5L8.5 11.5L9.5 10.5L12.5 13.5"/>"#),
    concat!(page_2x!(), r#"<path d="M3.25 12.75L6.5 9.5L8.5 11.5L9.5 10.5L12.75 13.75"/>"#)
);
pub static FILE_ARCHIVE: IconPair = pair!(
    "dromon-file-archive",
    concat!(page_1x!(), r#"<path d="M7.5 3.5V4.5M7.5 6.5V7.5M7.5 9.5V12.5H8.5"/>"#),
    concat!(page_2x!(), r#"<path d="M7.75 3.5V4.5M7.75 6.5V7.5M7.75 9.5V12.75H8.75"/>"#)
);
pub static FILE_BINARY: IconPair = pair!(
    "dromon-file-binary",
    concat!(page_1x!(), r#"<path d="M6 8.5L7.5 10L6 11.5M8.5 11.5H10"/>"#),
    concat!(page_2x!(), r#"<path d="M6 8.75L7.5 10.25L6 11.75M8.5 11.75H10"/>"#)
);
/// A music note: an eighth note whose stem (a straight vertical edge) is tangent to its head.
pub static FILE_AUDIO: IconPair = pair!(
    "dromon-file-audio",
    concat!(page_1x!(), r#"<circle cx="7" cy="11.5" r="1.5"/><path d="M8.5 11.5V7.5L10.5 9"/>"#),
    concat!(page_2x!(), r#"<circle cx="7.25" cy="11.5" r="1.5"/><path d="M8.75 11.5V7.5L10.5 9"/>"#)
);
/// A play triangle whose upright edge is pixel-exact, like `RESUME`'s.
pub static FILE_VIDEO: IconPair = pair!(
    "dromon-file-video",
    concat!(page_1x!(), r#"<path d="M6.5 7.5V12.5L10.5 10L6.5 7.5Z"/>"#),
    concat!(page_2x!(), r#"<path d="M6.25 7.5V12.5L10.5 10L6.25 7.5Z"/>"#)
);
/// A title line over a heavy bar (2 pt at 1×, 3 pt at 2×, centred on a whole point so both edges sit on pixels).
pub static FILE_PDF: IconPair = pair!(
    "dromon-file-pdf",
    concat!(page_1x!(), r#"<path d="M6 8.5H8"/><path d="M6 11H10" stroke-width="2"/>"#),
    concat!(page_2x!(), r#"<path d="M6 7.75H8"/><path d="M6 11H10" stroke-width="3"/>"#)
);
/// The connection log: a window of lines, each a timestamp tick and a message.
pub static LOG: IconPair = pair!(
    "dromon-log",
    r#"<rect x="2.5" y="3.5" width="11" height="10" rx="1.5"/><path d="M5.5 6.5H6.5M8.5 6.5H11M5.5 8.5H6.5M8.5 8.5H10.5M5.5 10.5H6.5M8.5 10.5H10"/>"#,
    r#"<rect x="2.25" y="3.75" width="11.5" height="10" rx="1.5"/><path d="M5.5 6.75H6.5M8.5 6.75H11M5.5 8.75H6.5M8.5 8.75H10.5M5.5 10.75H6.5M8.5 10.75H10"/>"#
);
/// Drawn at 10 pt in a knocked-out disc at a file icon's bottom-left corner, on its own 10 pt grid.
pub static LINK_BADGE: IconPair = IconPair {
    x1: stroked!("dromon-link-badge@1x", 10.0, "1", r#"<path d="M3.5 6.5L6.5 3.5M4 3.5H6.5V6"/>"#),
    x2: stroked!("dromon-link-badge@2x", 10.0, "1.5", r#"<path d="M3.5 6.5L6.75 3.25M4 3.25H6.75V6"/>"#),
};
pub static LOCK: IconPair = pair!(
    "dromon-lock",
    r#"<rect x="3.5" y="7.5" width="9" height="6" rx="1.5"/><path d="M5.5 7.5V5.5C5.5 4.12 6.62 3 8 3C9.38 3 10.5 4.12 10.5 5.5V7.5"/>"#,
    r#"<rect x="3.25" y="7.25" width="9.5" height="6" rx="1.5"/><path d="M5.25 7.25V5.5C5.25 4.12 6.62 3 8 3C9.38 3 10.75 4.12 10.75 5.5V7.25"/>"#
);
pub static SERVER: IconPair = pair!(
    "dromon-server",
    r#"<rect x="2.5" y="2.5" width="11" height="4" rx="1.5"/><rect x="2.5" y="9.5" width="11" height="4" rx="1.5"/><path d="M5 4.5H5.01M5 11.5H5.01" stroke-width="1.75"/>"#,
    r#"<rect x="2.25" y="2.25" width="11.5" height="4.5" rx="1.5"/><rect x="2.25" y="9.25" width="11.5" height="4.5" rx="1.5"/><path d="M5 4.5H5.01M5 11.5H5.01" stroke-width="1.75"/>"#
);
pub static CHEVRON_LEFT: IconPair = pair!("dromon-chevron-left", r#"<path d="M9.5 4L5.5 8L9.5 12"/>"#, r#"<path d="M9.5 4L5.5 8L9.5 12"/>"#);
pub static CHEVRON_RIGHT: IconPair = pair!("dromon-chevron-right", r#"<path d="M6.5 4L10.5 8L6.5 12"/>"#, r#"<path d="M6.5 4L10.5 8L6.5 12"/>"#);
/// The breadcrumb separator: a smaller chevron on the same grid.
pub static CRUMB: IconPair = pair!("dromon-crumb", r#"<path d="M6.5 5L9.5 8L6.5 11"/>"#, r#"<path d="M6.5 5L9.5 8L6.5 11"/>"#);
pub static CARET_UP: IconPair = pair!("dromon-caret-up", r#"<path d="M5 9.5L8 6.5L11 9.5"/>"#, r#"<path d="M5 9.5L8 6.5L11 9.5"/>"#);
pub static CARET_DOWN: IconPair = pair!("dromon-caret-down", r#"<path d="M5 6.5L8 9.5L11 6.5"/>"#, r#"<path d="M5 6.5L8 9.5L11 6.5"/>"#);
pub static ARROW_UP: IconPair = pair!(
    "dromon-arrow-up",
    r#"<path d="M7.5 13V3.5M4 7L7.5 3.5L11 7"/>"#,
    r#"<path d="M7.75 13V3.5M4.25 7L7.75 3.5L11.25 7"/>"#
);
pub static SEARCH: IconPair = pair!(
    "dromon-search",
    r#"<circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5L13.5 13.5"/>"#,
    r#"<circle cx="6.75" cy="6.75" r="4.5"/><path d="M10.25 10.25L13.25 13.25"/>"#
);
pub static PLUS: IconPair = pair!(
    "dromon-plus",
    r#"<path d="M7.5 3V12M3 7.5H12"/>"#,
    r#"<path d="M7.75 3.25V12.25M3.25 7.75H12.25"/>"#
);
pub static CLOSE: IconPair = pair!("dromon-close", r#"<path d="M5 5L11 11M11 5L5 11"/>"#, r#"<path d="M5 5L11 11M11 5L5 11"/>"#);
pub static DOWNLOAD: IconPair = pair!(
    "dromon-download",
    r#"<path d="M7.5 2.5V10M4 6.5L7.5 10L11 6.5M2.5 13.5H12.5"/>"#,
    r#"<path d="M7.75 2.5V10M4.25 6.5L7.75 10L11.25 6.5M2.75 13.75H12.75"/>"#
);
pub static UPLOAD: IconPair = pair!(
    "dromon-upload",
    r#"<path d="M7.5 10.5V3M4 6.5L7.5 3L11 6.5M2.5 13.5H12.5"/>"#,
    r#"<path d="M7.75 10.5V3M4.25 6.5L7.75 3L11.25 6.5M2.75 13.75H12.75"/>"#
);
pub static GEAR: IconPair = pair!(
    "dromon-gear",
    r#"<circle cx="7.5" cy="7.5" r="4.25"/><circle cx="7.5" cy="7.5" r="1.75"/><path d="M7.5 1V3.25M7.5 11.75V14M1.87 4.25L3.82 5.38M11.18 9.63L13.13 10.75M1.87 10.75L3.82 9.63M11.18 5.38L13.13 4.25"/>"#,
    r#"<circle cx="7.75" cy="7.75" r="4.25"/><circle cx="7.75" cy="7.75" r="1.75"/><path d="M7.75 1.25V3.5M7.75 12V14.25M2.12 4.5L4.07 5.63M11.43 9.88L13.38 11M2.12 11L4.07 9.88M11.43 5.63L13.38 4.5"/>"#
);
pub static MORE: IconPair = pair!(
    "dromon-more",
    r#"<path d="M3.5 8H3.51M8 8H8.01M12.5 8H12.51" stroke-width="2"/>"#,
    r#"<path d="M3.5 8H3.51M8 8H8.01M12.5 8H12.51" stroke-width="2"/>"#
);
pub static PAUSE: IconPair = pair!("dromon-pause", r#"<path d="M5.5 4V12M10.5 4V12"/>"#, r#"<path d="M5.75 4V12M10.25 4V12"/>"#);
/// Play: a triangle whose upright edge is pixel-exact.
pub static RESUME: IconPair = pair!(
    "dromon-resume",
    r#"<path d="M5.5 3.5V12.5L12.5 8L5.5 3.5Z"/>"#,
    r#"<path d="M5.25 3.5V12.5L12.5 8L5.25 3.5Z"/>"#
);
/// A clockwise turn back to the top, with its arrowhead's straight edges on the grid.
pub static RETRY: IconPair = pair!(
    "dromon-retry",
    r#"<path d="M13 8A5 5 0 1 1 11.54 4.46"/><path d="M11.5 1.5V4.5H8.5"/>"#,
    r#"<path d="M13 8A5 5 0 1 1 11.54 4.46"/><path d="M11.75 1.75V4.75H8.75"/>"#
);
pub static CHECK: IconPair = pair!("dromon-check", r#"<path d="M3.5 8.5L6.5 11.5L12.5 4.5"/>"#, r#"<path d="M3.5 8.5L6.5 11.5L12.5 4.5"/>"#);
/// Three quarters of a ring; spun by a rotation loop while connecting, so its curve stays as drawn.
pub static SPINNER: IconPair = pair!(
    "dromon-spinner",
    r#"<path d="M8 2.5C11.04 2.5 13.5 4.96 13.5 8C13.5 11.04 11.04 13.5 8 13.5C4.96 13.5 2.5 11.04 2.5 8"/>"#,
    r#"<path d="M8 2.5C11.04 2.5 13.5 4.96 13.5 8C13.5 11.04 11.04 13.5 8 13.5C4.96 13.5 2.5 11.04 2.5 8"/>"#
);
pub static WARNING: IconPair = pair!(
    "dromon-warning",
    r#"<path d="M7.5 2.5L13.5 13.5H1.5L7.5 2.5Z"/><path d="M7.5 6.5V9M7.5 11H7.51"/>"#,
    r#"<path d="M7.75 2.5L13.75 13.25H1.75L7.75 2.5Z"/><path d="M7.75 6.5V9M7.75 11H7.76"/>"#
);

/// What a file looks like in the list: its glyph and a category hue (degrees, OKLCH) that the
/// view desaturates into a quiet tint, or `None` for the neutral `text-2`.
pub fn for_file(name: &str) -> (&'static IconPair, Option<f32>) {
    let ext = name.rsplit_once('.').map(|(stem, ext)| if stem.is_empty() { "" } else { ext }).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "rs" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "css" | "html" | "htm" | "json" | "py" | "go" | "sh" | "rb" | "php" | "toml"
        | "yaml" | "yml" | "xml" | "lock" => (&FILE_CODE, Some(250.0)),
        "md" | "txt" | "log" | "csv" | "conf" | "ini" | "env" => (&FILE_TEXT, None),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif" | "heic" => (&FILE_IMAGE, Some(320.0)),
        "zip" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "tar" | "7z" | "rar" => (&FILE_ARCHIVE, Some(70.0)),
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "opus" | "aiff" | "aif" | "wma" => (&FILE_AUDIO, Some(150.0)),
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "mpg" | "mpeg" => (&FILE_VIDEO, Some(200.0)),
        "pdf" => (&FILE_PDF, Some(355.0)),
        "bin" | "exe" | "so" | "dylib" | "dll" | "o" | "a" | "wasm" => (&FILE_BINARY, Some(20.0)),
        _ => (&FILE, None),
    }
}
