//! Dromon's icon set (§6.7), drawn on a 16 pt grid with 1.5 pt strokes whose centre lines sit on
//! half-pixel coordinates. Rune rasterises each body into the glyph atlas at the device scale
//! and tints it with the element's colour (`icon(&FOLDER, 16.0).color(..)`), so they stay crisp
//! at 1× and 2× as long as they're drawn at 16 pt (or a multiple).
//!
//! The source of truth is these SVG bodies; `assets/icons/` will hold exported files later.

use rune::Icon;

/// Wraps path data in the set's stroke style.
macro_rules! stroked {
    ($name:literal, $body:literal) => {
        Icon {
            name: $name,
            view: 16.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        }
    };
}

pub static FOLDER: Icon = stroked!(
    "dromon-folder",
    r#"<path d="M2.5 12.5V3.5H6.25L7.75 5H13.5V12.5H2.5Z" fill="currentColor" fill-opacity="0.16"/>"#
);
pub static FOLDER_OPEN: Icon = stroked!(
    "dromon-folder-open",
    r#"<path d="M2.5 12.5V3.5H6.25L7.75 5H12.5V7M2.5 12.5L4.5 7.5H14.5L12.5 12.5H2.5Z"/>"#
);
pub static FILE: Icon = stroked!("dromon-file", r#"<path d="M3.5 1.5H9.5L12.5 4.5V14.5H3.5V1.5Z"/><path d="M9.5 1.5V4.5H12.5"/>"#);
pub static FILE_CODE: Icon = stroked!(
    "dromon-file-code",
    r#"<path d="M3.5 1.5H9.5L12.5 4.5V14.5H3.5V1.5Z"/><path d="M9.5 1.5V4.5H12.5"/><path d="M6.5 8L5.25 9.5L6.5 11M9.5 8L10.75 9.5L9.5 11"/>"#
);
pub static FILE_TEXT: Icon = stroked!(
    "dromon-file-text",
    r#"<path d="M3.5 1.5H9.5L12.5 4.5V14.5H3.5V1.5Z"/><path d="M9.5 1.5V4.5H12.5"/><path d="M6 8H10M6 10.5H10M6 13H8"/>"#
);
pub static FILE_IMAGE: Icon = stroked!(
    "dromon-file-image",
    r#"<path d="M3.5 1.5H9.5L12.5 4.5V14.5H3.5V1.5Z"/><path d="M9.5 1.5V4.5H12.5"/><path d="M3.5 12.5L6.5 9.5L8.5 11.5L9.5 10.5L12.5 13.5"/>"#
);
pub static FILE_ARCHIVE: Icon = stroked!(
    "dromon-file-archive",
    r#"<path d="M3.5 1.5H9.5L12.5 4.5V14.5H3.5V1.5Z"/><path d="M9.5 1.5V4.5H12.5"/><path d="M7.5 3.5V4.5M7.5 6.5V7.5M7.5 9.5V12.5H8.5"/>"#
);
pub static FILE_BINARY: Icon = stroked!(
    "dromon-file-binary",
    r#"<path d="M3.5 1.5H9.5L12.5 4.5V14.5H3.5V1.5Z"/><path d="M9.5 1.5V4.5H12.5"/><path d="M6 8.5L7.5 10L6 11.5M8.5 11.5H10"/>"#
);
/// Drawn at 10 pt in a knocked-out disc at a file icon's bottom-left corner.
pub static LINK_BADGE: Icon = Icon {
    name: "dromon-link-badge",
    view: 10.0,
    body: r#"<g fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 6.5L6.5 3.5M4 3.5H6.5V6"/></g>"#,
};
pub static LOCK: Icon = stroked!(
    "dromon-lock",
    r#"<rect x="3.5" y="7.5" width="9" height="6" rx="1.5"/><path d="M5.5 7.5V5.5C5.5 4.12 6.62 3 8 3C9.38 3 10.5 4.12 10.5 5.5V7.5"/>"#
);
pub static SERVER: Icon = stroked!(
    "dromon-server",
    r#"<rect x="2.5" y="2.5" width="11" height="4.5" rx="1.5"/><rect x="2.5" y="9" width="11" height="4.5" rx="1.5"/><path d="M5 4.75H5.01M5 11.25H5.01" stroke-width="1.75"/>"#
);
pub static CHEVRON_LEFT: Icon = stroked!("dromon-chevron-left", r#"<path d="M9.5 4L5.5 8L9.5 12"/>"#);
pub static CHEVRON_RIGHT: Icon = stroked!("dromon-chevron-right", r#"<path d="M6.5 4L10.5 8L6.5 12"/>"#);
/// The breadcrumb separator: a smaller chevron on the same grid.
pub static CRUMB: Icon = stroked!("dromon-crumb", r#"<path d="M6.5 5L9.5 8L6.5 11"/>"#);
pub static CARET_UP: Icon = stroked!("dromon-caret-up", r#"<path d="M5 9.5L8 6.5L11 9.5"/>"#);
pub static CARET_DOWN: Icon = stroked!("dromon-caret-down", r#"<path d="M5 6.5L8 9.5L11 6.5"/>"#);
pub static ARROW_UP: Icon = stroked!("dromon-arrow-up", r#"<path d="M8 13V3.5M4.5 7L8 3.5L11.5 7"/>"#);
pub static SEARCH: Icon = stroked!("dromon-search", r#"<circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5L13.5 13.5"/>"#);
pub static PLUS: Icon = stroked!("dromon-plus", r#"<path d="M8 3.5V12.5M3.5 8H12.5"/>"#);
pub static CLOSE: Icon = stroked!("dromon-close", r#"<path d="M5 5L11 11M11 5L5 11"/>"#);
pub static DOWNLOAD: Icon = stroked!("dromon-download", r#"<path d="M8 2.5V10M4.5 6.5L8 10L11.5 6.5M3 13.5H13"/>"#);
pub static UPLOAD: Icon = stroked!("dromon-upload", r#"<path d="M8 10.5V3M4.5 6.5L8 3L11.5 6.5M3 13.5H13"/>"#);
pub static GEAR: Icon = stroked!(
    "dromon-gear",
    r#"<circle cx="8" cy="8" r="4.25"/><circle cx="8" cy="8" r="1.75"/><path d="M8 1.5V3.75M8 12.25V14.5M2.37 4.75L4.32 5.88M11.68 10.13L13.63 11.25M2.37 11.25L4.32 10.13M11.68 5.88L13.63 4.75"/>"#
);
pub static MORE: Icon = stroked!("dromon-more", r#"<path d="M3.5 8H3.51M8 8H8.01M12.5 8H12.51" stroke-width="2"/>"#);
pub static CHECK: Icon = stroked!("dromon-check", r#"<path d="M3.5 8.5L6.5 11.5L12.5 4.5"/>"#);
/// Three quarters of a ring; spun by a rotation loop while connecting.
pub static SPINNER: Icon = stroked!("dromon-spinner", r#"<path d="M8 2.5C11.04 2.5 13.5 4.96 13.5 8C13.5 11.04 11.04 13.5 8 13.5C4.96 13.5 2.5 11.04 2.5 8"/>"#);
pub static WARNING: Icon = stroked!(
    "dromon-warning",
    r#"<path d="M8 2.5L14 13H2L8 2.5Z"/><path d="M8 6.5V9M8 11H8.01"/>"#
);

/// What a file looks like in the list: its glyph and a category hue (degrees, OKLCH) that the
/// view desaturates into a quiet tint, or `None` for the neutral `text-2`.
pub fn for_file(name: &str) -> (&'static Icon, Option<f32>) {
    let ext = name.rsplit_once('.').map(|(stem, ext)| if stem.is_empty() { "" } else { ext }).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "rs" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "css" | "html" | "htm" | "json" | "py" | "go" | "sh" | "rb" | "php" | "toml"
        | "yaml" | "yml" | "xml" | "lock" => (&FILE_CODE, Some(250.0)),
        "md" | "txt" | "log" | "csv" | "conf" | "ini" | "env" => (&FILE_TEXT, None),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif" | "heic" => (&FILE_IMAGE, Some(320.0)),
        "zip" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "tar" | "7z" | "rar" => (&FILE_ARCHIVE, Some(70.0)),
        "bin" | "exe" | "so" | "dylib" | "dll" | "o" | "a" | "wasm" => (&FILE_BINARY, Some(20.0)),
        _ => (&FILE, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_by_extension() {
        assert_eq!(for_file("index.html").0.name, "dromon-file-code");
        assert_eq!(for_file("photo.JPG").0.name, "dromon-file-image");
        assert_eq!(for_file("backup.tar.gz").0.name, "dromon-file-archive");
        assert_eq!(for_file("README").0.name, "dromon-file");
        // A dotfile's name is not an extension.
        assert_eq!(for_file(".gitignore").0.name, "dromon-file");
        assert_eq!(for_file(".env").0.name, "dromon-file");
    }

    #[test]
    fn every_icon_strokes_at_one_and_a_half() {
        for i in [&FOLDER, &FILE, &LOCK, &SERVER, &CHEVRON_LEFT, &CHEVRON_RIGHT, &SEARCH, &PLUS, &DOWNLOAD, &GEAR] {
            assert_eq!(i.view, 16.0, "{}", i.name);
            assert!(i.body.contains(r#"stroke-width="1.5""#), "{}", i.name);
        }
    }
}
