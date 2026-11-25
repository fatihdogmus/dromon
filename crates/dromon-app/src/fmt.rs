//! Text for numbers and dates in the list and the status bar. Pure functions of their inputs:
//! "now" is always passed in, so goldens and tests never depend on the clock.
//!
//! Dates are calendar dates in a fixed UTC offset (`offset_secs`); the app passes 0 until it
//! reads the local zone (no time-zone crate yet).

use dromon_core::Entry;

/// Decimal SI sizes as Finder writes them: "512 bytes", "1.2 KB", "14.8 KB", "148 KB", "18.2 MB".
/// One decimal below 100 of a unit (dropped when it is ".0"), none above; a value that would
/// round to 1,000 moves up a unit.
pub fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return if bytes == 1 { "1 byte".into() } else { format!("{bytes} bytes") };
    }
    const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
    let mut v = bytes as f64;
    for (i, unit) in UNITS.iter().enumerate() {
        v /= 1000.0;
        if v >= 999.5 && i + 1 < UNITS.len() {
            continue;
        }
        let s = if v < 99.95 { format!("{v:.1}") } else { format!("{v:.0}") };
        let s = s.strip_suffix(".0").unwrap_or(&s).to_owned();
        return format!("{s} {unit}");
    }
    unreachable!("u64 fits in exabytes")
}

/// The Size column: a dash for anything you navigate into.
pub fn entry_size(e: &Entry) -> String {
    if e.kind.is_dir_like() { "—".into() } else { size(e.size) }
}

/// `10000` → `"10,000"`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "1 item", "42 items", "10,000 items".
pub fn items(n: usize) -> String {
    if n == 1 { "1 item".into() } else { format!("{} items", thousands(n as u64)) }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Days since 1970-01-01 to (year, month 1–12, day 1–31), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// "Nov 3, 2026".
pub fn date(secs: i64, offset_secs: i64) -> String {
    let (y, m, d) = civil((secs + offset_secs).div_euclid(86_400));
    format!("{} {d}, {y}", MONTHS[m as usize - 1])
}

/// The Modified column: "Just now", "12 min ago", "3 hr ago" (today), "Yesterday", then the
/// date. Times in the future (clock skew) show their date.
pub fn modified(mtime: Option<i64>, now: i64) -> String {
    modified_in(mtime, now, 0)
}

/// [`modified`] in a fixed UTC offset.
pub fn modified_in(mtime: Option<i64>, now: i64, offset_secs: i64) -> String {
    let Some(t) = mtime else { return "—".into() };
    let ago = now - t;
    if ago < 0 {
        return date(t, offset_secs);
    }
    if ago < 60 {
        return "Just now".into();
    }
    if ago < 3600 {
        return format!("{} min ago", ago / 60);
    }
    let day = |s: i64| (s + offset_secs).div_euclid(86_400);
    match day(now) - day(t) {
        0 => format!("{} hr ago", ago / 3600),
        1 => "Yesterday".into(),
        _ => date(t, offset_secs),
    }
}

/// A log timestamp: `14:00:01.234` (UTC, like the dates, until the local zone is read).
pub fn clock_ms(ms: i64) -> String {
    let day_ms = ms.rem_euclid(86_400_000);
    let (h, m, s, milli) = (day_ms / 3_600_000, day_ms / 60_000 % 60, day_ms / 1000 % 60, day_ms % 1000);
    format!("{h:02}:{m:02}:{s:02}.{milli:03}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dromon_core::EntryKind;

    #[test]
    fn clock_times_are_padded() {
        assert_eq!(clock_ms(0), "00:00:00.000");
        assert_eq!(clock_ms(NOW * 1000 + 61_005), "14:01:01.005");
        assert_eq!(clock_ms(-1), "23:59:59.999");
    }

    #[test]
    fn sizes_read_like_finder() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1000), "1 KB");
        assert_eq!(size(1_234), "1.2 KB");
        assert_eq!(size(14_800), "14.8 KB");
        assert_eq!(size(148_000), "148 KB");
        assert_eq!(size(18_200_000), "18.2 MB");
        assert_eq!(size(4_200_000_000), "4.2 GB");
    }

    #[test]
    fn sizes_roll_over_instead_of_reading_1000() {
        assert_eq!(size(999_499), "999 KB");
        assert_eq!(size(999_600), "1 MB");
        assert_eq!(size(99_960), "100 KB");
        assert_eq!(size(u64::MAX), "18.4 EB");
    }

    #[test]
    fn folders_have_no_size() {
        let dir = Entry { name: "a".into(), kind: EntryKind::Dir, size: 4096, mtime: None, mode: None };
        assert_eq!(entry_size(&dir), "—");
        let file = Entry { kind: EntryKind::File, ..dir };
        assert_eq!(entry_size(&file), "4.1 KB");
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(items(1), "1 item");
        assert_eq!(items(10_000), "10,000 items");
    }

    // 2026-11-25 14:00:00 UTC.
    const NOW: i64 = 1_795_615_200;

    #[test]
    fn dates_are_civil() {
        assert_eq!(date(0, 0), "Jan 1, 1970");
        assert_eq!(date(NOW, 0), "Nov 25, 2026");
        assert_eq!(date(951_782_400, 0), "Feb 29, 2000");
        assert_eq!(date(NOW, 11 * 3600), "Nov 26, 2026");
    }

    #[test]
    fn modified_is_relative_when_recent() {
        assert_eq!(modified(None, NOW), "—");
        assert_eq!(modified(Some(NOW - 5), NOW), "Just now");
        assert_eq!(modified(Some(NOW - 120), NOW), "2 min ago");
        assert_eq!(modified(Some(NOW - 12 * 60), NOW), "12 min ago");
        assert_eq!(modified(Some(NOW - 3 * 3600), NOW), "3 hr ago");
        assert_eq!(modified(Some(NOW - 20 * 3600), NOW), "Yesterday");
        assert_eq!(modified(Some(NOW - 22 * 86_400), NOW), "Nov 3, 2026");
        assert_eq!(modified(Some(NOW + 86_400), NOW), "Nov 26, 2026");
    }

    #[test]
    fn yesterday_follows_the_offset() {
        let now = NOW - 13 * 3600; // 01:00 UTC on the 25th, 03:00 in UTC+2
        let t = now - 3 * 3600; // 22:00 UTC on the 24th, 00:00 on the 25th in UTC+2
        assert_eq!(modified_in(Some(t), now, 2 * 3600), "3 hr ago");
        assert_eq!(modified_in(Some(t), now, 0), "Yesterday");
    }
}
