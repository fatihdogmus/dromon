//! The icon set's pixel tests: each icon is rasterised with Rune's own icon rasteriser (the one
//! that fills the glyph atlas, `rune::text::icon_coverage`) at 1× and 2×, and the alpha profile
//! across its straight edges must hold no half-covered pixel. A layout test checks the icon
//! elements the views build sit on whole points, so the grid survives onto the screen.

use rune::Env;

use super::*;

#[test]
fn categories_by_extension() {
    assert_eq!(for_file("index.html").0.x1.name, "dromon-file-code@1x");
    assert_eq!(for_file("photo.JPG").0.x2.name, "dromon-file-image@2x");
    assert_eq!(for_file("backup.tar.gz").0.x1.name, "dromon-file-archive@1x");
    assert_eq!(for_file("README").0.x1.name, "dromon-file@1x");
    // A dotfile's name is not an extension.
    assert_eq!(for_file(".gitignore").0.x1.name, "dromon-file@1x");
    assert_eq!(for_file(".env").0.x1.name, "dromon-file@1x");
}

const ALL: &[&IconPair] = &[
    &FOLDER,
    &FOLDER_OPEN,
    &FILE,
    &FILE_CODE,
    &FILE_TEXT,
    &FILE_IMAGE,
    &FILE_ARCHIVE,
    &FILE_BINARY,
    &LINK_BADGE,
    &LOCK,
    &SERVER,
    &CHEVRON_LEFT,
    &CHEVRON_RIGHT,
    &CRUMB,
    &CARET_UP,
    &CARET_DOWN,
    &ARROW_UP,
    &SEARCH,
    &PLUS,
    &CLOSE,
    &DOWNLOAD,
    &UPLOAD,
    &GEAR,
    &MORE,
    &CHECK,
    &SPINNER,
    &WARNING,
    &PAUSE,
    &RESUME,
    &RETRY,
];

#[test]
fn the_scale_picks_the_body() {
    for scale in [0.5, 1.0, 1.25, 1.49] {
        assert!(std::ptr::eq(FOLDER.at(scale), &FOLDER.x1), "{scale}");
    }
    for scale in [1.5, 2.0, 3.0] {
        assert!(std::ptr::eq(FOLDER.at(scale), &FOLDER.x2), "{scale}");
    }
}

#[test]
fn every_icon_has_both_bodies_named_apart_and_stroked_for_its_scale() {
    for pair in ALL.iter().copied() {
        let (a, b) = (&pair.x1, &pair.x2);
        assert!(a.name.ends_with("@1x") && b.name.ends_with("@2x"), "{} / {}", a.name, b.name);
        assert_eq!(a.view, b.view, "{}", a.name);
        assert!(a.body.contains(r#"stroke-width="1""#), "{}", a.name);
        assert!(b.body.contains(r#"stroke-width="1.5""#), "{}", b.name);
        for (icon, scale) in [(a, 1u32), (b, 2)] {
            let px = icon.view as u32 * scale;
            let ink = icon_coverage(icon, px).iter().filter(|&&c| c > 0).count();
            assert!(ink > 0, "{} draws nothing", icon.name);
        }
    }
}

fn icon_coverage(icon: &Icon, px: u32) -> Vec<u8> {
    rune::text::icon_coverage(icon, px)
}

/// The icon with its tinted fill taken out, so only strokes are measured.
fn ink_only(icon: &Icon) -> Icon {
    let body = icon.body.replace(r#"fill="currentColor" fill-opacity="0.16""#, r#"fill="none""#);
    Icon { name: icon.name, view: icon.view, body: Box::leak(body.into_boxed_str()) }
}

#[derive(Clone, Copy, Debug)]
enum Line {
    /// The pixel row holding this y (pt): crosses vertical strokes.
    Row(f32),
    /// The pixel column holding this x (pt): crosses horizontal strokes.
    Col(f32),
}

/// A line of pixels across straight edges: how many strokes it crosses between `span` (pt along
/// the line, chosen to keep clear of corners, diagonals, curves and caps).
struct Scan {
    icon: &'static IconPair,
    line: Line,
    span: (f32, f32),
    strokes: usize,
}

const fn scan(icon: &'static IconPair, line: Line, span: (f32, f32), strokes: usize) -> Scan {
    Scan { icon, line, span, strokes }
}

/// Coverage (0 to 1) along the scan's pixels at `scale`.
fn profile(s: &Scan, scale: u32) -> Vec<f32> {
    let icon = ink_only(s.icon.at(scale as f32));
    let px = icon.view as u32 * scale;
    let cov = icon_coverage(&icon, px);
    let at = |x: u32, y: u32| cov[(y * px + x) as usize] as f32 / 255.0;
    let k = scale as f32;
    let (lo, hi) = ((s.span.0 * k) as u32, (s.span.1 * k) as u32);
    (lo..hi.min(px))
        .map(|i| match s.line {
            Line::Row(y) => at(i, (y * k) as u32),
            Line::Col(x) => at((x * k) as u32, i),
        })
        .collect()
}

#[test]
fn straight_edges_are_pixel_exact_at_both_scales() {
    let full = (0.0, 16.0);
    let scans = [
        // FOLDER: both sides, then the tab's and the body's top and the bottom.
        scan(&FOLDER, Line::Row(9.0), full, 2),
        scan(&FOLDER, Line::Col(4.0), full, 2),
        scan(&FOLDER, Line::Col(10.0), full, 2),
        scan(&FOLDER_OPEN, Line::Col(10.0), full, 3),
        // FILE and its relatives: the sides and the top and bottom.
        scan(&FILE, Line::Row(9.0), full, 2),
        scan(&FILE, Line::Col(6.0), full, 2),
        scan(&FILE_TEXT, Line::Col(7.0), full, 5),
        scan(&FILE_CODE, Line::Row(13.0), full, 2),
        scan(&FILE_ARCHIVE, Line::Row(10.0), full, 3),
        // SERVER: the tops and bottoms of both units, then each side clear of the status dot.
        scan(&SERVER, Line::Col(8.0), full, 4),
        scan(&SERVER, Line::Row(4.0), (0.0, 4.0), 1),
        scan(&SERVER, Line::Row(4.0), (12.0, 16.0), 1),
        // PLUS: one arm across each way.
        scan(&PLUS, Line::Row(5.0), full, 1),
        scan(&PLUS, Line::Col(5.0), full, 1),
        // The rest of the straight ones.
        scan(&LOCK, Line::Row(10.0), full, 2),
        scan(&ARROW_UP, Line::Row(10.0), full, 1),
        scan(&DOWNLOAD, Line::Col(3.0), (10.0, 16.0), 1),
        scan(&UPLOAD, Line::Col(3.0), (10.0, 16.0), 1),
        scan(&GEAR, Line::Row(1.5), (5.0, 10.0), 1),
        scan(&WARNING, Line::Row(7.0), (6.0, 9.0), 1),
        // The transfer row's buttons: both bars, the play triangle's upright edge, the retry
        // arrowhead's upright leg.
        scan(&PAUSE, Line::Row(8.0), full, 2),
        scan(&RESUME, Line::Row(8.0), (0.0, 8.0), 1),
        scan(&RETRY, Line::Row(2.5), (11.0, 16.0), 1),
    ];
    let mut report = String::new();
    let mut bad = Vec::new();
    for s in &scans {
        for scale in [1u32, 2] {
            let p = profile(s, scale);
            let name = s.icon.at(scale as f32).name;
            let shown: Vec<String> = p.iter().map(|c| format!("{c:.2}")).collect();
            report += &format!("{name} {:?} [{}]\n", s.line, shown.join(" "));
            let soft: Vec<f32> = p.iter().copied().filter(|&c| c > 0.1 && c < 0.9).collect();
            // A stroke is 1 pt at 1x (1 px) and 1.5 pt at 2x (3 px).
            let solid = p.iter().filter(|&&c| c >= 0.9).count();
            let want = s.strokes * if scale == 1 { 1 } else { 3 };
            if !soft.is_empty() || solid != want {
                bad.push(format!("{name} {:?}: soft {soft:?}, {solid} solid pixels (want {want})", s.line));
            }
        }
    }
    println!("{report}");
    assert!(bad.is_empty(), "half-covered or missing edges:\n{}\nprofiles:\n{report}", bad.join("\n"));
}

/// Elements whose icon follows text of a fractional width (breadcrumbs, the sort caret after a
/// column label, buttons centred on their label, what sits left of the ⌘K chip), so its x is
/// a fraction of a point. Rune rounds a resting icon's origin to the device grid when it paints
/// (`runtime/paint/content.rs`), so these still land on whole device pixels; only a y or an x
/// outside this list would put the grid at risk.
const TEXT_FLOWED: [&str; 7] = ["tb-path", "col-", "welcome-quick-connect", "state-go-up", "tb-transfers", "tb-filter", "drop-pill"];

/// The icon elements every scene builds begin on whole points (the views put them in 28 pt
/// rows and buttons, past whole-point paddings), at both scales, except where text flows
/// before them.
#[test]
fn icons_sit_on_whole_points_in_every_scene() {
    use crate::{app, scenes};
    let mut seen = 0;
    let mut off = Vec::new();
    for scale in [1.0f32, 2.0] {
        for &(scene, apply) in scenes::SCENES {
            let mut rt = app().headless_runtime((1080.0, 680.0), Env::new(true, scale));
            rt.update(|s, _| apply(s));
            for _ in 0..6 {
                rt.frame(1.0 / 60.0);
            }
            for e in rt.elements().into_iter().filter(|e| e.role == "icon") {
                let [x, y, ..] = e.rect;
                let flowed = TEXT_FLOWED.iter().any(|k| e.path.contains(k));
                if y.fract() != 0.0 || (x.fract() != 0.0 && !flowed) {
                    off.push(format!("{scene} @{scale}x: icon at ({x}, {y}) in {}", e.path));
                }
                seen += 1;
            }
        }
    }
    assert!(off.is_empty(), "icons off the whole-point grid:\n{}", off.join("\n"));
    assert!(seen > 100, "only {seen} icons seen");
}
