//! Adaptive polling of the folder on screen (plan §4.5). Pure logic, no timers: the caller
//! sleeps for whatever [`PollSchedule::next_delay`] returns.

use std::time::Duration;

/// How visible the window showing the folder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Visibility {
    Focused,
    VisibleUnfocused,
    /// Minimized, occluded or otherwise not on screen: never poll.
    Hidden,
}

/// The plan's interval table. `None` means don't poll (manual refresh only, or hidden).
///
/// Bands, by entry count (the table's shared endpoints resolved like this):
///
/// | Entries          | Focused | Visible, unfocused |
/// |------------------|---------|--------------------|
/// | 0 – 999          | 2 s     | 4 s                |
/// | 1,000 – 4,999    | 4 s     | 8 s                |
/// | 5,000 – 20,000   | 10 s    | 20 s               |
/// | 20,001 and up    | manual  | manual             |
pub fn base_interval(entries: usize, vis: Visibility) -> Option<Duration> {
    let focused = match entries {
        0..1_000 => 2,
        1_000..5_000 => 4,
        5_000..=20_000 => 10,
        _ => return None,
    };
    match vis {
        Visibility::Focused => Some(Duration::from_secs(focused)),
        Visibility::VisibleUnfocused => Some(Duration::from_secs(focused * 2)),
        Visibility::Hidden => None,
    }
}

/// Backoff on top of [`base_interval`]: a poll that took more than a quarter of the interval
/// doubles the interval (up to [`PollSchedule::MAX_FACTOR`] times the base); a poll that would
/// still be under a quarter of the halved interval halves it again, back down to the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollSchedule {
    factor: u32,
}

impl Default for PollSchedule {
    fn default() -> Self {
        Self::new()
    }
}

impl PollSchedule {
    /// The backoff never exceeds this multiple of the base interval.
    pub const MAX_FACTOR: u32 = 8;

    pub fn new() -> Self {
        PollSchedule { factor: 1 }
    }

    /// The current backoff multiple (1 = no backoff).
    pub fn factor(&self) -> u32 {
        self.factor
    }

    /// Drops the backoff, e.g. after navigating to another folder.
    pub fn reset(&mut self) {
        self.factor = 1;
    }

    /// The delay before the next poll of a folder with `entries` entries, given how long the
    /// last poll took (`Duration::ZERO` before the first). `None`: don't schedule a poll; the
    /// backoff is then left as it is.
    pub fn next_delay(
        &mut self,
        entries: usize,
        vis: Visibility,
        last_poll_took: Duration,
    ) -> Option<Duration> {
        let base = base_interval(entries, vis)?;
        let current = base * self.factor;
        if last_poll_took * 4 > current {
            self.factor = (self.factor * 2).min(Self::MAX_FACTOR);
        } else if self.factor > 1 && last_poll_took * 8 <= current {
            self.factor /= 2;
        }
        Some(base * self.factor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn table_and_boundaries() {
        use Visibility::*;
        for (entries, focused, unfocused) in [
            (0, Some(2), Some(4)),
            (999, Some(2), Some(4)),
            (1_000, Some(4), Some(8)),
            (4_999, Some(4), Some(8)),
            (5_000, Some(10), Some(20)),
            (20_000, Some(10), Some(20)),
            (20_001, None, None),
            (usize::MAX, None, None),
        ] {
            assert_eq!(base_interval(entries, Focused), focused.map(S), "{entries} focused");
            assert_eq!(base_interval(entries, VisibleUnfocused), unfocused.map(S), "{entries}");
            assert_eq!(base_interval(entries, Hidden), None);
        }
    }

    #[test]
    fn fast_polls_keep_the_base_interval() {
        let mut s = PollSchedule::new();
        for _ in 0..5 {
            assert_eq!(s.next_delay(10, Visibility::Focused, Duration::from_millis(100)), Some(S(2)));
        }
        assert_eq!(s.factor(), 1);
    }

    #[test]
    fn slow_polls_back_off_and_cap() {
        let mut s = PollSchedule::new();
        // 2 s interval: exactly 25% (500 ms) is fine, more backs off.
        assert_eq!(s.next_delay(10, Visibility::Focused, Duration::from_millis(500)), Some(S(2)));
        assert_eq!(s.next_delay(10, Visibility::Focused, Duration::from_millis(501)), Some(S(4)));
        // A 3 s poll is over 25% of 4 s and of 8 s; then the cap holds at 8 × 2 s.
        assert_eq!(s.next_delay(10, Visibility::Focused, S(3)), Some(S(8)));
        assert_eq!(s.next_delay(10, Visibility::Focused, S(3)), Some(S(16)));
        assert_eq!(s.next_delay(10, Visibility::Focused, S(30)), Some(S(16)));
        assert_eq!(s.factor(), PollSchedule::MAX_FACTOR);
    }

    #[test]
    fn backoff_decays_when_polls_get_fast() {
        let mut s = PollSchedule::new();
        s.next_delay(10, Visibility::Focused, S(1)); // 4 s
        s.next_delay(10, Visibility::Focused, S(2)); // 8 s
        assert_eq!(s.factor(), 4);
        // Between 1/8 and 1/4 of the current interval: hold.
        assert_eq!(s.next_delay(10, Visibility::Focused, Duration::from_millis(1500)), Some(S(8)));
        // At most 1/8: halve, one step per poll.
        assert_eq!(s.next_delay(10, Visibility::Focused, S(1)), Some(S(4)));
        assert_eq!(s.next_delay(10, Visibility::Focused, Duration::ZERO), Some(S(2)));
        assert_eq!(s.next_delay(10, Visibility::Focused, Duration::ZERO), Some(S(2)));
        assert_eq!(s.factor(), 1);
    }

    #[test]
    fn backoff_scales_with_the_band_and_visibility() {
        let mut s = PollSchedule::new();
        s.next_delay(10, Visibility::Focused, S(1));
        assert_eq!(s.factor(), 2);
        // Same factor applied to the unfocused, larger-folder base (8 s): 1 s isn't slow there,
        // and it's exactly 1/16 of 16 s, so the backoff decays.
        assert_eq!(s.next_delay(2_000, Visibility::VisibleUnfocused, S(1)), Some(S(8)));
    }

    #[test]
    fn hidden_and_manual_dont_schedule_or_touch_the_backoff() {
        let mut s = PollSchedule::new();
        s.next_delay(10, Visibility::Focused, S(1));
        assert_eq!(s.next_delay(10, Visibility::Hidden, S(100)), None);
        assert_eq!(s.next_delay(50_000, Visibility::Focused, S(100)), None);
        assert_eq!(s.factor(), 2);
        s.reset();
        assert_eq!(s.factor(), 1);
    }
}
