use dromon_core::{EntryKind, HostId, PromptKind};

use super::*;

const A: WindowId = WindowId(0);
const B: WindowId = WindowId(1);

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

fn few() -> Collector {
    let mut got = Collector::default();
    let entries: Vec<Entry> = (0..3).map(|i| Entry { name: format!("f{i}"), kind: EntryKind::File, size: 1, mtime: None, mode: None, owner: None, group: None }).collect();
    got.copy(&entries);
    got
}

/// `path` listed for the windows at `now`.
fn listed(w: &mut Watch, path: &RemotePath, now: Instant) {
    w.listing(path);
    w.listed(path, few(), Duration::ZERO, now);
}

#[test]
fn two_windows_in_two_folders_are_both_polled_while_shown() {
    let t0 = Instant::now();
    let mut w = Watch::default();
    w.window(A, Visibility::Focused, t0);
    w.window(B, Visibility::VisibleUnfocused, t0);
    w.show(A, p("/srv/a"), t0);
    w.show(B, p("/srv/b"), t0);
    assert_eq!(w.folders(), [p("/srv/a"), p("/srv/b")]);
    assert_eq!(w.due(), None, "nothing is polled before it's listed");
    listed(&mut w, &p("/srv/a"), t0);
    listed(&mut w, &p("/srv/b"), t0);
    // A small folder: 2 s focused, 4 s visible but unfocused (§4.5).
    assert_eq!(w.polls[&p("/srv/a")].due(), Some(t0 + Duration::from_secs(2)));
    assert_eq!(w.polls[&p("/srv/b")].due(), Some(t0 + Duration::from_secs(4)));
    assert_eq!(w.next_due(), Some(p("/srv/a")));
    assert!(w.may_poll(&p("/srv/a")) && w.may_poll(&p("/srv/b")));
}

#[test]
fn a_hidden_window_folder_waits_while_a_visible_one_is_polled() {
    let t0 = Instant::now();
    let mut w = Watch::default();
    w.window(A, Visibility::Hidden, t0);
    w.window(B, Visibility::Focused, t0);
    w.show(A, p("/a"), t0);
    w.show(B, p("/b"), t0);
    listed(&mut w, &p("/a"), t0);
    listed(&mut w, &p("/b"), t0);
    assert_eq!(w.polls[&p("/a")].due(), None, "hidden: not polled");
    assert!(!w.may_poll(&p("/a")) && !w.visible(&p("/a")));
    assert_eq!(w.next_due(), Some(p("/b")));
    // A shows again: polled again, from now.
    let t1 = t0 + Duration::from_secs(1);
    w.window(A, Visibility::VisibleUnfocused, t1);
    assert_eq!(w.polls[&p("/a")].due(), Some(t1 + Duration::from_secs(4)));
    // B's schedule didn't move.
    assert_eq!(w.polls[&p("/b")].due(), Some(t0 + Duration::from_secs(2)));
}

#[test]
fn one_folder_in_two_windows_is_polled_once_as_the_best_shown_of_them() {
    let t0 = Instant::now();
    let mut w = Watch::default();
    w.window(A, Visibility::Hidden, t0);
    w.window(B, Visibility::VisibleUnfocused, t0);
    w.show(A, p("/x"), t0);
    w.show(B, p("/x"), t0);
    assert_eq!(w.folders(), [p("/x")]);
    listed(&mut w, &p("/x"), t0);
    assert_eq!(w.polls[&p("/x")].visibility(), Visibility::VisibleUnfocused);
    w.window(A, Visibility::Focused, t0);
    assert_eq!(w.polls[&p("/x")].visibility(), Visibility::Focused);
    assert_eq!(w.focused(), Some(A));
    // B moves on: /x is A's alone, then nobody's once A closes.
    w.show(B, p("/y"), t0);
    assert_eq!(w.folders(), [p("/x"), p("/y")]);
    w.closed(A, t0);
    assert_eq!(w.folders(), [p("/y")]);
    assert_eq!(w.polled(&p("/x"), Ok(few()), Duration::ZERO, t0), Outcome::Quiet, "a poll of a folder nobody shows");
    assert_eq!(w.folder(B), Some(&p("/y")));
    assert_eq!(w.focused(), None);
}

#[test]
fn a_fresh_session_forgets_the_folders_but_not_the_windows() {
    let t0 = Instant::now();
    let mut w = Watch::default();
    w.window(A, Visibility::Focused, t0);
    w.show(A, p("/a"), t0);
    w.forget();
    assert!(w.folders().is_empty() && w.folder(A).is_none());
    assert_eq!(w.focused(), Some(A));
}

fn prompt(id: u64) -> Prompt {
    Prompt { id: PromptId(id), host: HostId("prod".into()), kind: PromptKind::Password, text: "pw: ".into(), error: None }
}

#[test]
fn prompts_go_to_the_focused_window_else_the_one_that_connected() {
    let t0 = Instant::now();
    let mut watch = Watch::default();
    let mut r = Route::default();
    assert_eq!(r.target(&watch), None, "nobody known: every window");
    r.started_by(A);
    assert_eq!(r.target(&watch), Some(A));
    watch.window(A, Visibility::VisibleUnfocused, t0);
    watch.window(B, Visibility::Focused, t0);
    assert_eq!(r.target(&watch), Some(B), "the focused window, when a prompt arrives");
    // B answers: the host's next prompt (a wrong password) asks there again, wherever focus is.
    r.asked(prompt(1), Some(B));
    r.answered(PromptId(1), B);
    watch.window(B, Visibility::VisibleUnfocused, t0);
    watch.window(A, Visibility::Focused, t0);
    assert_eq!(r.target(&watch), Some(B));
    r.settled();
    assert_eq!(r.target(&watch), Some(A));
}

#[test]
fn a_closing_window_hands_its_prompts_on() {
    let t0 = Instant::now();
    let mut watch = Watch::default();
    let mut r = Route::default();
    r.started_by(A);
    watch.window(A, Visibility::Focused, t0);
    watch.window(B, Visibility::VisibleUnfocused, t0);
    r.asked(prompt(1), Some(A));
    r.asked(prompt(2), Some(B));
    watch.closed(A, t0);
    let moved = r.closed(A, &watch);
    // Nobody is focused and the window that connected is gone: every window shows it.
    assert_eq!(moved, [(prompt(1), None)]);
    // With B focused, a third window's prompt would move to B.
    let c = WindowId(2);
    r.asked(prompt(3), Some(c));
    watch.window(B, Visibility::Focused, t0);
    assert_eq!(r.closed(c, &watch), [(prompt(3), Some(B))]);
    r.gone(PromptId(2));
    r.gone(PromptId(3));
    watch.closed(B, t0);
    assert!(r.closed(B, &watch).is_empty(), "only open prompts move");
}

#[test]
fn a_window_joining_hears_the_connections_state() {
    assert_eq!(status_of(&Phase::Idle), None);
    assert_eq!(status_of(&Phase::Starting { attempt: 0, prompts: 0 }), Some(ConnStatus::Connecting));
    assert_eq!(status_of(&Phase::Starting { attempt: 0, prompts: 1 }), Some(ConnStatus::NeedsInput));
    assert_eq!(status_of(&Phase::Starting { attempt: 2, prompts: 0 }), Some(ConnStatus::Reconnecting { attempt: 2 }));
    assert_eq!(status_of(&Phase::Waiting { attempt: 3 }), Some(ConnStatus::Reconnecting { attempt: 3 }));
    assert_eq!(status_of(&Phase::Ready), Some(ConnStatus::Ready));
}

#[test]
fn show_in_hands_the_open_prompts_to_the_asking_window_and_routes_later_ones_there() {
    let t0 = Instant::now();
    let mut watch = Watch::default();
    let mut r = Route::default();
    r.started_by(A);
    watch.window(A, Visibility::Focused, t0);
    watch.window(B, Visibility::VisibleUnfocused, t0);
    assert!(r.show_in(B).is_empty(), "no prompt open: nothing to show");
    r.settled();
    r.asked(prompt(1), Some(A));
    assert_eq!(r.show_in(B), [prompt(1)]);
    // The host's later prompts are B's now, though A has the focus; until it settles.
    assert_eq!(r.target(&watch), Some(B));
    r.settled();
    assert_eq!(r.target(&watch), Some(A));
    // The prompt is B's now: when B closes it moves on.
    r.show_in(B);
    watch.closed(B, t0);
    assert_eq!(r.closed(B, &watch), [(prompt(1), Some(A))]);
}
