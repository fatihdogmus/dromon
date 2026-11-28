//! Recording steps: what a step is, what servers' answers it keeps, the fixture and script it
//! builds (that they parse, and name rows alike), sign-in without the typed secrets, the limits,
//! actions the palette and menus run, a folder too big to go whole, a recording started on an
//! open host.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use dromon_core::{ConnStatus, HostId, RemotePath};
use serde_json::Value;

use super::fixture::CAP;
use super::*;
use crate::fixture;
use crate::state::report::Draft;
use crate::state::{Effect, PaletteAction, PreviewAction};

/// A clock that stands still: a fixture's is shared by the process, and tests on other fixtures
/// move it.
fn still() -> i64 {
    0
}

fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.wall_ms = still;
    s.report.about = Some(crate::state::report::About {
        version: "9.9.0".into(),
        os: "Linux".into(),
        arch: "x86_64".into(),
        desktop: None,
    });
    s
}

fn on(json: &str) -> App {
    let mut s = fixture::app(Arc::new(fixture::parse(json).expect("the fixture")));
    s.wall_ms = still;
    s
}

fn prod() -> HostId {
    HostId("prod".into())
}

fn record(s: &mut App) {
    s.act(Action::Report(ReportAction::Record));
    assert_eq!(s.recording_steps(), Some(0));
}

fn stop(s: &mut App) -> Draft {
    s.act(Action::Report(ReportAction::StopRecording));
    assert_eq!(s.recording_steps(), None);
    s.report.draft.clone().expect("the sheet")
}

fn click(s: &mut App, name: &str, extend: bool) {
    s.act(Action::Click { name: name.into(), extend, toggle: false });
}

/// The block fenced with ```` ```json <what> ````, from a report's text.
fn block(text: &str, what: &str) -> String {
    let open = format!("```json {what}\n");
    let from = text.find(&open).unwrap_or_else(|| panic!("no {what} block in {text}")) + open.len();
    let len = text[from..].find("\n```").expect("the block's end");
    text[from..from + len].to_owned()
}

fn events(draft: &Draft) -> Vec<Value> {
    let script: Value = serde_json::from_str(&block(&draft.text, "script")).expect("the script is JSON");
    script["events"].as_array().expect("events").clone()
}

fn steps(s: &App) -> Vec<Step> {
    s.report.recording.as_ref().expect("recording").steps.iter().map(|t| t.step.clone()).collect()
}

#[test]
fn a_recording_takes_each_action_as_a_step_and_folds_letters_typed_to_select() {
    let mut s = demo();
    record(&mut s);
    s.act(Action::SelectHost(prod()));
    s.act(Action::Open("releases".into()));
    s.act(Action::Back);
    click(&mut s, "index.html", false);
    click(&mut s, "robots.txt", true);
    for (i, c) in ["f", "a", "v"].into_iter().enumerate() {
        s.act(Action::Type { text: c.into(), now: i as f64 * 0.1 });
    }
    s.act(Action::Preview(PreviewAction::Toggle));
    let row = |name: &str, dir| Row { name: name.into(), dir };
    assert_eq!(
        steps(&s),
        [
            Step::Connect(prod()),
            Step::Open(row("releases", true)),
            Step::Back,
            Step::Click { row: row("index.html", false), extend: false, toggle: false },
            Step::Click { row: row("robots.txt", false), extend: true, toggle: false },
            Step::TypeSelect(Some(row("favicon.ico", false))),
            Step::Preview(Some(row("favicon.ico", false))),
        ]
    );
    let rec = s.report.recording.as_ref().unwrap();
    let after = rec.steps[1].after.as_ref().unwrap();
    assert_eq!(after.folder, Some(RemotePath::new("/var/www/site/releases")), "where it left the window");
    let server = &rec.servers[&prod()];
    assert_eq!(server.home, Some(RemotePath::new("/var/www/site")));
    assert!(server.folders.contains_key(&RemotePath::new("/var/www/site/releases")), "its listing was kept");
    assert!(server.opened && !server.failed);
}

#[test]
fn the_fixture_parses_and_the_script_names_its_rows() {
    let mut s = demo();
    record(&mut s);
    s.act(Action::SelectHost(prod()));
    s.act(Action::GoTo(RemotePath::new("/root")));
    s.act(Action::GoUp);
    s.act(Action::GoTo(RemotePath::new("/var/www/site")));
    click(&mut s, "index.html", false);
    let d = stop(&mut s);
    let fx = fixture::parse(&block(&d.text, "fixture")).unwrap_or_else(|e| panic!("{e}\n{}", d.text));
    assert_eq!(fx.hosts.len(), 1, "only the host the steps used");
    let server = fx.server("host-1").expect("host-1's server");
    assert!(server.auth.is_empty(), "the demo asks nothing");
    let site = server.list(&server.home).expect("home lists");
    let tapped: Vec<String> = events(&d)
        .iter()
        .filter_map(|e| e["key"].as_str().and_then(|k| k.strip_prefix("rows/")).map(str::to_owned))
        .collect();
    assert!(!tapped.is_empty());
    for name in &tapped {
        assert!(site.iter().any(|e| &e.name == name), "{name} isn't in the fixture's home: {site:?}");
    }
    let denied = fixture::Node::Error(fixture::TreeError::PermissionDenied);
    assert!(server.tree.values().any(|n| *n == denied), "the denied folder keeps its error");
    assert!(d.text.contains("\n  2. Went to /dir-"), "{}", d.text);
}

const SIGN_IN: &str = r#"{"version": 1, "now": 1795528800,
  "hosts": [{"alias": "acme-db", "source": "saved", "user": "zelda", "hostname": "db.acme.example"}],
  "servers": {"acme-db": {"home": "/home/zelda", "auth": [
    {"host-key": {"key_type": "ED25519", "fingerprint": "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU"}},
    {"password": {"accept": "correct horse"}}],
    "tree": {"/home/zelda": [{"name": "notes.txt", "kind": "file", "size": 120}]}}}}"#;

/// Types `answer` into the open prompt sheet and presses its primary button.
fn answer(s: &mut App, answer: &str) {
    s.ui.sheets.edit_mut().text = answer.into();
    s.act(Action::SubmitPrompt);
}

#[test]
fn sign_in_is_kept_by_kind_and_what_was_typed_never() {
    let mut s = on(SIGN_IN);
    record(&mut s);
    s.act(Action::SelectHost(HostId("acme-db".into())));
    s.act(Action::SubmitPrompt);
    answer(&mut s, "s3cret-wrong-guess");
    answer(&mut s, "correct horse");
    assert!(s.conn().is_some_and(|c| c.opened), "signed in");
    let d = stop(&mut s);
    for secret in ["s3cret-wrong-guess", "correct horse", "zelda", "acme", "47DEQpj8"] {
        assert!(!d.text.contains(secret), "{secret} in {}", d.text);
        assert!(!d.url.contains(secret), "{secret} in the link");
    }
    let fx = fixture::parse(&block(&d.text, "fixture")).expect("the fixture");
    let auth = &fx.server("host-1").unwrap().auth;
    assert_eq!(auth.len(), 2, "{auth:?}");
    assert!(matches!(&auth[0], fixture::Step::HostKey { key_type, .. } if key_type == "ED25519"));
    assert!(matches!(&auth[1], fixture::Step::Password { accept, prompt: None } if accept == "pw"));
    let typed: Vec<String> = events(&d).iter().filter_map(|e| e["text"].as_str().map(str::to_owned)).collect();
    assert_eq!(typed, ["wrong", "pw"], "a refused answer replays as a wrong one: {}", d.text);
    assert!(d.text.contains("Answered the password (what you typed isn't recorded; the server refused it)"));
}

#[test]
fn a_third_refusal_is_the_answers_and_a_failure_without_one_is_the_servers() {
    let mut s = on(SIGN_IN);
    record(&mut s);
    s.act(Action::SelectHost(HostId("acme-db".into())));
    s.act(Action::SubmitPrompt);
    for _ in 0..3 {
        answer(&mut s, "nope");
    }
    assert!(matches!(s.conn().unwrap().status, dromon_core::ConnStatus::Failed(_)));
    let rec = s.report.recording.as_ref().unwrap();
    let wrong = rec.steps.iter().filter(|t| matches!(t.step, Step::Answer { wrong: true, .. })).count();
    assert_eq!(wrong, 3);
    assert!(!rec.servers[&HostId("acme-db".into())].failed, "no `fail`: the answers did it");
    let mut other = demo();
    other.backend = Box::new(crate::state::Silent::default());
    record(&mut other);
    other.act(Action::SelectHost(HostId("203.0.113.7".into())));
    other.apply(ServiceEvent::Status { host: HostId("203.0.113.7".into()), status: ConnStatus::Failed("x".into()) });
    let d = stop(&mut other);
    assert!(block(&d.text, "fixture").contains("\"fail\""), "{}", d.text);
    assert!(!d.text.contains("203.0.113.7"), "{}", d.text);
}

static CLOCK: AtomicI64 = AtomicI64::new(0);

fn clock() -> i64 {
    CLOCK.load(Ordering::Relaxed)
}

#[test]
fn fifty_steps_or_fifteen_minutes_stop_it_and_open_the_sheet() {
    let mut s = demo();
    s.act(Action::SelectHost(prod()));
    record(&mut s);
    for i in 0..MAX_STEPS {
        assert_eq!(s.recording_steps(), Some(i));
        s.act(Action::Move { delta: if i % 2 == 0 { 1 } else { -1 }, extend: false });
    }
    assert_eq!(s.recording_steps(), None, "stopped");
    assert_eq!(s.report.draft.as_ref().and_then(|d| d.recorded.as_ref()).map(|r| r.steps), Some(MAX_STEPS));
    let mut s = demo();
    s.wall_ms = clock;
    CLOCK.store(1_000, Ordering::Relaxed);
    record(&mut s);
    s.act(Action::SelectHost(prod()));
    CLOCK.store(1_000 + MAX_MS, Ordering::Relaxed);
    s.act(Action::GoUp);
    assert_eq!(s.recording_steps(), None, "time's up");
    assert!(s.report.is_open());
}

#[test]
fn what_the_palette_and_menus_run_is_the_step_and_what_an_action_runs_is_part_of_it() {
    let mut s = demo();
    s.act(Action::SelectHost(prod()));
    record(&mut s);
    s.act(Action::Palette(PaletteAction::OpenPath));
    s.ui.palette.edit = rune::TextEdit::new("/var/www");
    s.act(Action::Palette(PaletteAction::Edited));
    s.act(Action::Palette(PaletteAction::Run));
    s.act(Action::File(FileAction::OpenMenu { kind: MenuKind::Row("html".into()), at: None }));
    s.act(Action::File(FileAction::Pick { kind: MenuKind::Row("html".into()), id: "rename".into() }));
    s.act(Action::File(FileAction::EditCancel));
    s.act(Action::OpenFilter);
    s.act(Action::FilterChanged("si".into()));
    s.act(Action::Escape);
    let html = Row { name: "html".into(), dir: true };
    assert_eq!(
        steps(&s),
        [
            Step::GoTo(RemotePath::new("/var/www")),
            Step::Click { row: html.clone(), extend: false, toggle: false },
            Step::Rename(html),
            Step::EditCancel,
            Step::NotReplayed(FILTER),
        ]
    );
}

#[test]
fn a_folder_too_big_for_the_report_keeps_the_rows_touched_and_sums_up_the_rest() {
    let mut s = demo();
    record(&mut s);
    s.act(Action::SelectHost(prod()));
    s.act(Action::Open("releases".into()));
    for path in ["/", "/var", "/home", crate::scenes::BIG_FOLDER] {
        s.act(Action::GoTo(RemotePath::new(path)));
    }
    click(&mut s, "IMG_00042.jpg", false);
    let d = stop(&mut s);
    let text = block(&d.text, "fixture");
    assert!(text.len() < CAP + 4_000, "{} characters", text.len());
    assert_eq!(d.recorded.as_ref().map(|r| r.summarised), Some(1));
    let fx = fixture::parse(&text).expect("the fixture");
    let server = fx.server("host-1").unwrap();
    let listed = server
        .tree
        .keys()
        .filter_map(|p| server.list(p).ok())
        .find(|rows| rows.len() == 10_000)
        .expect("the photos' folder, with as many rows");
    let clicked =
        events(&d).iter().filter_map(|e| e["key"].as_str()?.strip_prefix("rows/").map(str::to_owned)).next_back();
    let clicked = clicked.expect("the photo's tap");
    assert!(listed.iter().any(|e| e.name == clicked), "{clicked} stays whole");
    assert!(d.copies.is_some(), "too long for the link: Open GitHub copies it");
    let fx = s.act(Action::Report(ReportAction::OpenGitHub));
    assert!(matches!(&fx[..], [Effect::Copy(c), Effect::OpenUrl(_)] if c.starts_with("```json fixture")), "{fx:?}");
}

#[test]
fn a_recording_started_on_an_open_host_replays_by_going_there_first() {
    let mut s = demo();
    s.act(Action::SelectHost(prod()));
    s.act(Action::Open("releases".into()));
    record(&mut s);
    click(&mut s, "20261125-1348", false);
    let d = stop(&mut s);
    assert!(d.recorded.as_ref().is_some_and(|r| r.mid_session));
    assert!(d.text.contains("Started with host-1 open, in /dir-"), "{}", d.text);
    let ev = events(&d);
    assert_eq!(ev[0]["key"], "host-host-1", "{ev:?}");
    assert_eq!((ev[1]["name"].as_str(), ev[1]["cmd"].as_bool()), (Some("l"), Some(true)), "⌘L, then the path");
    let fx = fixture::parse(&block(&d.text, "fixture")).expect("the fixture");
    let path = RemotePath::new(ev[2]["text"].as_str().unwrap());
    let rows = fx.server("host-1").unwrap().list(&path).expect("the folder it went to lists");
    assert!(rows.iter().any(|e| ev.iter().any(|v| v["key"].as_str() == Some(&format!("rows/{}", e.name)))));
}
