//! A connection task's checks (`wake.rs`, spike S6) and its verbose logging (§6.8).
//!
//! - A probe runs as its own task on the live SFTP session, beside any listing, so it blocks
//!   nothing; one at a time (`ProbeGate`). No answer within the timeout ends the session as
//!   lost, and the machine's silent reconnect takes over (§3.9).
//! - Verbose logging changes the next ssh's command line; a live connection reconnects at once
//!   to take it (`Event::Restart`).

use std::future::pending;

use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::super::Sink;
use super::super::lifecycle::{Event, Phase};
use super::super::wake::{self, PROBE_TIMEOUT, Why};
use super::Task;
use crate::state::ServiceEvent;
use crate::state::service::tracing;

/// A probe under way on session `epoch`.
pub(super) struct Probe {
    epoch: u64,
    why: Why,
    run: JoinHandle<bool>,
}

/// A probe that ended: on which session, why it ran, whether the session answered.
pub(super) struct Probed {
    epoch: u64,
    why: Why,
    alive: bool,
}

/// The probe's end, once it comes; the slot is empty again then.
pub(super) async fn next(probe: &mut Option<Probe>) -> Probed {
    let Some(p) = probe else { return pending().await };
    // A probe task that panicked or was aborted got no answer.
    let alive = (&mut p.run).await.unwrap_or(false);
    let (epoch, why) = (p.epoch, p.why);
    *probe = None;
    Probed { epoch, why, alive }
}

impl<S: Sink> Task<S> {
    /// Checks the live session for `why`, unless the gate says not now. Only a connection
    /// that's up has anything to check.
    pub(super) fn probe(&mut self, why: Why) {
        if self.life.phase != Phase::Ready {
            return;
        }
        let Some(sftp) = self.sftp() else { return };
        if !self.probes.start(why, self.epoch, Instant::now()) {
            return;
        }
        if tracing() {
            eprintln!("dromon-trace: Probe host={} why={}", self.host, why.name());
        }
        if let Some(old) = self.probe.take() {
            old.run.abort();
        }
        let run = tokio::spawn(wake::answers(sftp, PROBE_TIMEOUT));
        self.probe = Some(Probe { epoch: self.epoch, why, run });
    }

    /// A probe ended. A session that didn't answer is lost, if it's still the one up.
    pub(super) async fn on_probed(&mut self, probed: Probed) {
        let Probed { epoch, why, alive } = probed;
        self.probes.ended(epoch);
        let current = epoch == self.epoch && self.sftp().is_some() && self.life.phase == Phase::Ready;
        if !current {
            return;
        }
        if alive {
            if why == Why::Wake {
                self.log("The connection still answers.");
            }
            return;
        }
        if tracing() {
            eprintln!("dromon-trace: ProbeFailed host={} why={}", self.host, why.name());
        }
        self.log(format!("The connection didn't answer a check within {} s.", PROBE_TIMEOUT.as_secs()));
        self.drive(Event::Lost, None).await;
    }

    /// Verbose logging on or off: every window hears it, and a live connection reconnects to
    /// take it.
    pub(super) async fn set_verbose(&mut self, on: bool) {
        let changed = self.verbose != on;
        self.verbose = on;
        self.emit(ServiceEvent::Verbose { host: self.host.clone(), on });
        if !changed {
            return;
        }
        self.log(if on { "Verbose logging on: ssh runs with -v." } else { "Verbose logging off." });
        self.drive(Event::Restart, None).await;
    }
}
