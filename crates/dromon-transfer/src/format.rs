//! Human formatting of sizes, speeds and durations (Finder style, decimal SI units).
//! See `.plans/dromon-plan.md` §6.8 "Transfers popover".
//!
//! Text uses plain ASCII digits and U+00B7 (middle dot) with single spaces as separator.

use std::time::Duration;

const UNITS: [&str; 7] = ["bytes", "KB", "MB", "GB", "TB", "PB", "EB"];

/// A rounded value in a unit: `UNITS[unit]` is its unit, `text` the number as shown.
struct Scaled {
    text: String,
    unit: usize,
    /// True when the value is exactly one (for "1 byte").
    is_one: bool,
}

impl Scaled {
    fn unit_name(&self) -> &'static str {
        if self.unit == 0 && self.is_one { "byte" } else { UNITS[self.unit] }
    }
}

/// Scales `x` (>= 0) into the largest unit that keeps it >= 1, rounding to one decimal below
/// 100 and to an integer at or above 100, and rolling over to the next unit when rounding would
/// reach 1000 (so "1000 KB" becomes "1 MB").
fn scale(x: f64) -> Scaled {
    let x = if x.is_finite() && x > 0.0 { x } else { 0.0 };
    let mut v = x;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    let mut r = if unit == 0 || v >= 100.0 { v.round() } else { (v * 10.0).round() / 10.0 };
    if r >= 1000.0 && unit < UNITS.len() - 1 {
        r = 1.0;
        unit += 1;
    }
    let text = if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r:.1}") };
    Scaled { text, unit, is_one: r == 1.0 }
}

/// Formats a byte count: "0 bytes", "1 byte", "999 bytes", "1 KB", "14.8 KB", "120 MB".
pub fn bytes(n: u64) -> String {
    let s = scale(n as f64);
    format!("{} {}", s.text, s.unit_name())
}

/// Formats a speed: "8.1 MB/s", "512 bytes/s". Negative and non-finite values show as zero.
pub fn speed(bps: f64) -> String {
    let s = scale(bps);
    format!("{} {}/s", s.text, s.unit_name())
}

/// Friendly, rounded time remaining. Scheme:
///
/// - under 5 s: "a few seconds"
/// - under 60 s: "about N s", N exact (rounded) below 10 s, rounded to 5 s steps from 10 s up
/// - under 60 min: "about N min", N rounded to whole minutes
/// - 1 h up to 10 h: rounded to 10 minutes: "about 1 h", "about 1 h 20 min"
/// - 10 h and more: "about N h", rounded to whole hours
///
/// A value that rounds up into the next band ("60 s", "60 min") is shown in that band
/// ("about 1 min", "about 1 h").
pub fn eta(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 5.0 {
        return "a few seconds".to_owned();
    }
    if secs < 60.0 {
        let n = if secs < 10.0 { secs.round() } else { (secs / 5.0).round() * 5.0 };
        if n < 60.0 {
            return format!("about {n:.0} s");
        }
        return "about 1 min".to_owned();
    }
    if secs < 3600.0 {
        let m = (secs / 60.0).round();
        if m < 60.0 {
            return format!("about {m:.0} min");
        }
        return "about 1 h".to_owned();
    }
    if secs < 36_000.0 {
        let total = ((secs / 600.0).round() * 10.0) as u64;
        let (h, m) = (total / 60, total % 60);
        return if m == 0 { format!("about {h} h") } else { format!("about {h} h {m} min") };
    }
    format!("about {:.0} h", (secs / 3600.0).round())
}

/// One popover row line: "12.4 / 80 MB · 8.1 MB/s · about 9 s".
///
/// - The unit is shared when both sizes use the same one ("12.4 / 80 MB"); otherwise each side
///   has its own ("900 KB / 1.2 MB"). A `done` of zero always shares the total's unit
///   ("0 / 80 MB").
/// - An unknown total (`total == 0`) shows only the done size.
/// - Missing speed or ETA are omitted together with their separator.
pub fn progress_line(done: u64, total: u64, speed_bps: Option<f64>, remaining: Option<Duration>) -> String {
    let mut parts = Vec::with_capacity(3);
    if total == 0 {
        parts.push(bytes(done));
    } else {
        let d = scale(done as f64);
        let t = scale(total as f64);
        if done == 0 || d.unit == t.unit {
            parts.push(format!("{} / {} {}", d.text, t.text, t.unit_name()));
        } else {
            parts.push(format!("{} {} / {} {}", d.text, d.unit_name(), t.text, t.unit_name()));
        }
    }
    if let Some(s) = speed_bps {
        parts.push(speed(s));
    }
    if let Some(e) = remaining {
        parts.push(eta(e));
    }
    parts.join(" \u{b7} ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn bytes_table() {
        let cases: [(u64, &str); 28] = [
            (0, "0 bytes"),
            (1, "1 byte"),
            (2, "2 bytes"),
            (999, "999 bytes"),
            (1000, "1 KB"),
            (1049, "1 KB"),
            (1050, "1.1 KB"),
            (1234, "1.2 KB"),
            (14_800, "14.8 KB"),
            (99_949, "99.9 KB"),
            (99_950, "100 KB"),
            (100_000, "100 KB"),
            (120_400, "120 KB"),
            (999_499, "999 KB"),
            (999_500, "1 MB"),
            (999_999, "1 MB"),
            (1_000_000, "1 MB"),
            (18_200_000, "18.2 MB"),
            (120_000_000, "120 MB"),
            (999_999_999, "1 GB"),
            (1_500_000_000, "1.5 GB"),
            (2_000_000_000_000, "2 TB"),
            (999_999_999_999_999, "1 PB"),
            (1_000_000_000_000_000, "1 PB"),
            (1_000_000_000_000_000_000, "1 EB"),
            (u64::MAX, "18.4 EB"),
            (10_000_000_000_000_000_000, "10 EB"),
            (1_001, "1 KB"),
        ];
        for (n, want) in cases {
            assert_eq!(bytes(n), want, "n = {n}");
        }
    }

    #[test]
    fn speed_table() {
        let cases: [(f64, &str); 14] = [
            (0.0, "0 bytes/s"),
            (-3.0, "0 bytes/s"),
            (f64::NAN, "0 bytes/s"),
            (0.4, "0 bytes/s"),
            (1.0, "1 byte/s"),
            (512.0, "512 bytes/s"),
            (999.6, "1 KB/s"),
            (1000.0, "1 KB/s"),
            (8_100_000.0, "8.1 MB/s"),
            (8_149_999.0, "8.1 MB/s"),
            (999_999.0, "1 MB/s"),
            (125_000_000.0, "125 MB/s"),
            (1_250_000_000.0, "1.3 GB/s"),
            (f64::INFINITY, "0 bytes/s"),
        ];
        for (v, want) in cases {
            assert_eq!(speed(v), want, "v = {v}");
        }
    }

    #[test]
    fn eta_table() {
        let ms = Duration::from_millis;
        let cases: Vec<(Duration, &str)> = vec![
            (s(0), "a few seconds"),
            (ms(4999), "a few seconds"),
            (s(5), "about 5 s"),
            (ms(5400), "about 5 s"),
            (s(9), "about 9 s"),
            (ms(9400), "about 9 s"),
            (ms(9500), "about 10 s"),
            (s(10), "about 10 s"),
            (ms(12_400), "about 10 s"),
            (ms(12_500), "about 15 s"),
            (s(44), "about 45 s"),
            (s(57), "about 55 s"),
            (ms(57_500), "about 1 min"),
            (s(59), "about 1 min"),
            (s(60), "about 1 min"),
            (s(89), "about 1 min"),
            (s(90), "about 2 min"),
            (s(120), "about 2 min"),
            (s(59 * 60), "about 59 min"),
            (s(59 * 60 + 29), "about 59 min"),
            (s(59 * 60 + 30), "about 1 h"),
            (s(3599), "about 1 h"),
            (s(3600), "about 1 h"),
            (s(3600 + 299), "about 1 h"),
            (s(3600 + 300), "about 1 h 10 min"),
            (s(4800), "about 1 h 20 min"),
            (s(2 * 3600 + 40 * 60), "about 2 h 40 min"),
            (s(35_999), "about 10 h"),
            (s(36_000), "about 10 h"),
            (s(36_000 + 1799), "about 10 h"),
            (s(36_000 + 1800), "about 11 h"),
            (s(30 * 3600), "about 30 h"),
        ];
        for (d, want) in cases {
            assert_eq!(eta(d), want, "d = {d:?}");
        }
    }

    #[test]
    fn progress_line_table() {
        type Case = (u64, u64, Option<f64>, Option<Duration>, &'static str);
        let cases: Vec<Case> = vec![
            (12_400_000, 80_000_000, Some(8_100_000.0), Some(s(9)), "12.4 / 80 MB \u{b7} 8.1 MB/s \u{b7} about 9 s"),
            (900_000, 1_200_000, None, None, "900 KB / 1.2 MB"),
            (12_400_000, 80_000_000, Some(8_100_000.0), None, "12.4 / 80 MB \u{b7} 8.1 MB/s"),
            (12_400_000, 80_000_000, None, Some(s(9)), "12.4 / 80 MB \u{b7} about 9 s"),
            (0, 80_000_000, None, None, "0 / 80 MB"),
            (0, 500, Some(0.0), None, "0 / 500 bytes \u{b7} 0 bytes/s"),
            (1, 1, None, None, "1 / 1 byte"),
            (80_000_000, 80_000_000, None, None, "80 / 80 MB"),
            (999_999, 1_000_000, None, None, "1 / 1 MB"),
            (5_000, 0, Some(100.0), None, "5 KB \u{b7} 100 bytes/s"),
            (0, 0, None, None, "0 bytes"),
            (500, 2_000_000_000, None, Some(s(4000)), "500 bytes / 2 GB \u{b7} about 1 h 10 min"),
        ];
        for (done, total, sp, e, want) in cases {
            assert_eq!(progress_line(done, total, sp, e), want, "{done}/{total}");
        }
    }

    #[test]
    fn separator_is_middle_dot_and_text_is_ascii_digits() {
        let line = progress_line(1, 2000, Some(1.0), Some(s(10)));
        assert!(line.contains(" \u{b7} "));
        assert!(!line.contains('\u{2022}'));
        assert!(line.chars().filter(char::is_ascii_digit).count() > 0);
    }
}
