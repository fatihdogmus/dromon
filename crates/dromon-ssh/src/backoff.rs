//! The reconnect schedule: 1 s, 2 s, 5 s, 10 s, then every 30 s.

use std::time::Duration;

const STEPS: [Duration; 4] =
    [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(5), Duration::from_secs(10)];
const STEADY: Duration = Duration::from_secs(30);

/// Delays between reconnect attempts. Call [`Backoff::reset`] once a connection is up again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// The delay before the next attempt, counting that attempt.
    pub fn next_delay(&mut self) -> Duration {
        let delay = STEPS.get(self.attempt as usize).copied().unwrap_or(STEADY);
        self.attempt = self.attempt.saturating_add(1);
        delay
    }

    /// How many delays have been handed out since the last reset: the number of the attempt
    /// now waiting or running, from 1 (matches `ConnStatus::Reconnecting { attempt }`), or 0.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Starts over at 1 s.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(b: &mut Backoff, n: usize) -> Vec<u64> {
        (0..n).map(|_| b.next_delay().as_secs()).collect()
    }

    #[test]
    fn schedule() {
        let mut b = Backoff::new();
        assert_eq!(b.attempt(), 0);
        assert_eq!(secs(&mut b, 8), [1, 2, 5, 10, 30, 30, 30, 30]);
        assert_eq!(b.attempt(), 8);
    }

    #[test]
    fn reset_starts_over() {
        let mut b = Backoff::new();
        secs(&mut b, 6);
        b.reset();
        assert_eq!(b.attempt(), 0);
        assert_eq!(secs(&mut b, 3), [1, 2, 5]);
        assert_eq!(b.attempt(), 3);
    }

    #[test]
    fn stays_at_30s_without_overflow() {
        let mut b = Backoff { attempt: u32::MAX - 1 };
        assert_eq!(secs(&mut b, 3), [30, 30, 30]);
        assert_eq!(b.attempt(), u32::MAX);
    }
}
