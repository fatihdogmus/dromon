//! Waking from sleep, and checking that a connection still answers (§4.3 "Backoff", §3.9;
//! spike S6). Rune tells the app about neither system wake nor network changes, so the
//! service finds out itself:
//!
//! - [`SleepDetector`]: the manager ticks every [`TICK`] and compares how far the monotonic
//!   clock moved with how far the wall clock did. Asleep, the monotonic clock stands still
//!   (Linux `CLOCK_MONOTONIC`, macOS `CLOCK_UPTIME_RAW`) while the wall clock goes on, so a wall
//!   jump more than [`ASLEEP`] beyond it is a wake. Where the monotonic clock counts sleep too,
//!   the tick itself comes more than [`ASLEEP`] late, which counts the same.
//! - [`ProbeGate`]: when a host's live session gets a probe. A wake always probes; a window
//!   gaining focus probes at most once per [`FOCUS_EVERY`]; never two at once.
//! - [`answers`]: the probe itself, `realpath(".")` on the session within [`PROBE_TIMEOUT`].
//!   No answer ends the session as lost, and the silent reconnect takes over.
//!
//! Pure but for [`watch`] and [`answers`]: the times come in as arguments.

use std::time::{Duration, SystemTime};

use dromon_core::Error;
use dromon_sftp::Sftp;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};

#[cfg(test)]
#[path = "wake_tests.rs"]
mod tests;

/// How often the manager looks at the clocks.
pub const TICK: Duration = Duration::from_secs(5);
/// A gap in the monotonic clock longer than this is a sleep.
pub const ASLEEP: Duration = Duration::from_secs(10);
/// How long a probe waits for its answer.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// The least time between two focus probes of a host.
pub const FOCUS_EVERY: Duration = Duration::from_secs(30);

/// Why a probe is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The computer woke from sleep.
    Wake,
    /// A window gained focus.
    Focus,
}

impl Why {
    pub fn name(self) -> &'static str {
        match self {
            Why::Wake => "wake",
            Why::Focus => "focus",
        }
    }
}

/// Tells a sleep from the clocks at each tick.
#[derive(Debug)]
pub struct SleepDetector {
    /// How far apart the ticks are meant to be.
    period: Duration,
    /// The clocks at the last tick.
    last: Option<(Instant, SystemTime)>,
}

impl SleepDetector {
    pub fn new(period: Duration) -> SleepDetector {
        SleepDetector { period, last: None }
    }

    /// A tick at `mono` on the monotonic clock and `wall` on the wall clock: how long the
    /// computer slept since the last one, if it did. The first tick only sets the clocks; a wall
    /// clock set back is never a sleep.
    pub fn tick(&mut self, mono: Instant, wall: SystemTime) -> Option<Duration> {
        let (mono0, wall0) = self.last.replace((mono, wall))?;
        let ran = mono.saturating_duration_since(mono0);
        let passed = wall.duration_since(wall0).unwrap_or(Duration::ZERO);
        // Asleep while the monotonic clock stood still, or (one that counts sleep) a late tick.
        let unseen = passed.saturating_sub(ran);
        let late = ran.saturating_sub(self.period);
        let slept = unseen.max(late);
        (slept > ASLEEP).then_some(slept)
    }
}

/// When one host's session is probed.
#[derive(Debug, Default)]
pub struct ProbeGate {
    /// When the last probe started.
    last: Option<Instant>,
    /// The session (its epoch) a probe runs on now.
    running: Option<u64>,
}

impl ProbeGate {
    /// Whether to start a probe of session `epoch` at `now`, for `why`; if so it's running.
    /// Not while one runs on that session; for focus, not within [`FOCUS_EVERY`] of the last.
    /// A probe of an older session (gone meanwhile) doesn't count as running.
    pub fn start(&mut self, why: Why, epoch: u64, now: Instant) -> bool {
        if self.running == Some(epoch) {
            return false;
        }
        if why == Why::Focus && self.last.is_some_and(|at| now.saturating_duration_since(at) < FOCUS_EVERY) {
            return false;
        }
        self.running = Some(epoch);
        self.last = Some(now);
        true
    }

    /// The probe of session `epoch` ended.
    pub fn ended(&mut self, epoch: u64) {
        if self.running == Some(epoch) {
            self.running = None;
        }
    }

    /// Whether a probe of session `epoch` runs.
    #[cfg(test)]
    pub fn running(&self, epoch: u64) -> bool {
        self.running == Some(epoch)
    }
}

/// Whether the session answers a `realpath(".")` within `within`. Any answer, a refusal
/// included, shows it's alive; only a lost connection or silence don't.
pub async fn answers(sftp: Sftp, within: Duration) -> bool {
    match tokio::time::timeout(within, sftp.home()).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => e != Error::ConnectionLost,
        Err(_) => false,
    }
}

/// Ticks every [`TICK`] until `out` closes, sending how long the computer slept at each wake.
pub async fn watch(out: mpsc::UnboundedSender<Duration>) {
    let mut every = tokio::time::interval(TICK);
    every.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut detector = SleepDetector::new(TICK);
    loop {
        tokio::select! {
            _ = every.tick() => {}
            _ = out.closed() => return,
        }
        if let Some(slept) = detector.tick(Instant::now(), SystemTime::now())
            && out.send(slept).is_err()
        {
            return;
        }
    }
}
