//! Properties of the poll schedule, for any folder size, visibility and run of polls: the base
//! interval stays in its table (2 to 20 s, or no polling), never shortens for a bigger folder
//! and doubles when unfocused; the delay stays between the base and eight times it, backs off
//! after a slow poll and comes back after a fast one; not polling leaves the backoff alone, and
//! a reset returns to the base.

use std::time::Duration;

use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::{PollSchedule, Visibility, base_interval};

#[hegel::composite]
fn entry_counts(tc: &TestCase) -> usize {
    tc.draw(hegel::one_of!(
        gs::sampled_from([0, 999, 1_000, 4_999, 5_000, 20_000, 20_001, usize::MAX].as_slice()),
        gs::integers::<usize>().max_value(25_000),
        gs::integers::<usize>(),
    ))
}

#[hegel::composite]
fn visibilities(tc: &TestCase) -> Visibility {
    tc.draw(gs::sampled_from([Visibility::Focused, Visibility::VisibleUnfocused, Visibility::Hidden].as_slice()))
}

/// How long a poll took. The caller measures it with a clock, so it is bounded far above any
/// real poll (about 36 billion years): `next_delay` multiplies it by 8, which overflows a
/// `Duration` past `u64::MAX / 8` seconds.
#[hegel::composite]
fn poll_times(tc: &TestCase) -> Duration {
    tc.draw(hegel::one_of!(
        gs::integers::<u64>().max_value(200_000).map(Duration::from_millis),
        gs::temporal::std_durations().max_value(Duration::from_secs(u64::MAX / 16)),
    ))
}

#[hegel::composite]
fn polls(tc: &TestCase) -> Vec<(usize, Visibility, Duration)> {
    tc.draw(gs::vecs(hegel::tuples!(entry_counts(), visibilities(), poll_times())).max_size(12))
}

#[hegel::test]
fn the_base_interval_is_two_to_twenty_seconds_or_none(tc: TestCase) {
    let entries = tc.draw(entry_counts());
    let vis = tc.draw(visibilities());
    let base = base_interval(entries, vis);
    assert_eq!(base.is_none(), vis == Visibility::Hidden || entries > 20_000, "{entries} {vis:?}: {base:?}");
    if let Some(b) = base {
        assert!((Duration::from_secs(2)..=Duration::from_secs(20)).contains(&b), "{entries} {vis:?}: {b:?}");
    }
}

#[hegel::test]
fn a_bigger_folder_never_polls_more_often(tc: TestCase) {
    let a = tc.draw(entry_counts());
    let b = tc.draw(entry_counts());
    let vis = tc.draw(visibilities());
    let (small, big) = (a.min(b), a.max(b));
    match (base_interval(small, vis), base_interval(big, vis)) {
        (Some(s), Some(g)) => assert!(s <= g, "{small}: {s:?}, {big}: {g:?}"),
        (None, g) => assert_eq!(g, None, "{small} is not polled but {big} is"),
        (Some(_), None) => {}
    }
}

#[hegel::test]
fn an_unfocused_window_polls_half_as_often(tc: TestCase) {
    let entries = tc.draw(entry_counts());
    let focused = base_interval(entries, Visibility::Focused);
    assert_eq!(base_interval(entries, Visibility::VisibleUnfocused), focused.map(|d| d * 2));
}

#[hegel::test]
fn the_delay_stays_between_the_base_and_eight_times_it(tc: TestCase) {
    let polls = tc.draw(polls());
    let mut s = PollSchedule::new();
    for (entries, vis, took) in polls {
        let delay = s.next_delay(entries, vis, took);
        assert_eq!(delay.is_some(), base_interval(entries, vis).is_some());
        if let (Some(d), Some(base)) = (delay, base_interval(entries, vis)) {
            assert!(base <= d && d <= base * PollSchedule::MAX_FACTOR, "{d:?} for a base of {base:?}");
            assert_eq!(d, base * s.factor());
        }
        assert!([1, 2, 4, 8].contains(&s.factor()), "{}", s.factor());
    }
}

#[hegel::test]
fn a_slow_poll_backs_off_and_a_fast_one_comes_back(tc: TestCase) {
    let polls = tc.draw(polls());
    let mut s = PollSchedule::new();
    for (entries, vis, took) in polls {
        let (Some(base), Some(before)) = (base_interval(entries, vis), s.current(entries, vis)) else { continue };
        let after = s.next_delay(entries, vis, took).expect("a polled folder gets a delay");
        if took * 4 > before {
            // More than a quarter of the interval: doubled, up to the cap.
            assert_eq!(after, (before * 2).min(base * PollSchedule::MAX_FACTOR), "{took:?} of {before:?}");
        } else if took * 8 <= before {
            // At most a quarter of the halved interval: halved, down to the base.
            assert_eq!(after, (before / 2).max(base), "{took:?} of {before:?}");
        } else {
            assert_eq!(after, before, "{took:?} of {before:?}");
        }
    }
}

#[hegel::test]
fn not_polling_leaves_the_backoff_as_it_is(tc: TestCase) {
    let polls = tc.draw(polls());
    let entries = tc.draw(entry_counts());
    let vis = tc.draw(visibilities());
    let took = tc.draw(poll_times());
    tc.assume(base_interval(entries, vis).is_none());
    let mut s = PollSchedule::new();
    for (entries, vis, took) in polls {
        s.next_delay(entries, vis, took);
    }
    let before = s.factor();
    assert_eq!(s.next_delay(entries, vis, took), None);
    assert_eq!(s.current(entries, vis), None);
    assert_eq!(s.factor(), before);
}

#[hegel::test]
fn a_reset_returns_to_the_base(tc: TestCase) {
    let polls = tc.draw(polls());
    let entries = tc.draw(entry_counts());
    let vis = tc.draw(visibilities());
    let mut s = PollSchedule::new();
    for (entries, vis, took) in polls {
        s.next_delay(entries, vis, took);
    }
    s.reset();
    assert_eq!(s.factor(), 1);
    assert_eq!(s.current(entries, vis), base_interval(entries, vis));
}

#[hegel::test]
fn reading_the_schedule_does_not_move_it(tc: TestCase) {
    let polls = tc.draw(polls());
    let entries = tc.draw(entry_counts());
    let vis = tc.draw(visibilities());
    let mut s = PollSchedule::new();
    for (entries, vis, took) in polls {
        s.next_delay(entries, vis, took);
    }
    let before = s;
    assert_eq!(s.current(entries, vis), base_interval(entries, vis).map(|b| b * s.factor()));
    assert_eq!(s, before);
}
