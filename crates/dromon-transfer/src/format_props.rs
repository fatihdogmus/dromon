//! Properties of the human formatting, for any size, speed (NaN and infinities too) and time:
//! nothing panics; sizes and speeds read as a rounded number under 1000 and a unit, close to
//! the value and never smaller for a bigger one; times read in their documented bands, never
//! shorter for a longer time; a progress line joins its parts with middle dots, in ASCII.

use std::time::Duration;

use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::{bytes, eta, progress_line, speed};

const UNITS: [&str; 7] = ["bytes", "KB", "MB", "GB", "TB", "PB", "EB"];

#[hegel::composite]
fn byte_counts(tc: &TestCase) -> u64 {
    tc.draw(hegel::one_of!(
        gs::integers::<u64>().max_value(2_000),
        gs::sampled_from([999_499, 999_500, 99_949, 99_950, 999_999_999_999_999_999, u64::MAX].as_slice()),
        gs::integers::<u64>(),
    ))
}

#[hegel::composite]
fn durations(tc: &TestCase) -> Duration {
    tc.draw(hegel::one_of!(
        gs::integers::<u64>().max_value(40_000_000).map(Duration::from_millis),
        gs::sampled_from([Duration::ZERO, Duration::MAX].as_slice()),
        gs::temporal::std_durations(),
    ))
}

/// The value a size text shows, in bytes: "14.8 KB" is 14,800. `None` when it isn't a number
/// (ASCII digits, at most one decimal) and a unit.
fn shown(text: &str) -> Option<f64> {
    let (number, unit) = text.split_once(' ')?;
    let unit = if unit == "byte" { 0 } else { UNITS.iter().position(|u| *u == unit)? };
    let (int, frac) = number.split_once('.').unwrap_or((number, "0"));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || !digits(frac) || frac.len() != 1 {
        return None;
    }
    Some(number.parse::<f64>().ok()? * 1000f64.powi(unit as i32))
}

/// The seconds a time text says, or `None` when it is none of the documented forms.
fn said(text: &str) -> Option<f64> {
    if text == "a few seconds" {
        return Some(0.0);
    }
    let words: Vec<&str> = text.strip_prefix("about ")?.split(' ').collect();
    let n = |s: &str| s.parse::<f64>().ok().filter(|_| s.bytes().all(|b| b.is_ascii_digit()));
    match words.as_slice() {
        [s, "s"] => n(s).filter(|s| (5.0..60.0).contains(s)),
        [m, "min"] => n(m).filter(|m| (1.0..60.0).contains(m)).map(|m| m * 60.0),
        [h, "h"] => n(h).filter(|h| *h >= 1.0).map(|h| h * 3600.0),
        [h, "h", m, "min"] => {
            let (h, m) = (n(h)?, n(m)?);
            ((1.0..10.0).contains(&h) && [10.0, 20.0, 30.0, 40.0, 50.0].contains(&m)).then_some(h * 3600.0 + m * 60.0)
        }
        _ => None,
    }
}

#[hegel::test]
fn a_size_reads_as_a_number_under_1000_and_a_unit(tc: TestCase) {
    let n = tc.draw(byte_counts());
    let text = bytes(n);
    let value = shown(&text).unwrap_or_else(|| panic!("{n}: {text:?}"));
    let (number, unit) = text.split_once(' ').expect("checked by shown");
    assert!(unit == "EB" || number.parse::<f64>().is_ok_and(|x| x < 1000.0), "{n}: {text:?}");
    assert_eq!(unit == "byte", n == 1, "{n}: {text:?}");
    assert!((value - n as f64).abs() <= n as f64 * 0.05 + 0.5, "{n}: {text:?}");
}

#[hegel::test]
fn a_bigger_size_never_reads_smaller(tc: TestCase) {
    let a = tc.draw(byte_counts());
    let b = tc.draw(byte_counts());
    let (small, big) = (bytes(a.min(b)), bytes(a.max(b)));
    assert!(shown(&small) <= shown(&big), "{small:?} for {}, {big:?} for {}", a.min(b), a.max(b));
}

#[hegel::test]
fn a_speed_reads_as_a_size_per_second(tc: TestCase) {
    let bps = tc.draw(gs::floats::<f64>());
    let text = speed(bps);
    let size = text.strip_suffix("/s").unwrap_or_else(|| panic!("{bps}: {text:?}"));
    assert!(shown(size).is_some(), "{bps}: {text:?}");
    if !bps.is_finite() || bps <= 0.0 {
        assert_eq!(text, "0 bytes/s", "{bps}");
    }
}

#[hegel::test]
fn a_time_reads_in_its_documented_band(tc: TestCase) {
    let d = tc.draw(durations());
    let text = eta(d);
    let secs = said(&text).unwrap_or_else(|| panic!("{d:?}: {text:?}"));
    if d.as_secs_f64() >= 5.0 {
        // Rounded to at most 5 s, a minute, 10 minutes or an hour, by band.
        let step = [(60.0, 2.5), (3600.0, 30.0), (36_000.0, 300.0)]
            .into_iter()
            .find(|(below, _)| d.as_secs_f64() < *below)
            .map_or(1800.0, |(_, half)| half);
        // Plus the float error of seconds up to `Duration::MAX`.
        assert!((secs - d.as_secs_f64()).abs() <= step + d.as_secs_f64() * 1e-15, "{d:?}: {text:?}");
    }
}

#[hegel::test]
fn a_longer_time_never_reads_shorter(tc: TestCase) {
    let a = tc.draw(durations());
    let b = tc.draw(durations());
    let (short, long) = (eta(a.min(b)), eta(a.max(b)));
    assert!(said(&short) <= said(&long), "{short:?} for {:?}, {long:?} for {:?}", a.min(b), a.max(b));
}

#[hegel::test]
fn a_progress_line_joins_its_parts_with_middle_dots(tc: TestCase) {
    let done = tc.draw(byte_counts());
    let total = tc.draw(byte_counts());
    let bps = tc.draw(gs::optional(gs::floats::<f64>()));
    let remaining = tc.draw(gs::optional(durations()));
    let line = progress_line(done, total, bps, remaining);
    let parts: Vec<&str> = line.split(" \u{b7} ").collect();
    assert_eq!(parts.len(), 1 + usize::from(bps.is_some()) + usize::from(remaining.is_some()), "{line:?}");
    assert!(line.chars().all(|c| c.is_ascii() || c == '\u{b7}'), "{line:?}");
    if total == 0 {
        assert_eq!(parts[0], bytes(done));
    }
}
