//! Polling in the real app, headless (§4.5, §6.6, §6.8): the window's focus and visibility
//! decide polls, a poll's new rows slide in under a mint wash that decays, removed rows
//! collapse out, nothing scrolls, Reduce Motion keeps the wash, and a folder deleted
//! elsewhere sends the view to its parent with a toast. Polls happen when the window regains
//! focus (the fixture has no clock), so the tests step frames, never the wall clock.

use std::sync::Arc;

use rune::{Env, Runtime};

use crate::state::App;
use crate::{app, fixture};

const SIZE: (f32, f32) = (1080.0, 680.0);
const FRAME: f64 = 1.0 / 60.0;

/// prod at /w: `sub/` and f-01.txt … f-40.txt, and the changes made elsewhere `changes`.
fn fixture(changes: &str) -> Arc<fixture::Fixture> {
    let src = format!(
        r#"{{"version": 1, "now": "2026-11-25T14:00:00Z",
            "hosts": [{{"alias": "prod", "source": "ssh-config"}}],
            "servers": {{"prod": {{"home": "/w", "changes": {changes}, "tree": {{
                "/w": [{{"name": "sub", "kind": "dir"}},
                       {{"generate": {{"pattern": "f-{{02}}.txt", "count": 40, "size": 1000, "mtime": "2026-11-25T13:00:00Z"}}}}],
                "/w/sub": [{{"name": "x.txt", "kind": "file"}}]}}}}}}}}"#
    );
    Arc::new(fixture::parse(&src).unwrap_or_else(|e| panic!("{e}")))
}

fn runtime(fx: Arc<fixture::Fixture>, reduced: bool) -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.set_system_reduced_motion(reduced);
    rt.update(move |s, _| *s = fixture::app(fx));
    rt.frame(0.0);
    // The window reports its focus first (Rune does once it shows).
    rt.window_focus(true);
    rt.frame(FRAME);
    click(&mut rt, "host-prod");
    rt
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(FRAME);
}

fn frames(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / FRAME).round() as usize {
        rt.frame(FRAME);
    }
}

/// The window loses focus and gets it back: a poll happens now.
fn poll(rt: &mut Runtime<App>) {
    rt.window_focus(false);
    rt.frame(FRAME);
    rt.window_focus(true);
    rt.frame(FRAME);
}

/// The drawn opacity of refresh `seq`'s wash on row `name`, if it's there.
fn wash(rt: &Runtime<App>, seq: u64, name: &str) -> Option<f32> {
    let key = format!("fresh-{seq}");
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str()) && e.path.contains(name)).map(|e| e.opacity)
}

fn row_y(rt: &Runtime<App>, name: &str) -> f32 {
    rt.presented(&format!("rows/{name}")).unwrap_or_else(|| panic!("no row {name}")).0[1]
}

fn exiting(rt: &Runtime<App>, name: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(name) && e.exiting)
}

fn text_of(rt: &Runtime<App>, key: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text).unwrap_or_default()
}

const STEP: &str = r#"[[{"add": "/w/f-00a.txt", "kind": "file", "size": 5}, {"remove": "/w/f-03.txt"}, {"modify": "/w/f-05.txt", "size": 123456}]]"#;

#[test]
fn a_poll_slides_new_rows_in_under_a_wash_that_decays_and_collapses_removed_ones() {
    let mut rt = runtime(fixture(STEP), false);
    let before_y = row_y(&rt, "f-04.txt");
    poll(&mut rt);
    let c = rt.state.conn().unwrap();
    assert!(c.find("f-00a.txt").is_some() && c.find("f-03.txt").is_none());
    assert_eq!(c.find("f-05.txt").unwrap().size, 123_456, "changed in place");
    assert!(rt.find("rows/f-00a.txt").is_some());

    // The removed row is still leaving, and the rows below it close its gap gliding.
    assert!(exiting(&rt, "f-03.txt"), "f-03 collapses out");
    let ys = track(&mut rt, 0.35);
    let early = wash(&rt, 1, "f-00a.txt").expect("the wash");
    frames(&mut rt, 1.2);
    let end_y = row_y(&rt, "f-04.txt");
    // f-00a.txt came in above (one row down), f-03.txt left above (one row up): back where it was.
    assert!((end_y - before_y).abs() < 0.5, "f-04 rests where it was: {before_y} → {end_y}");
    assert!(ys.iter().any(|y| *y > before_y + 1.0 && *y < before_y + 27.0), "and glided there: {ys:?}");
    assert!(!exiting(&rt, "f-03.txt") && rt.find("rows/f-03.txt").is_none());

    assert!(early > 0.25, "the wash shows at first: {early}");
    assert!(wash(&rt, 1, "f-00a.txt").unwrap_or(0.0) < 0.02, "and is gone after 1.2 s");
    assert!(rt.state.conn().unwrap().fresh.seen);
}

/// f-04.txt's drawn top, frame by frame for `secs`.
fn track(rt: &mut Runtime<App>, secs: f64) -> Vec<f32> {
    (0..(secs / FRAME).round() as usize)
        .map(|_| {
            rt.frame(FRAME);
            row_y(rt, "f-04.txt")
        })
        .collect()
}

#[test]
fn under_reduce_motion_the_wash_holds_then_goes() {
    let mut rt = runtime(fixture(STEP), true);
    let before_y = row_y(&rt, "f-04.txt");
    let new_y = row_y(&rt, "f-03.txt") - 2.0 * 28.0;
    poll(&mut rt);
    let ys = track(&mut rt, 0.8);
    let held = wash(&rt, 1, "f-00a.txt").expect("the wash");
    assert!(held > 0.95, "colour, not motion: it holds: {held}");
    frames(&mut rt, 0.7);
    assert!(wash(&rt, 1, "f-00a.txt").unwrap_or(0.0) < 0.02);
    // Nothing glides: the gap closes in one step, and the new row doesn't slide.
    assert!(ys.iter().all(|y| (*y - before_y).abs() < 0.5 || (*y - before_y - 28.0).abs() < 0.5), "{ys:?}");
    assert!(rt.find("rows/f-03.txt").is_none());
    assert!((row_y(&rt, "f-00a.txt") - new_y).abs() < 0.5);
}

#[test]
fn a_poll_moves_neither_the_scroll_nor_the_selection() {
    let mut rt = runtime(fixture(r#"[{"add": "/w/a-first.txt", "kind": "file"}]"#), false);
    click(&mut rt, "rows/f-02.txt");
    let top = row_y(&rt, "f-25.txt");
    rt.update(|_, tx| tx.scroll_to_y(crate::views::LIST, 420.0));
    frames(&mut rt, 1.0);
    let y = row_y(&rt, "f-25.txt");
    assert!(y < top - 400.0, "scrolled: f-25 from {top} to {y}");
    poll(&mut rt);
    frames(&mut rt, 1.0);
    assert!(rt.state.conn().unwrap().find("a-first.txt").is_some());
    assert!((row_y(&rt, "f-25.txt") - y).abs() < 0.5, "the row read stays put: {y} → {}", row_y(&rt, "f-25.txt"));
    let c = rt.state.conn().unwrap();
    assert!(c.selection.contains("f-02.txt") && c.selection.len() == 1);
}

#[test]
fn nothing_polls_while_the_window_is_hidden() {
    let mut rt = runtime(fixture(STEP), false);
    rt.set_live_visible(false);
    rt.frame(FRAME);
    poll(&mut rt);
    assert!(rt.state.conn().unwrap().find("f-00a.txt").is_none(), "hidden: no poll");
    rt.set_live_visible(true);
    rt.frame(FRAME);
    poll(&mut rt);
    assert!(rt.state.conn().unwrap().find("f-00a.txt").is_some());
}

#[test]
fn the_folder_on_screen_deleted_elsewhere_goes_up_with_a_toast() {
    let mut rt = runtime(fixture(r#"[{"remove": "/w/sub"}]"#), false);
    // Into sub (double-click), then it's deleted on the server.
    let (x, y) = rt.find("rows/sub").unwrap();
    for _ in 0..2 {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.05);
    }
    assert_eq!(rt.state.conn().unwrap().cwd.as_str(), "/w/sub");
    poll(&mut rt);
    assert_eq!(rt.state.conn().unwrap().cwd.as_str(), "/w");
    assert!(rt.state.conn().unwrap().find("sub").is_none());
    frames(&mut rt, 0.4);
    assert_eq!(text_of(&rt, "toast-note-1-text"), "/w/sub was removed");
}
