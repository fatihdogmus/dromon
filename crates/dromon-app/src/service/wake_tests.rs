//! The sleep detector and the probe gate on synthetic clocks.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::Instant;

use super::*;

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// Both clocks, moved by hand.
struct Clocks {
    mono: Instant,
    wall: SystemTime,
}

impl Clocks {
    fn new() -> Clocks {
        Clocks { mono: Instant::now(), wall: UNIX_EPOCH + secs(1_795_615_200) }
    }

    /// Awake for `d`: both clocks move.
    fn run(&mut self, d: Duration) -> (Instant, SystemTime) {
        self.mono += d;
        self.wall += d;
        (self.mono, self.wall)
    }

    /// Asleep for `d`: only the wall clock moves.
    fn sleep(&mut self, d: Duration) {
        self.wall += d;
    }
}

#[test]
fn ticks_on_time_are_no_sleep() {
    let mut c = Clocks::new();
    let mut d = SleepDetector::new(TICK);
    assert_eq!(d.tick(c.mono, c.wall), None, "the first tick sets the clocks");
    for _ in 0..100 {
        let (m, w) = c.run(TICK);
        assert_eq!(d.tick(m, w), None);
    }
    // A loaded machine's tick a few seconds late, the wall clock a few seconds ahead (NTP).
    let (m, w) = c.run(secs(9));
    assert_eq!(d.tick(m, w + secs(4)), None);
}

#[test]
fn a_wall_jump_past_the_monotonic_clock_is_a_sleep() {
    let mut c = Clocks::new();
    let mut d = SleepDetector::new(TICK);
    d.tick(c.mono, c.wall);
    // Asleep for an hour between two ticks (the monotonic clock stood still).
    c.run(secs(2));
    c.sleep(secs(3600));
    let (m, w) = c.run(secs(3));
    assert_eq!(d.tick(m, w), Some(secs(3600)));
    // The next tick is an ordinary one again.
    let (m, w) = c.run(TICK);
    assert_eq!(d.tick(m, w), None);
    // Ten seconds is the threshold: just under isn't a sleep, just over is.
    c.sleep(secs(10));
    let (m, w) = c.run(TICK);
    assert_eq!(d.tick(m, w), None);
    c.sleep(secs(11));
    let (m, w) = c.run(TICK);
    assert_eq!(d.tick(m, w), Some(secs(11)));
}

#[test]
fn a_monotonic_clock_that_counts_sleep_shows_it_as_a_late_tick() {
    let mut c = Clocks::new();
    let mut d = SleepDetector::new(TICK);
    d.tick(c.mono, c.wall);
    // Both clocks went on through 20 minutes asleep: the tick came that late.
    let (m, w) = c.run(TICK + secs(1200));
    assert_eq!(d.tick(m, w), Some(secs(1200)));
}

#[test]
fn a_wall_clock_set_back_is_no_sleep() {
    let mut c = Clocks::new();
    let mut d = SleepDetector::new(TICK);
    d.tick(c.mono, c.wall);
    let (m, w) = c.run(TICK);
    assert_eq!(d.tick(m, w - secs(7200)), None, "set back two hours");
    let (m, w) = c.run(TICK);
    assert_eq!(d.tick(m, w - secs(7200)), None);
}

#[test]
fn a_wake_always_probes_but_never_twice_at_once() {
    let t = Instant::now();
    let mut g = ProbeGate::default();
    assert!(g.start(Why::Wake, 1, t));
    assert!(g.running(1));
    assert!(!g.start(Why::Wake, 1, t + secs(1)), "one runs");
    assert!(!g.start(Why::Focus, 1, t + secs(1)));
    g.ended(1);
    assert!(!g.running(1));
    assert!(g.start(Why::Wake, 1, t + secs(2)), "a wake isn't rate-limited");
    g.ended(1);
}

#[test]
fn focus_probes_at_most_every_thirty_seconds() {
    let t = Instant::now();
    let mut g = ProbeGate::default();
    assert!(g.start(Why::Focus, 1, t), "the first focus probes");
    g.ended(1);
    assert!(!g.start(Why::Focus, 1, t + secs(5)));
    assert!(!g.start(Why::Focus, 1, t + FOCUS_EVERY - Duration::from_millis(1)));
    assert!(g.start(Why::Focus, 1, t + FOCUS_EVERY));
    g.ended(1);
    // A wake probe counts too: focus right after it doesn't probe again.
    assert!(g.start(Why::Wake, 1, t + secs(100)));
    g.ended(1);
    assert!(!g.start(Why::Focus, 1, t + secs(110)));
}

#[test]
fn a_probe_of_a_session_that_went_away_doesnt_block_the_next_one() {
    let t = Instant::now();
    let mut g = ProbeGate::default();
    assert!(g.start(Why::Wake, 1, t));
    // Session 1 ended and 2 came up before the probe gave up.
    assert!(g.start(Why::Wake, 2, t + secs(1)));
    g.ended(1);
    assert!(g.running(2), "the old probe's end leaves the new one running");
    g.ended(2);
    assert!(!g.running(2));
}
