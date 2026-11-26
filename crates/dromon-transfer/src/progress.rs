//! Progress maths: speed EMA, time remaining, aggregate progress. See `.plans/dromon-plan.md`
//! §4.6 and §6.8.

use std::time::Duration;

/// Time constant of the speed moving average.
const TAU_SECS: f64 = 3.0;

/// Minimum amount of data time before an ETA is shown.
const ETA_MIN_ELAPSED: Duration = Duration::from_secs(2);

/// Exponential moving average of transfer speed (bytes per second), time constant ~3 s.
///
/// Samples may arrive at irregular intervals: each update uses `alpha = 1 - exp(-dt / tau)`, so
/// the result does not depend on the sampling rate.
#[derive(Debug, Clone, Default)]
pub struct SpeedMeter {
    last: Option<(Duration, u64)>,
    ema: Option<f64>,
}

impl SpeedMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the cumulative bytes done at monotonic time `now` (arbitrary epoch).
    ///
    /// - The first sample only sets the baseline.
    /// - A sample with `now` not after the previous one is ignored (its bytes are counted by the
    ///   next valid sample).
    /// - If `total_bytes_done` went backwards (the transfer restarted), the sample becomes the
    ///   new baseline and the average is left as is.
    pub fn record(&mut self, now: Duration, total_bytes_done: u64) {
        let Some((last_now, last_bytes)) = self.last else {
            self.last = Some((now, total_bytes_done));
            return;
        };
        if now <= last_now {
            return;
        }
        self.last = Some((now, total_bytes_done));
        if total_bytes_done < last_bytes {
            return;
        }
        let dt = (now - last_now).as_secs_f64();
        let instant = (total_bytes_done - last_bytes) as f64 / dt;
        self.ema = Some(match self.ema {
            None => instant,
            Some(prev) => {
                let alpha = 1.0 - (-dt / TAU_SECS).exp();
                prev + alpha * (instant - prev)
            }
        });
    }

    /// Smoothed speed in bytes per second; `None` until two valid samples have been recorded.
    pub fn bytes_per_sec(&self) -> Option<f64> {
        self.ema
    }

    /// Forgets everything (call on pause or reconnect so the resumed speed is not polluted).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Time remaining. `None` before 2 s of data, or with a missing, zero, negative or non-finite
/// speed. Nothing remaining gives `Some(Duration::ZERO)`.
pub fn eta(remaining: u64, speed: Option<f64>, elapsed_with_data: Duration) -> Option<Duration> {
    if elapsed_with_data < ETA_MIN_ELAPSED {
        return None;
    }
    let speed = speed.filter(|s| s.is_finite() && *s > 0.0)?;
    if remaining == 0 {
        return Some(Duration::ZERO);
    }
    Duration::try_from_secs_f64(remaining as f64 / speed).ok()
}

/// Bytes done and total for a job (or an aggregate of jobs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

impl Progress {
    /// `done / total` clamped to `0.0..=1.0`. An unknown or empty total (`total == 0`) gives
    /// `0.0` whatever `done` is; callers show finished state from `TaskState`, not from this.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        ((self.done as f64 / self.total as f64) as f32).clamp(0.0, 1.0)
    }
}

/// Sums progress across jobs for the toolbar ring (saturating).
pub fn aggregate<'a>(it: impl IntoIterator<Item = &'a Progress>) -> Progress {
    it.into_iter().fold(Progress::default(), |acc, p| Progress {
        done: acc.done.saturating_add(p.done),
        total: acc.total.saturating_add(p.total),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn none_until_two_samples() {
        let mut m = SpeedMeter::new();
        assert_eq!(m.bytes_per_sec(), None);
        m.record(ms(0), 0);
        assert_eq!(m.bytes_per_sec(), None);
        m.record(ms(100), 1000);
        assert!(close(m.bytes_per_sec().unwrap(), 10_000.0, 1e-9));
    }

    #[test]
    fn constant_rate_stays_constant() {
        let mut m = SpeedMeter::new();
        for i in 0..=100u64 {
            m.record(ms(i * 50), i * 500); // 10 kB/s
        }
        assert!(close(m.bytes_per_sec().unwrap(), 10_000.0, 1e-6));
    }

    #[test]
    fn step_response_follows_time_constant() {
        // 1 MB/s for a while, then 0 B/s: after one tau the value is e^-1 of the start.
        let mut m = SpeedMeter::new();
        let mut bytes = 0u64;
        for i in 0..=100u64 {
            bytes += 100_000;
            m.record(ms(i * 100), bytes);
        }
        let start = m.bytes_per_sec().unwrap();
        assert!(close(start, 1_000_000.0, 1.0));
        let t0 = 100 * 100;
        for i in 1..=30u64 {
            m.record(ms(t0 + i * 100), bytes); // 3 s of nothing
        }
        let after = m.bytes_per_sec().unwrap();
        let expected = start * (-1.0f64).exp();
        assert!(close(after, expected, expected * 1e-6), "{after} vs {expected}");
    }

    #[test]
    fn independent_of_sampling_rate() {
        let run = |step_ms: u64| {
            let mut m = SpeedMeter::new();
            // 1 MB/s for 5 s, then 3 MB/s for 3 s.
            let mut t = 0;
            while t <= 8000 {
                let bytes = if t <= 5000 { t * 1000 } else { 5_000_000 + (t - 5000) * 3000 };
                m.record(ms(t), bytes);
                t += step_ms;
            }
            m.bytes_per_sec().unwrap()
        };
        let fast = run(10);
        let slow = run(100);
        assert!(close(fast, slow, fast * 1e-3), "{fast} vs {slow}");
    }

    #[test]
    fn long_gap_nearly_replaces_average() {
        let mut m = SpeedMeter::new();
        m.record(ms(0), 0);
        m.record(ms(1000), 1_000_000); // 1 MB/s
        m.record(ms(61_000), 1_000_000 + 60 * 5_000_000); // 5 MB/s over a 60 s gap
        assert!(close(m.bytes_per_sec().unwrap(), 5_000_000.0, 5_000_000.0 * 1e-6));
    }

    #[test]
    fn non_advancing_time_is_ignored() {
        let mut m = SpeedMeter::new();
        m.record(ms(1000), 0);
        m.record(ms(1000), 500); // same instant: ignored
        assert_eq!(m.bytes_per_sec(), None);
        m.record(ms(500), 900); // earlier: ignored
        assert_eq!(m.bytes_per_sec(), None);
        m.record(ms(2000), 1000);
        assert!(close(m.bytes_per_sec().unwrap(), 1000.0, 1e-9));
    }

    #[test]
    fn counter_going_backwards_rebases() {
        let mut m = SpeedMeter::new();
        m.record(ms(0), 0);
        m.record(ms(1000), 1000);
        let before = m.bytes_per_sec().unwrap();
        m.record(ms(2000), 10); // restarted
        assert_eq!(m.bytes_per_sec().unwrap(), before);
        m.record(ms(3000), 1010);
        assert!(m.bytes_per_sec().unwrap().is_finite());
    }

    #[test]
    fn reset_clears_everything() {
        let mut m = SpeedMeter::new();
        m.record(ms(0), 0);
        m.record(ms(1000), 1000);
        assert!(m.bytes_per_sec().is_some());
        m.reset();
        assert_eq!(m.bytes_per_sec(), None);
        // Resumed counters start a fresh baseline.
        m.record(ms(90_000), 50_000);
        assert_eq!(m.bytes_per_sec(), None);
        m.record(ms(91_000), 52_000);
        assert!(close(m.bytes_per_sec().unwrap(), 2000.0, 1e-9));
    }

    #[test]
    fn eta_table() {
        let s = Duration::from_secs;
        let cases = [
            // (remaining, speed, elapsed, expected)
            (1000, Some(100.0), ms(1999), None),
            (1000, Some(100.0), s(2), Some(s(10))),
            (1000, Some(100.0), s(60), Some(s(10))),
            (1000, None, s(10), None),
            (1000, Some(0.0), s(10), None),
            (1000, Some(-5.0), s(10), None),
            (1000, Some(f64::NAN), s(10), None),
            (1000, Some(f64::INFINITY), s(10), None),
            (0, Some(100.0), s(10), Some(Duration::ZERO)),
            (0, Some(100.0), s(1), None),
            (1500, Some(1000.0), s(5), Some(ms(1500))),
            (u64::MAX, Some(1e-300), s(5), None), // overflows Duration
        ];
        for (rem, speed, el, want) in cases {
            assert_eq!(eta(rem, speed, el), want, "{rem} {speed:?} {el:?}");
        }
    }

    #[test]
    fn fraction_table() {
        let cases = [
            (0, 100, 0.0),
            (50, 100, 0.5),
            (100, 100, 1.0),
            (150, 100, 1.0), // clamped
            (0, 0, 0.0),
            (10, 0, 0.0), // unknown total
            (1, 3, 1.0 / 3.0),
            (u64::MAX, u64::MAX, 1.0),
        ];
        for (done, total, want) in cases {
            let got = Progress { done, total }.fraction();
            assert!((got - want as f32).abs() < 1e-6, "{done}/{total}: {got}");
        }
    }

    #[test]
    fn aggregate_sums() {
        let items =
            [Progress { done: 10, total: 100 }, Progress { done: 40, total: 50 }, Progress { done: 0, total: 0 }];
        assert_eq!(aggregate(&items), Progress { done: 50, total: 150 });
        assert_eq!(aggregate(&[]), Progress::default());
        assert_eq!(aggregate(items.iter().take(1)), items[0]);
    }

    #[test]
    fn aggregate_saturates() {
        let items = [Progress { done: u64::MAX, total: u64::MAX }, Progress { done: 5, total: 5 }];
        assert_eq!(aggregate(&items), Progress { done: u64::MAX, total: u64::MAX });
    }
}
