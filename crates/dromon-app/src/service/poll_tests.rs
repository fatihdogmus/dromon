use dromon_core::EntryKind;

use super::*;

const MS: fn(u64) -> Duration = Duration::from_millis;
const S: fn(u64) -> Duration = Duration::from_secs;

fn e(name: &str, size: u64) -> Entry {
    Entry { name: name.into(), kind: EntryKind::File, size, mtime: Some(1), mode: None }
}

fn got(entries: &[Entry]) -> Collector {
    let mut c = Collector::default();
    c.copy(entries);
    c
}

fn many(n: usize) -> Vec<Entry> {
    (0..n).map(|i| e(&format!("f{i}"), 1)).collect()
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// A poller with `/w` listed (`a`, `b`), the window focused, at `t0`.
fn listed(t0: Instant) -> Poller {
    let mut p0 = Poller::default();
    p0.window(Visibility::Focused, t0);
    p0.listing(&p("/w"));
    p0.listed(&p("/w"), got(&[e("a", 1), e("b", 2)]), MS(10), t0);
    p0
}

#[test]
fn nothing_is_due_until_the_window_says_it_shows() {
    let t0 = Instant::now();
    let mut poller = Poller::default();
    assert_eq!(poller.visibility(), Visibility::Hidden);
    poller.listing(&p("/w"));
    poller.listed(&p("/w"), got(&[e("a", 1)]), MS(10), t0);
    assert_eq!(poller.due(), None, "hidden by default");
    assert!(!poller.may_poll());
    poller.window(Visibility::Focused, t0);
    assert_eq!(poller.due(), Some(t0 + S(2)));
    assert!(poller.may_poll());
}

#[test]
fn the_interval_follows_focus_and_hiding_stops_it() {
    let t0 = Instant::now();
    let mut poller = listed(t0);
    assert_eq!(poller.due(), Some(t0 + S(2)));
    poller.window(Visibility::VisibleUnfocused, t0 + S(1));
    assert_eq!(poller.due(), Some(t0 + S(5)), "4 s from the change");
    poller.window(Visibility::Hidden, t0 + S(2));
    assert_eq!(poller.due(), None);
    assert!(!poller.may_poll());
    poller.window(Visibility::Focused, t0 + S(9));
    assert_eq!(poller.due(), Some(t0 + S(11)));
}

#[test]
fn a_poll_that_finds_nothing_new_is_quiet_and_reschedules() {
    let t0 = Instant::now();
    let mut poller = listed(t0);
    // Same entries, another order: nothing to tell.
    let out = poller.polled(&p("/w"), Ok(got(&[e("b", 2), e("a", 1)])), MS(30), t0 + S(2));
    assert_eq!(out, Outcome::Quiet);
    assert_eq!(poller.due(), Some(t0 + S(4)));
}

#[test]
fn a_poll_that_finds_a_change_sends_the_whole_folder() {
    let t0 = Instant::now();
    let mut poller = listed(t0);
    let now = [e("a", 1), e("b", 3), e("c", 0)];
    let out = poller.polled(&p("/w"), Ok(got(&now)), MS(30), t0 + S(2));
    assert_eq!(out, Outcome::Changed(now.to_vec()));
    // And the next poll compares with that.
    assert_eq!(poller.polled(&p("/w"), Ok(got(&now)), MS(30), t0 + S(4)), Outcome::Quiet);
}

#[test]
fn slow_polls_back_off() {
    let t0 = Instant::now();
    let mut poller = listed(t0);
    // 1 s of a 2 s interval is over a quarter: 4 s next.
    poller.polled(&p("/w"), Ok(got(&[e("a", 1), e("b", 2)])), S(1), t0 + S(2));
    assert_eq!(poller.due(), Some(t0 + S(6)));
    // The focus changing keeps the backoff: 8 s unfocused.
    poller.window(Visibility::VisibleUnfocused, t0 + S(3));
    assert_eq!(poller.due(), Some(t0 + S(11)));
}

#[test]
fn folders_over_twenty_thousand_entries_are_never_polled() {
    let t0 = Instant::now();
    let mut poller = Poller::default();
    poller.window(Visibility::Focused, t0);
    poller.listing(&p("/big"));
    let mut c = Collector::default();
    for chunk in many(20_001).chunks(512) {
        c.copy(chunk);
    }
    assert_eq!(c.count(), 20_001);
    poller.listed(&p("/big"), c, MS(10), t0);
    assert_eq!(poller.due(), None);
    assert!(!poller.may_poll(), "focus gained: still manual only");
    poller.window(Visibility::Focused, t0 + S(1));
    assert_eq!(poller.due(), None);

    // Exactly 20,000 still is (every 10 s).
    poller.listing(&p("/big"));
    poller.listed(&p("/big"), got(&many(20_000)), MS(10), t0);
    assert_eq!(poller.due(), Some(t0 + S(10)));
}

#[test]
fn a_folder_that_grows_past_the_limit_is_sent_once_then_left_alone() {
    let t0 = Instant::now();
    let mut poller = Poller::default();
    poller.window(Visibility::Focused, t0);
    poller.listing(&p("/big"));
    poller.listed(&p("/big"), got(&many(19_999)), MS(10), t0);
    let mut c = Collector::default();
    c.take(many(20_005));
    match poller.polled(&p("/big"), Ok(c), MS(10), t0 + S(10)) {
        Outcome::Changed(all) => assert_eq!(all.len(), 20_005),
        other => panic!("{other:?}"),
    }
    assert_eq!(poller.due(), None);
    assert!(!poller.may_poll());
}

#[test]
fn a_folder_that_vanished_is_reported_once_and_no_longer_polled() {
    let t0 = Instant::now();
    let mut poller = listed(t0);
    let gone = Error::NotFound(p("/w"));
    assert_eq!(poller.polled(&p("/w"), Err(gone.clone()), MS(5), t0 + S(2)), Outcome::Failed(gone.clone()));
    assert_eq!(poller.due(), None);
    assert!(!poller.may_poll());
    // A second poll (the window regaining focus) finding the same: nothing new.
    assert_eq!(poller.polled(&p("/w"), Err(gone), MS(5), t0 + S(3)), Outcome::Quiet);
    // An explicit listing that fails isn't polled either.
    poller.listing(&p("/w/locked"));
    poller.list_failed(&p("/w/locked"), &Error::PermissionDenied(p("/w/locked")));
    assert_eq!(poller.due(), None);
}

#[test]
fn a_navigation_starts_afresh_and_late_polls_are_ignored() {
    let t0 = Instant::now();
    let mut poller = listed(t0);
    poller.polled(&p("/w"), Ok(got(&[e("a", 1), e("b", 2)])), S(1), t0 + S(2)); // backed off
    poller.listing(&p("/w/sub"));
    assert_eq!(poller.due(), None, "nothing due until the new folder is listed");
    // The old folder's poll, cut off by the navigation, ends late: ignored.
    assert_eq!(poller.polled(&p("/w"), Ok(got(&[e("z", 9)])), MS(5), t0 + S(3)), Outcome::Quiet);
    poller.listed(&p("/w/sub"), got(&[e("x", 1)]), MS(10), t0 + S(3));
    assert_eq!(poller.due(), Some(t0 + S(5)), "the backoff went with the old folder");
    // A relist of the same folder (⌘R) keeps its place and schedule.
    poller.listing(&p("/w/sub"));
    assert_eq!(poller.due(), None);
    poller.listed(&p("/w/sub"), got(&[e("x", 1)]), MS(10), t0 + S(4));
    assert_eq!(poller.due(), Some(t0 + S(6)));
    poller.forget();
    assert_eq!(poller.due(), None);
}

#[test]
fn a_listing_for_the_ui_keeps_entries_only_below_the_limit() {
    let mut c = Collector::default();
    c.copy(&many(20_000));
    assert_eq!((c.count(), c.entries.len()), (20_000, 20_000));
    c.copy(&many(1));
    assert_eq!((c.count(), c.entries.len()), (20_001, 0));
}
