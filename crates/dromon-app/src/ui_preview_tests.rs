//! The quick preview driven headless through the real view, handlers and `on_key`, on the demo
//! fixture (which answers reads at once with made-up content): Space opens it, ↑/↓ and a click
//! on another row move it along, Escape and Space close it, Download from it starts a
//! download, and Tab stays among its controls. The machine itself, its reads and what each kind
//! of file shows are `state/preview_tests.rs`'s.

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};

use crate::state::preview::{Body, Phase, Text};
use crate::state::{App, PreviewEvent, ServiceEvent};
use crate::theme::space;
use crate::views::{
    PREVIEW, PREVIEW_ADVANCE as ADVANCE, PREVIEW_CLOSE, PREVIEW_DOWNLOAD, PREVIEW_LINE_H as LINE_H,
    PREVIEW_LINES as LINES, ROWS,
};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);
const SHIFT: Mods = Mods { shift: true, cmd: false, alt: false, ctrl: false, logo: false };

/// prod's site, index.html selected.
fn runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| scenes::listing(s));
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

fn space(rt: &mut Runtime<App>) {
    press(rt, Key::Text(" ".into()), Mods::default());
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let e = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && (e.key.as_deref() == Some(key) || e.path.ends_with(&format!("/{key}"))))
        .unwrap_or_else(|| panic!("no element {key:?}"));
    // A row's left end: its name, which the panel leaves in reach.
    let [x, y, w, h] = e.rect;
    let (x, y) = if key.starts_with("rows/") { (x + 40.0, y + h / 2.0) } else { (x + w / 2.0, y + h / 2.0) };
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn shown(rt: &Runtime<App>, k: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

/// The file on the panel, by its title; `None` while it's closed.
fn on_panel(rt: &Runtime<App>) -> Option<String> {
    shown(rt, PREVIEW)?;
    shown(rt, "preview-title").and_then(|e| e.text)
}

/// Block `b`'s lines on the panel (`preview-lines/<b>/lines`), or their numbers (`…/n`), if
/// built.
fn block(rt: &Runtime<App>, b: usize) -> Option<ElementInfo> {
    part(rt, &format!("{LINES}/{b}/lines"))
}

fn numbers(rt: &Runtime<App>, b: usize) -> Option<ElementInfo> {
    part(rt, &format!("{LINES}/{b}/n"))
}

fn part(rt: &Runtime<App>, path: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| !e.exiting && e.path.ends_with(path))
}

/// The blocks built (mounted) now, by index.
fn built(rt: &Runtime<App>) -> Vec<usize> {
    let mut v: Vec<usize> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.key.as_deref() == Some("lines"))
        .filter_map(|e| e.path.rsplit('/').nth(1).and_then(|i| i.parse().ok()))
        .collect();
    v.sort_unstable();
    v
}

/// The text on the panel.
fn shown_text(rt: &Runtime<App>) -> &Text {
    match &rt.state.ui.preview.phase {
        Phase::Shown { body: Body::Text(t), .. } => t,
        other => panic!("{other:?}"),
    }
}

/// Answers the read the panel waits for with `bytes` of a file of `total`.
fn answer(rt: &mut Runtime<App>, bytes: Vec<u8>, total: u64) {
    let id = match &rt.state.ui.preview.phase {
        Phase::Loading { id, .. } => *id,
        other => panic!("{other:?}"),
    };
    rt.update(move |s, _| {
        let bytes = crate::state::PreviewBytes(bytes.into());
        let host = dromon_core::HostId("prod".into());
        s.apply(ServiceEvent::Preview(PreviewEvent::Done { id, host, bytes, total }));
    });
    settle(rt);
}

fn focused(rt: &Runtime<App>) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.focused && !e.exiting)
}

fn focused_key(rt: &Runtime<App>) -> String {
    focused(rt).and_then(|e| e.key).unwrap_or_default()
}

#[test]
fn space_opens_the_preview_the_arrows_move_it_and_escape_closes_it() {
    let mut rt = runtime();
    assert_eq!(on_panel(&rt), None);
    space(&mut rt);
    assert_eq!(on_panel(&rt).as_deref(), Some("index.html"));
    let panel = shown(&rt, PREVIEW).unwrap();
    assert_eq!((panel.role, panel.label.as_deref()), ("dialog", Some("Preview of index.html")));
    assert!(block(&rt, 0).and_then(|e| e.text).is_some_and(|t| t.starts_with("<!doctype html>")));
    let first = numbers(&rt, 0).and_then(|e| e.text).expect("line numbers beside it");
    assert!(first.starts_with("1\n2\n"), "{first:?}");
    // ↓ moves the cursor, and the preview with it; ↑ back.
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(on_panel(&rt).as_deref(), Some("package-lock.json"));
    assert!(shown(&rt, "preview-cut").and_then(|e| e.text).is_some_and(|t| t == "Showing the first 256 KB of 412 KB"));
    press(&mut rt, Key::Up, Mods::default());
    assert_eq!(on_panel(&rt).as_deref(), Some("index.html"));
    press(&mut rt, Key::Escape, Mods::default());
    assert_eq!(on_panel(&rt), None, "Escape closes it");
    assert!(rt.state.conn().unwrap().selection.contains("index.html"), "and leaves the selection");
    // Space again opens it, and closes it.
    space(&mut rt);
    assert!(on_panel(&rt).is_some());
    space(&mut rt);
    assert_eq!(on_panel(&rt), None, "Space closes it");
}

#[test]
fn a_click_on_another_row_moves_the_preview_and_a_log_shows_its_end() {
    let mut rt = runtime();
    space(&mut rt);
    click(&mut rt, "rows/server.log");
    assert_eq!(on_panel(&rt).as_deref(), Some("server.log"));
    let cut = shown(&rt, "preview-cut").and_then(|e| e.text);
    assert_eq!(cut.as_deref(), Some("Showing the last 256 KB of 4.8 MB"));
    let blocks = built(&rt);
    assert!(!blocks.is_empty() && numbers(&rt, blocks[0]).is_none(), "a cut tail has no line numbers");
    // It opens at its end: the last block's last line is on screen, inside the panel.
    let last = shown_text(&rt).blocks() - 1;
    let (body, last) =
        (shown(&rt, "preview-body").unwrap().rect, block(&rt, last).expect("the last block is built").rect);
    let bottom = last[1] + last[3];
    assert!(bottom <= body[1] + body[3] + 0.5 && bottom - LINE_H >= body[1], "on screen: {last:?} in {body:?}");
    click(&mut rt, "rows/backup-2026-11-01.tar.gz");
    assert_eq!(on_panel(&rt).as_deref(), Some("backup-2026-11-01.tar.gz"));
    assert!(shown(&rt, "preview-kind-value").is_some(), "an archive: its details");
    assert!(shown(&rt, LINES).is_none());
}

#[test]
fn the_text_is_a_region_of_blocks_laid_out_only_where_seen() {
    let mut rt = runtime();
    click(&mut rt, "rows/package-lock.json");
    space(&mut rt);
    let n = shown_text(&rt).lines();
    assert!(n > 3_000, "256 KB of 64-byte lines: {n}");
    let region = shown(&rt, LINES).expect("the lines");
    assert_eq!(region.role, "region");
    let said = format!("Contents of package-lock.json, {} lines", crate::fmt::thousands(n as u64));
    assert_eq!(region.label.as_deref(), Some(said.as_str()));
    let blocks = built(&rt);
    assert!(blocks.first() == Some(&0) && blocks.len() < 25, "a few screens of {n} lines: {blocks:?}");
    // Readers get the lines, not the numbers.
    let nodes = crate::ui_a11y_tests::tree(&rt);
    assert!(nodes.iter().any(|n| n.key == "lines"), "the lines are read");
    assert!(!nodes.iter().any(|n| n.key == "n"), "the numbers aren't");
    // Each column is the mono face's advance: what the content's width is reckoned in.
    let first = block(&rt, 0).unwrap();
    let chars = first.text.as_deref().unwrap().lines().map(|l| l.chars().count()).max().unwrap() as f32;
    assert!((first.rect[2] - chars * ADVANCE).abs() < 1.0, "{} columns, {} pt", chars, first.rect[2]);
}

#[test]
fn line_numbers_stay_beside_their_lines_across_blocks() {
    let mut rt = runtime();
    click(&mut rt, "rows/package-lock.json");
    space(&mut rt);
    let (lines0, numbers0) = (block(&rt, 0).unwrap(), numbers(&rt, 0).unwrap());
    let (lines1, numbers1) = (block(&rt, 1).unwrap(), numbers(&rt, 1).unwrap());
    // A whole block is 16 lines tall, its numbers too, and the next starts where it ends.
    let tall = 16.0 * LINE_H;
    for e in [&lines0, &numbers0, &lines1, &numbers1] {
        assert!((e.rect[3] - tall).abs() < 0.5, "{}: {:?}", e.path, e.rect);
    }
    assert!((lines1.rect[1] - (lines0.rect[1] + tall)).abs() < 0.5, "{:?} then {:?}", lines0.rect, lines1.rect);
    assert_eq!((numbers0.rect[1], numbers1.rect[1]), (lines0.rect[1], lines1.rect[1]), "level with their lines");
    // The second block's numbers go on from the first's.
    let n: Vec<String> = (17..=32).map(|i| i.to_string()).collect();
    assert_eq!(numbers1.text.as_deref(), Some(n.join("\n").as_str()));
    assert!(lines1.text.as_deref().unwrap().starts_with("  \"package-00017\""), "{:?}", lines1.text);
}

#[test]
fn a_long_line_scrolls_sideways_the_whole_width_of_the_widest_line() {
    let mut rt = runtime();
    rt.update(|s, _| s.backend = Box::new(crate::state::Silent::default()));
    space(&mut rt);
    let long = "x".repeat(400);
    let bytes = format!("short\n{long}\nend\n").into_bytes();
    let total = bytes.len() as u64;
    answer(&mut rt, bytes, total);
    let wide = block(&rt, 0).expect("the block with the long line").rect;
    let content = shown(&rt, "preview-content").unwrap().rect;
    assert!(content[2] >= wide[2] + 2.0 * ADVANCE, "the content holds the widest line: {content:?} {wide:?}");
    assert!((wide[2] - 400.0 * ADVANCE).abs() < 2.0, "{wide:?}");
    assert!(shown(&rt, "preview-long").is_none(), "400 characters aren't cut");
    let (x, y) = rt.find("preview-body").unwrap();
    // Further than the content is wide: the scroll stops at its end.
    rt.wheel_xy(x, y, -10_000.0, 0.0);
    settle(&mut rt);
    let moved = block(&rt, 0).unwrap().rect;
    let body = shown(&rt, "preview-body").unwrap().rect;
    assert!(moved[0] < wide[0] - 100.0, "it scrolled sideways: {wide:?} -> {moved:?}");
    assert!(moved[0] + moved[2] <= body[0] + body[2] + 0.5, "to its end and no further: {moved:?} in {body:?}");
    assert!(moved[0] + moved[2] >= body[0] + body[2] - space::S3 - 2.0, "its end in sight: {moved:?} in {body:?}");
}

#[test]
fn a_minified_line_is_cut_and_the_notes_say_so() {
    let mut rt = runtime();
    rt.update(|s, _| s.backend = Box::new(crate::state::Silent::default()));
    space(&mut rt);
    // index.html's first 256 KB, one line of it 10,000 characters long.
    let bytes = format!("<html>\n{}\n</html>\n", "y".repeat(10_000)).into_bytes();
    answer(&mut rt, bytes, 400_000);
    let notes: Vec<_> =
        ["preview-cut", "preview-long"].iter().filter_map(|k| shown(&rt, k).and_then(|e| e.text)).collect();
    assert_eq!(notes, ["Showing the first 256 KB of 400 KB", "Long lines are cut at 2,000 characters"]);
    let text = block(&rt, 0).and_then(|e| e.text).unwrap();
    assert_eq!(text.lines().nth(1).map(|l| l.chars().count()), Some(2_002), "2,000 characters and \" …\"");
    let content = shown(&rt, "preview-content").unwrap().rect;
    assert!(content[2] < 2_010.0 * ADVANCE, "as wide as what's shown: {content:?}");
}

#[test]
fn moving_on_shows_the_next_text_once_the_cursor_rests() {
    let mut rt = runtime();
    space(&mut rt);
    assert!(block(&rt, 0).is_some());
    // ↓: the next file is read at once, its text held back while the cursor may move on. The
    // key is handled after the frame's poll: the next frame's poll starts the rest.
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(on_panel(&rt).as_deref(), Some("package-lock.json"));
    assert!(shown(&rt, "preview-loading").is_some() && block(&rt, 0).is_none(), "loading meanwhile");
    rt.frame(1.0 / 60.0);
    rt.frame(0.1);
    assert!(block(&rt, 0).is_none(), "not rested yet");
    assert!(matches!(rt.state.ui.preview.phase, Phase::Waiting { .. }), "{:?}", rt.state.ui.preview.phase);
    rt.frame(0.1);
    assert!(block(&rt, 0).and_then(|e| e.text).is_some_and(|t| t.starts_with('{')), "rested: the text shows");
}

#[test]
fn download_from_the_panel_starts_a_download() {
    let mut rt = runtime();
    space(&mut rt);
    assert!(rt.state.transfers.jobs.is_empty());
    click(&mut rt, PREVIEW_DOWNLOAD);
    let titles: Vec<_> = rt.state.transfers.jobs.iter().map(|j| j.title.clone()).collect();
    assert_eq!(titles, ["index.html"]);
    assert!(on_panel(&rt).is_some(), "the panel stays");
}

#[test]
fn tab_stays_inside_the_panel_and_escape_gives_the_keyboard_back_to_the_list() {
    let mut rt = runtime();
    click(&mut rt, "rows/robots.txt");
    assert_eq!(focused_key(&rt), ROWS);
    space(&mut rt);
    assert_eq!(focused_key(&rt), PREVIEW_CLOSE, "the keyboard goes into the panel");
    let root = shown(&rt, PREVIEW).unwrap().path;
    let mut seen = Vec::new();
    for back in [false, false, false, true, true, true] {
        press(&mut rt, Key::Tab, if back { SHIFT } else { Mods::default() });
        let f = focused(&rt).expect("something focused");
        assert!(f.path.starts_with(&root), "Tab left the panel for {:?}", f.key);
        assert!(f.focus_visible, "with its ring");
        seen.push(f.key.unwrap_or_default());
    }
    assert_eq!(
        seen,
        [PREVIEW_DOWNLOAD, PREVIEW_CLOSE, PREVIEW_DOWNLOAD, PREVIEW_CLOSE, PREVIEW_DOWNLOAD, PREVIEW_CLOSE]
    );
    // ↓ from there still moves the list's cursor.
    press(&mut rt, Key::Down, Mods::default());
    assert_eq!(on_panel(&rt).as_deref(), Some("server.log"));
    press(&mut rt, Key::Escape, Mods::default());
    assert_eq!(on_panel(&rt), None);
    assert_eq!(focused_key(&rt), ROWS, "back to the rows");
}

#[test]
fn an_image_shows_fitted_and_says_its_size_once_decoded() {
    let mut rt = runtime();
    click(&mut rt, "rows/favicon.ico");
    space(&mut rt);
    rt.state.ui.preview.wait_for_image();
    // The poll notices the decode ended, and the view says the image's size.
    settle(&mut rt);
    let image = shown(&rt, "preview-image").expect("the image");
    assert_eq!((image.role, image.label.as_deref()), ("img", Some("favicon.ico")));
    let meta = shown(&rt, "preview-meta").and_then(|e| e.text);
    assert_eq!(meta.as_deref(), Some("64 × 64 pixels · 4.3 KB"));
}

/// The panel 60 ms after Space: how far from its place, and how opaque.
fn opening(system: bool, forced: bool) -> (f32, f32) {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.set_system_reduced_motion(system);
    rt.update(move |s, _| {
        scenes::listing(s);
        s.ui.access.reduce_motion = forced;
    });
    settle(&mut rt);
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    rt.frame(0.0);
    rt.frame(0.06);
    let e = shown(&rt, PREVIEW).expect("the panel");
    let off = e.rect.iter().zip(&e.target).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
    (off, e.opacity)
}

#[test]
fn the_panel_springs_in_and_comes_at_once_under_reduce_motion() {
    let (off, opacity) = opening(false, false);
    assert!(off > 1.0 && opacity < 0.99, "it slides and fades in: {off} pt off, opacity {opacity}");
    for (system, forced) in [(true, false), (false, true)] {
        let (off, opacity) = opening(system, forced);
        assert!(off < 0.01 && opacity > 0.999, "at once (system {system}, forced {forced}): {off} pt, {opacity}");
    }
}

#[test]
fn a_read_that_failed_says_why() {
    let mut rt = runtime();
    rt.update(|s, _| s.backend = Box::new(crate::state::Silent::default()));
    space(&mut rt);
    assert!(shown(&rt, "preview-loading").is_some(), "loading");
    let id = match &rt.state.ui.preview.phase {
        crate::state::preview::Phase::Loading { id, .. } => *id,
        other => panic!("{other:?}"),
    };
    rt.update(move |s, _| {
        let error = dromon_core::Error::PermissionDenied(dromon_core::RemotePath::new("/var/www/site/index.html"));
        let host = dromon_core::HostId("prod".into());
        s.apply(ServiceEvent::Preview(PreviewEvent::Failed { id, host, error }));
    });
    settle(&mut rt);
    let note = shown(&rt, "preview-note").expect("why");
    assert_eq!(note.role, "alert");
    assert!(note.text.is_some_and(|t| t.starts_with("Couldn't preview it:")));
}
