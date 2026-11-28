//! Recording steps driven headless, then replayed: a user's session on the real view, recorded
//! from the palette's "Record steps…" to the pill's Stop, and the script it makes played on the
//! fixture it makes, in a window of its own. Both must end in the same state, by kind: what's on
//! screen, how deep the folder is, the status bar's counts and sizes, the selection, the
//! preview. Sessions on the demo and on two flows' fixtures: signing in with a wrong then a right
//! password, a denied folder, a Shift range, a row below the fold (the replay scrolls to it), a
//! rename and a preview.
//!
//! The player is a small one, for the events a recording writes (`state/record_script.rs`); it
//! fails where Rune's would: a row tapped where the list doesn't show it.

use std::sync::Arc;

use rune::{Env, Key, KeyEvent, Mods, PhysicalInput, Runtime, WheelEvent};
use serde_json::Value;

use crate::app;
use crate::fixture;
use crate::state::App;
use crate::state::redact::Redactor;

const SIZE: (f32, f32) = (1080.0, 680.0);
const MAC: bool = cfg!(target_os = "macos");
/// The primary modifier: ⌘ on macOS, Ctrl elsewhere.
const CMD: Mods = Mods { shift: false, cmd: true, alt: false, ctrl: !MAC, logo: MAC };
const SHIFT: Mods = Mods { shift: true, cmd: false, alt: false, ctrl: false, logo: false };

/// A clock that stands still: a fixture's is the process's, and other tests move it.
fn still() -> i64 {
    0
}

fn runtime(fx: Arc<fixture::Fixture>) -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(move |s, _| {
        *s = fixture::app(fx);
        s.wall_ms = still;
    });
    settle(&mut rt);
    rt
}

fn flow(name: &str) -> Arc<fixture::Fixture> {
    let path = format!("{}/../../tests/flows/{name}/fixture.json", env!("CARGO_MANIFEST_DIR"));
    Arc::new(fixture::load(&path).unwrap_or_else(|e| panic!("{e}")))
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

/// The centre of `key`, which must show: a row inside the list's drawn part.
fn at(rt: &Runtime<App>, key: &str) -> (f32, f32) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    if key.starts_with("rows/") {
        let list = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("list")).expect("the list");
        let [lx, ly, lw, lh] = list.rect;
        assert!(
            x >= lx && x <= lx + lw && y >= ly && y <= ly + lh,
            "{key} at {y} isn't shown (the list is {ly}..{})",
            ly + lh
        );
    }
    (x, y)
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = at(rt, key);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn double_click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = at(rt, key);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.05);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn secondary_click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = at(rt, key);
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    settle(rt);
}

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

fn type_text(rt: &mut Runtime<App>, text: &str) {
    for c in text.chars() {
        rt.key(KeyEvent { key: Key::Text(c.to_string()), mods: Mods::default() });
        rt.frame(0.02);
    }
    settle(rt);
}

fn hold(rt: &mut Runtime<App>, mods: Mods) {
    rt.physical_input(PhysicalInput::Modifiers(mods));
    rt.frame(0.02);
}

fn wheel(rt: &mut Runtime<App>, dy: f32) {
    let (x, y) = rt.find("list").expect("the list");
    rt.pointer_move(x, y);
    rt.wheel_event(WheelEvent::new(0.0, dy).precise(true).at(x, y));
    settle(rt);
}

/// A script's key, as Rune's script reads it.
fn named(name: &str, cmd: bool) -> Key {
    match name {
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "Up" => Key::Up,
        "Down" => Key::Down,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Delete" => Key::Delete,
        "F2" => Key::Function(2),
        one if cmd => Key::Command(one.chars().next().expect("a key").to_ascii_lowercase()),
        one => Key::Text(one.to_owned()),
    }
}

/// Plays a recording's script.
fn play(rt: &mut Runtime<App>, script: &Value) {
    for e in script["events"].as_array().expect("events") {
        if let Some(h) = e.get("hold").and_then(Value::as_str) {
            let mods = match h {
                "shift" => SHIFT,
                "cmd" => CMD,
                _ => Mods::default(),
            };
            hold(rt, mods);
            continue;
        }
        let key = e["key"].as_str().unwrap_or_default();
        match e["type"].as_str() {
            Some("tap") => tap(rt, key),
            Some("double-click") => double_click(rt, key),
            Some("secondary-click") => secondary_click(rt, key),
            Some("key") => {
                let cmd = e["cmd"].as_bool().unwrap_or(false);
                let mut mods = if cmd { CMD } else { Mods::default() };
                mods.shift = e["shift"].as_bool().unwrap_or(false);
                press(rt, named(e["name"].as_str().expect("a key's name"), cmd), mods);
            }
            Some("type") => type_text(rt, e["text"].as_str().expect("text")),
            Some("wheel") => wheel(rt, e["dy"].as_f64().expect("dy") as f32),
            Some("mark") => {}
            other => panic!("a recording doesn't write {other:?}: {e}"),
        }
    }
}

/// The window's state by kind, names left out.
fn state(rt: &Runtime<App>) -> String {
    let s = &rt.state;
    let diagnostics = s.diagnostics(None, &mut Redactor::default()).text();
    let on_screen = diagnostics.lines().find(|l| l.starts_with("On screen:")).unwrap_or_default().to_owned();
    let text = |k: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(k) && !e.exiting)
            .and_then(|e| e.text)
            .unwrap_or_default()
    };
    let depth = s.conn().map(|c| c.cwd.components().count());
    let selected = s.conn().map(|c| c.selection.len());
    format!("{on_screen} | depth {depth:?} | {} | selected {selected:?}", text("status-summary"))
}

/// Starts recording from the palette.
fn start(rt: &mut Runtime<App>) {
    press(rt, Key::Command('k'), CMD);
    type_text(rt, "record steps");
    press(rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.recording_steps(), Some(0), "recording");
    assert!(rt.find("record").is_some(), "the pill");
}

/// Stops with the pill: the sheet, and the fixture and script it holds.
fn stop(rt: &mut Runtime<App>) -> (String, Value) {
    tap(rt, "record-stop");
    assert!(rt.find("report").is_some(), "the report sheet");
    let text = rt.state.report.draft.as_ref().expect("the draft").text.clone();
    let block = |what: &str| {
        let open = format!("```json {what}\n");
        let from = text.find(&open).unwrap_or_else(|| panic!("no {what} in {text}")) + open.len();
        text[from..from + text[from..].find("\n```").expect("its end")].to_owned()
    };
    let script = serde_json::from_str(&block("script")).expect("the script is JSON");
    (block("fixture"), script)
}

/// Records `session` on `fx`, replays it, and compares how both ended.
fn round_trip(fx: Arc<fixture::Fixture>, session: impl FnOnce(&mut Runtime<App>)) -> String {
    let mut rt = runtime(fx);
    start(&mut rt);
    session(&mut rt);
    let recorded = state(&rt);
    let (fixture_text, script) = stop(&mut rt);
    let replay_fixture = fixture::parse(&fixture_text).unwrap_or_else(|e| panic!("{e}\n{fixture_text}"));
    let mut replay = runtime(Arc::new(replay_fixture));
    play(&mut replay, &script);
    assert_eq!(state(&replay), recorded, "the replay ends as the recording did\n{script:#}");
    rt.state.report.draft.as_ref().unwrap().text.clone()
}

#[test]
fn a_session_on_the_demo_replays_to_the_same_state() {
    let text = round_trip(fixture::demo(), |rt| {
        tap(rt, "host-prod");
        double_click(rt, "rows/releases");
        tap(rt, "tb-back");
        tap(rt, "rows/index.html");
        hold(rt, SHIFT);
        tap(rt, "rows/robots.txt");
        hold(rt, Mods::default());
        assert_eq!(rt.state.conn().unwrap().selection.len(), 5, "a Shift range");
        press(rt, Key::Command('l'), CMD);
        type_text(rt, "/root");
        press(rt, Key::Enter, Mods::default());
        tap(rt, "state-go-up");
        double_click(rt, "rows/var");
        double_click(rt, "rows/www");
        double_click(rt, "rows/site");
        tap(rt, "rows/README.md");
        press(rt, Key::Function(2), Mods::default());
        press(rt, Key::Command('a'), CMD);
        type_text(rt, "CHANGES.md");
        press(rt, Key::Enter, Mods::default());
        tap(rt, "rows/robots.txt");
        press(rt, Key::Text(" ".into()), Mods::default());
        assert!(rt.state.ui.preview.is_open(), "the preview");
    });
    assert!(text.contains("(permission denied)") || text.contains("Went to /dir-"), "{text}");
    assert!(text.contains("Named it file-"), "{text}");
}

#[test]
fn signing_in_wrong_then_right_replays_with_placeholders() {
    let text = round_trip(flow("connect-trust-sign-in"), |rt| {
        tap(rt, "host-prod");
        tap(rt, "sheet-submit");
        type_text(rt, "wrong horse");
        press(rt, Key::Enter, Mods::default());
        type_text(rt, "correct horse");
        press(rt, Key::Enter, Mods::default());
        assert!(rt.state.conn().is_some_and(|c| c.opened), "signed in");
        tap(rt, "rows/server.log");
        press(rt, Key::Text(" ".into()), Mods::default());
    });
    assert!(!text.contains("horse"), "{text}");
    assert!(text.contains("the server refused it"), "{text}");
}

#[test]
fn a_row_below_the_fold_a_denied_folder_and_a_rename_replay() {
    let text = round_trip(flow("delete-progress"), |rt| {
        tap(rt, "host-web");
        double_click(rt, "rows/old-build");
        press(rt, Key::End, Mods::default());
        tap(rt, "rows/chunk-035.js");
        hold(rt, SHIFT);
        tap(rt, "rows/chunk-038.js");
        hold(rt, Mods::default());
        assert_eq!(rt.state.conn().unwrap().selection.len(), 4, "a Shift range below the fold");
        tap(rt, "tb-back");
        double_click(rt, "rows/cache");
        tap(rt, "state-go-up");
        tap(rt, "rows/a.log");
        press(rt, Key::Function(2), Mods::default());
        press(rt, Key::Command('a'), CMD);
        type_text(rt, "c.log");
        press(rt, Key::Enter, Mods::default());
        tap(rt, "rows/b.log");
        press(rt, Key::Text(" ".into()), Mods::default());
    });
    assert!(
        text.contains("\"type\":\"wheel\",\"key\":\"list\",\"dx\":0,\"dy\":-"),
        "the replay scrolls to the row: {text}"
    );
}

#[test]
fn the_pill_counts_the_steps_and_stop_opens_the_sheet_with_them() {
    let mut rt = runtime(fixture::demo());
    start(&mut rt);
    tap(&mut rt, "host-prod");
    tap(&mut rt, "rows/index.html");
    let pill = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("record-text")).and_then(|e| e.text);
    assert_eq!(pill.as_deref(), Some("Recording · 2 steps"));
    let (_, script) = stop(&mut rt);
    assert!(rt.find("record").is_none(), "the pill goes");
    assert!(rt.find("report-record").is_none(), "no Record Steps… on a report with a recording");
    let events = script["events"].as_array().unwrap();
    assert_eq!(events.last().map(|e| e["name"].clone()), Some(Value::from("end")));
    tap(&mut rt, "report-cancel");
    assert!(rt.state.report.draft.is_none() && rt.state.recording_steps().is_none(), "Cancel discards it");
}
