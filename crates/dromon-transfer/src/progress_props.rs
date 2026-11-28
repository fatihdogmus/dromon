//! Properties of the progress maths, for any byte counts, times and speeds (NaN and infinities
//! too): a fraction stays in 0–1 and grows with the bytes done, `aggregate` sums with
//! saturation, the speed is never negative or NaN and reads a steady rate as that rate, and the
//! time remaining is shown only when the documented rules allow it.

use std::time::Duration;

use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::{Progress, SpeedMeter, aggregate, eta};

#[hegel::composite]
fn byte_counts(tc: &TestCase) -> u64 {
    tc.draw(hegel::one_of!(
        gs::integers::<u64>().max_value(10),
        gs::sampled_from([u64::MAX - 1, u64::MAX].as_slice()),
        gs::integers::<u64>(),
    ))
}

#[hegel::composite]
fn progresses(tc: &TestCase) -> Progress {
    Progress { done: tc.draw(byte_counts()), total: tc.draw(byte_counts()) }
}

/// A clock reading or a time span: small, at the edges, or anything.
#[hegel::composite]
fn durations(tc: &TestCase) -> Duration {
    tc.draw(hegel::one_of!(
        gs::integers::<u64>().max_value(5_000).map(Duration::from_millis),
        gs::sampled_from([Duration::ZERO, Duration::from_nanos(1), Duration::MAX].as_slice()),
        gs::temporal::std_durations(),
    ))
}

#[hegel::test]
fn a_fraction_stays_between_zero_and_one(tc: TestCase) {
    let p = tc.draw(progresses());
    let f = p.fraction();
    assert!((0.0..=1.0).contains(&f), "{p:?}: {f}");
    if p.total == 0 {
        assert_eq!(f, 0.0);
    }
}

#[hegel::test]
fn a_fraction_grows_with_the_bytes_done(tc: TestCase) {
    let a = tc.draw(byte_counts());
    let b = tc.draw(byte_counts());
    let total = tc.draw(byte_counts());
    let (less, more) = (Progress { done: a.min(b), total }, Progress { done: a.max(b), total });
    assert!(less.fraction() <= more.fraction(), "{less:?} {more:?}");
}

#[hegel::test]
fn aggregate_sums_and_saturates(tc: TestCase) {
    let items = tc.draw(gs::vecs(progresses()).max_size(8));
    let sum = |f: fn(&Progress) -> u64| items.iter().map(|p| u128::from(f(p))).sum::<u128>().min(u128::from(u64::MAX));
    let got = aggregate(&items);
    assert_eq!(u128::from(got.done), sum(|p| p.done));
    assert_eq!(u128::from(got.total), sum(|p| p.total));
}

#[hegel::test]
fn the_speed_is_never_negative_or_nan(tc: TestCase) {
    let samples =
        tc.draw(gs::vecs(hegel::tuples!(durations(), byte_counts(), gs::weighted_booleans(0.1))).max_size(12));
    let mut m = SpeedMeter::new();
    for (now, bytes, reset) in samples {
        if reset {
            m.reset();
        }
        m.record(now, bytes);
        if let Some(s) = m.bytes_per_sec() {
            assert!(s.is_finite() && s >= 0.0, "{s}");
        }
    }
}

#[hegel::test]
fn a_steady_rate_reads_as_that_rate(tc: TestCase) {
    let rate = tc.draw(gs::integers::<u64>().max_value(1_000_000_000));
    let steps = tc.draw(gs::vecs(gs::integers::<u64>().min_value(1).max_value(10_000)).min_size(2).max_size(20));
    let mut m = SpeedMeter::new();
    let mut ms = 0;
    m.record(Duration::ZERO, 0);
    for step in steps {
        ms += step;
        m.record(Duration::from_millis(ms), rate * ms / 1000);
    }
    let speed = m.bytes_per_sec().expect("two samples give a speed");
    // Integer bytes: each sample is off by under a byte.
    let tolerance = 1000.0 + rate as f64 * 1e-9;
    assert!((speed - rate as f64).abs() <= tolerance, "{speed} for {rate} B/s");
}

#[hegel::test]
fn time_remaining_follows_the_documented_rules(tc: TestCase) {
    let remaining = tc.draw(byte_counts());
    let speed = tc.draw(gs::optional(gs::floats::<f64>()));
    let elapsed = tc.draw(durations());
    let got = eta(remaining, speed, elapsed);
    let usable = speed.is_some_and(|s| s.is_finite() && s > 0.0);
    if elapsed < Duration::from_secs(2) || !usable {
        assert_eq!(got, None);
    } else if remaining == 0 {
        assert_eq!(got, Some(Duration::ZERO));
    } else {
        let want = remaining as f64 / speed.unwrap_or(1.0);
        match got {
            Some(d) => assert!((d.as_secs_f64() - want).abs() <= want * 1e-9 + 1e-9, "{d:?} for {want} s"),
            // Only a time too long for a `Duration` has none.
            None => assert!(want >= u64::MAX as f64, "no time remaining for {want} s"),
        }
    }
}
