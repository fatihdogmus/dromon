//! One host's connection lifecycle as an explicit state machine (§4.3 "Reconnect", §3.9):
//! a pure [`step`] from a [`Phase`] and an [`Event`] to the next phase and the [`Effect`]s the
//! connection task carries out. No I/O and no clocks here, so every transition is unit-tested.
//!
//! ```text
//!            Connect                     Up
//!   Idle ───────────▶ Starting{0} ─────────────▶ Ready ◀──────────────┐
//!   Failed            │  ▲ prompts>0 = NeedsInput │                    │ Up
//!   Disconnected      │  │                        │ Exited / Lost      │
//!                     │  └ PromptOpened/Closed    ▼                    │
//!                     │                   Waiting{n} ──RetryDue──▶ Starting{n}
//!                     │                        ▲                       │
//!                     │                        └──── Exited (other) ───┘
//!                     └─ Exited: auth or host key ─▶ Failed      (any) ─Disconnect─▶ Disconnected
//! ```
//!
//! - The first attempt (`Starting{0}`) fails outright on any exit: the user just clicked, and
//!   "Connection refused" is more use than a silent retry loop.
//! - Reconnects (`Starting{n ≥ 1}`) retry on the [`Backoff`] schedule unless ssh refused for
//!   authentication or the host key, which no retry fixes.
//! - Reconnects answer remembered passwords silently (the task's `AuthAttempt` does that); a
//!   prompt that still needs the user shows the sheet and the status is `NeedsInput`.

use std::time::Duration;

use dromon_core::ConnStatus;
use dromon_ssh::Backoff;

/// Where the connection is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Never connected.
    Idle,
    /// ssh runs and SFTP isn't up yet. `attempt` 0 is the first connection, `n ≥ 1` the nth
    /// reconnect; `prompts` counts the open prompts waiting for the user.
    Starting { attempt: u32, prompts: u32 },
    Ready,
    /// The connection dropped; reconnect `attempt` starts when its backoff is over.
    Waiting { attempt: u32 },
    Failed(String),
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Connect,
    /// ssh asked something the user has to answer (not a notice).
    PromptOpened,
    /// Such a prompt was answered, cancelled, or went away.
    PromptClosed,
    /// SFTP is up and the start folder resolved.
    Up,
    /// ssh exited. `stderr` is what it printed last (for the reason), `error` our own note
    /// (SFTP didn't start, say).
    Exited { stderr: Vec<String>, error: Option<String> },
    /// An SFTP request found the connection gone while ssh still runs.
    Lost,
    /// The backoff before reconnect `attempt` is over.
    RetryDue { attempt: u32 },
    Disconnect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Start a fresh attempt: a new ssh, askpass token and `AuthAttempt`, then SFTP.
    Spawn,
    /// Stop the running ssh, if any.
    Kill,
    Status(ConnStatus),
    /// Up for the first time: tell the UI its start folder.
    Opened,
    /// Up again after a reconnect: refresh the folder on screen.
    Reopened,
    /// Wake with `RetryDue { attempt }` after `after`.
    Retry { attempt: u32, after: Duration },
    /// ssh gave up on authentication: forget the password it was given (`on_auth_failed`).
    AuthFailed,
    /// Disconnect: forget the host's remembered passwords and passphrases.
    ForgetSecrets,
    /// A line for the connection log.
    Log(String),
}

/// The machine's whole state: the phase and the reconnect schedule.
#[derive(Clone, Debug)]
pub struct Lifecycle {
    pub phase: Phase,
    backoff: Backoff,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Lifecycle { phase: Phase::Idle, backoff: Backoff::new() }
    }
}

/// Why ssh exited before SFTP started, read from its stderr.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// "Permission denied (publickey,password)."
    Auth(String),
    /// "Host key verification failed." (cancelled, or a changed key that ssh refused).
    HostKey(String),
    Other(String),
}

impl ExitReason {
    pub fn message(&self) -> &str {
        match self {
            ExitReason::Auth(m) | ExitReason::HostKey(m) | ExitReason::Other(m) => m,
        }
    }

    /// Retrying can't help: the server or ssh itself refused us.
    pub fn is_refusal(&self) -> bool {
        matches!(self, ExitReason::Auth(_) | ExitReason::HostKey(_))
    }
}

/// Reads ssh's last stderr lines for why it exited. A changed host key names the
/// `known_hosts` file and line to inspect (§3.4).
pub fn exit_reason(stderr: &[String], error: Option<&str>) -> ExitReason {
    let lines: Vec<&str> = stderr.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let has = |needle: &str| lines.iter().any(|l| l.to_ascii_lowercase().contains(&needle.to_ascii_lowercase()));
    if has("Host key verification failed") {
        if has("REMOTE HOST IDENTIFICATION HAS CHANGED") || has("host key for") && has("has changed") {
            let offending = lines
                .iter()
                .find_map(|l| l.split_once(" key in ").filter(|_| l.starts_with("Offending")).map(|(_, at)| at.trim().to_owned()));
            let mut msg = "The server's host key has changed since you last connected, so ssh refused to connect. \
                           If the server was reinstalled or its keys rotated, remove the old key"
                .to_owned();
            match offending {
                Some(at) => msg.push_str(&format!(" at {at}.")),
                None => msg.push_str(" from your known_hosts file."),
            }
            return ExitReason::HostKey(msg);
        }
        return ExitReason::HostKey("Host key verification failed.".into());
    }
    if let Some(line) = lines.iter().rev().find(|l| l.contains("Permission denied")) {
        // "deploy@prod: Permission denied (publickey,password)." → from "Permission denied".
        let at = line.find("Permission denied").unwrap_or(0);
        return ExitReason::Auth(line[at..].to_owned());
    }
    let last = lines.iter().rev().find(|l| !l.starts_with("debug") && !l.starts_with("Warning: Permanently added"));
    match (last, error) {
        (Some(line), _) => ExitReason::Other(line.strip_prefix("ssh: ").unwrap_or(line).to_owned()),
        (None, Some(e)) => ExitReason::Other(e.to_owned()),
        (None, None) => ExitReason::Other("ssh exited without saying why.".into()),
    }
}

fn starting_status(attempt: u32) -> ConnStatus {
    if attempt == 0 { ConnStatus::Connecting } else { ConnStatus::Reconnecting { attempt } }
}

/// The next phase for `event`, and what to do about it.
pub fn step(m: &mut Lifecycle, event: Event) -> Vec<Effect> {
    use Effect as E;
    match (&mut m.phase, event) {
        (_, Event::Disconnect) => {
            m.phase = Phase::Disconnected;
            m.backoff.reset();
            vec![E::Kill, E::ForgetSecrets, E::Status(ConnStatus::Disconnected), E::Log("Disconnected.".into())]
        }
        (Phase::Idle | Phase::Failed(_) | Phase::Disconnected, Event::Connect) => {
            m.phase = Phase::Starting { attempt: 0, prompts: 0 };
            m.backoff.reset();
            vec![E::Status(ConnStatus::Connecting), E::Spawn]
        }
        // Already live: nothing to do.
        (_, Event::Connect) => Vec::new(),

        (Phase::Starting { prompts, .. }, Event::PromptOpened) => {
            *prompts += 1;
            if *prompts == 1 { vec![E::Status(ConnStatus::NeedsInput)] } else { Vec::new() }
        }
        (Phase::Starting { attempt, prompts }, Event::PromptClosed) => {
            if *prompts == 0 {
                return Vec::new();
            }
            *prompts -= 1;
            if *prompts == 0 { vec![E::Status(starting_status(*attempt))] } else { Vec::new() }
        }
        (_, Event::PromptOpened | Event::PromptClosed) => Vec::new(),

        (Phase::Starting { attempt, .. }, Event::Up) => {
            let first = *attempt == 0;
            m.phase = Phase::Ready;
            m.backoff.reset();
            let note = if first { "Connected." } else { "Reconnected." };
            vec![E::Status(ConnStatus::Ready), if first { E::Opened } else { E::Reopened }, E::Log(note.into())]
        }
        (_, Event::Up) => Vec::new(),

        (Phase::Starting { attempt, .. }, Event::Exited { stderr, error }) => {
            let attempt = *attempt;
            let reason = exit_reason(&stderr, error.as_deref());
            if attempt == 0 || reason.is_refusal() {
                let msg = reason.message().to_owned();
                m.phase = Phase::Failed(msg.clone());
                let mut fx = Vec::new();
                if matches!(reason, ExitReason::Auth(_)) {
                    fx.push(E::AuthFailed);
                }
                fx.push(E::Status(ConnStatus::Failed(msg.clone())));
                fx.push(E::Log(format!("Couldn't connect: {msg}")));
                return fx;
            }
            let mut fx = vec![E::Log(format!("Reconnect {attempt} failed: {}", reason.message()))];
            fx.extend(schedule(m));
            fx
        }
        (Phase::Ready, Event::Exited { stderr, error }) => {
            let reason = exit_reason(&stderr, error.as_deref());
            let mut fx = vec![E::Log(format!("Connection lost: {}", reason.message()))];
            fx.extend(schedule(m));
            fx
        }
        (Phase::Ready, Event::Lost) => {
            let mut fx = vec![E::Kill, E::Log("Connection lost: the SFTP channel stopped answering.".into())];
            fx.extend(schedule(m));
            fx
        }
        // An exit or loss we already acted on, or with nothing running.
        (_, Event::Exited { .. } | Event::Lost) => Vec::new(),

        (Phase::Waiting { attempt }, Event::RetryDue { attempt: due }) if *attempt == due => {
            let attempt = *attempt;
            m.phase = Phase::Starting { attempt, prompts: 0 };
            vec![E::Log(format!("Reconnecting (attempt {attempt})…")), E::Spawn]
        }
        // A timer for an attempt that's no longer waiting.
        (_, Event::RetryDue { .. }) => Vec::new(),
    }
}

/// Waits out the next backoff step before reconnecting.
fn schedule(m: &mut Lifecycle) -> Vec<Effect> {
    let after = m.backoff.next_delay();
    let attempt = m.backoff.attempt();
    m.phase = Phase::Waiting { attempt };
    vec![Effect::Status(ConnStatus::Reconnecting { attempt }), Effect::Retry { attempt, after }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn exited(lines: &[&str]) -> Event {
        Event::Exited { stderr: lines.iter().map(|l| (*l).to_owned()).collect(), error: None }
    }

    /// The effects without the log lines, which tests check separately.
    fn quiet(fx: Vec<Effect>) -> Vec<Effect> {
        fx.into_iter().filter(|e| !matches!(e, Effect::Log(_))).collect()
    }

    fn up() -> Lifecycle {
        let mut m = Lifecycle::default();
        step(&mut m, Event::Connect);
        step(&mut m, Event::Up);
        m
    }

    #[test]
    fn first_connect_with_a_password_prompt() {
        let mut m = Lifecycle::default();
        assert_eq!(step(&mut m, Event::Connect), [Effect::Status(ConnStatus::Connecting), Effect::Spawn]);
        assert_eq!(step(&mut m, Event::PromptOpened), [Effect::Status(ConnStatus::NeedsInput)]);
        assert_eq!(step(&mut m, Event::PromptClosed), [Effect::Status(ConnStatus::Connecting)]);
        assert_eq!(quiet(step(&mut m, Event::Up)), [Effect::Status(ConnStatus::Ready), Effect::Opened]);
        assert_eq!(m.phase, Phase::Ready);
        assert!(step(&mut m, Event::Connect).is_empty(), "connecting again while up does nothing");
    }

    #[test]
    fn two_prompts_need_input_until_both_close() {
        let mut m = Lifecycle::default();
        step(&mut m, Event::Connect);
        step(&mut m, Event::PromptOpened);
        assert!(step(&mut m, Event::PromptOpened).is_empty());
        assert!(step(&mut m, Event::PromptClosed).is_empty());
        assert_eq!(step(&mut m, Event::PromptClosed), [Effect::Status(ConnStatus::Connecting)]);
        assert!(step(&mut m, Event::PromptClosed).is_empty(), "never below zero");
    }

    #[test]
    fn a_dropped_connection_reconnects_on_the_backoff_schedule() {
        let mut m = up();
        assert_eq!(
            quiet(step(&mut m, exited(&["Connection to prod closed by remote host."]))),
            [Effect::Status(ConnStatus::Reconnecting { attempt: 1 }), Effect::Retry { attempt: 1, after: secs(1) }]
        );
        assert!(step(&mut m, Event::RetryDue { attempt: 7 }).is_empty(), "a stale timer");
        assert_eq!(quiet(step(&mut m, Event::RetryDue { attempt: 1 })), [Effect::Spawn]);
        // Still down: the next tries wait longer.
        assert_eq!(
            quiet(step(&mut m, exited(&["ssh: connect to host prod port 22: Network is unreachable"]))),
            [Effect::Status(ConnStatus::Reconnecting { attempt: 2 }), Effect::Retry { attempt: 2, after: secs(2) }]
        );
        step(&mut m, Event::RetryDue { attempt: 2 });
        let fx = quiet(step(&mut m, exited(&[])));
        assert_eq!(fx[1], Effect::Retry { attempt: 3, after: secs(5) });
        step(&mut m, Event::RetryDue { attempt: 3 });
        // A prompt during a reconnect needs input, and closing it goes back to reconnecting.
        assert_eq!(step(&mut m, Event::PromptOpened), [Effect::Status(ConnStatus::NeedsInput)]);
        assert_eq!(step(&mut m, Event::PromptClosed), [Effect::Status(ConnStatus::Reconnecting { attempt: 3 })]);
        assert_eq!(quiet(step(&mut m, Event::Up)), [Effect::Status(ConnStatus::Ready), Effect::Reopened]);
        // The schedule starts over once it's back.
        let fx = quiet(step(&mut m, Event::Lost));
        assert_eq!(fx, [Effect::Kill, Effect::Status(ConnStatus::Reconnecting { attempt: 1 }), Effect::Retry { attempt: 1, after: secs(1) }]);
        assert!(step(&mut m, exited(&[])).is_empty(), "the exit of the ssh we killed");
    }

    #[test]
    fn auth_failure_fails_without_retrying() {
        let mut m = Lifecycle::default();
        step(&mut m, Event::Connect);
        let fx = quiet(step(&mut m, exited(&["deploy@prod: Permission denied (publickey,password)."])));
        let msg = "Permission denied (publickey,password).".to_owned();
        assert_eq!(fx, [Effect::AuthFailed, Effect::Status(ConnStatus::Failed(msg.clone()))]);
        assert_eq!(m.phase, Phase::Failed(msg));
        // During a reconnect too.
        let mut m = up();
        step(&mut m, exited(&[]));
        step(&mut m, Event::RetryDue { attempt: 1 });
        let fx = quiet(step(&mut m, exited(&["x@y: Permission denied (password)."])));
        assert_eq!(fx[0], Effect::AuthFailed);
        assert!(matches!(m.phase, Phase::Failed(_)));
        // Connecting again from there starts over.
        assert_eq!(step(&mut m, Event::Connect), [Effect::Status(ConnStatus::Connecting), Effect::Spawn]);
    }

    #[test]
    fn a_first_attempt_fails_on_any_exit() {
        let mut m = Lifecycle::default();
        step(&mut m, Event::Connect);
        let fx = quiet(step(&mut m, exited(&["ssh: Could not resolve hostname nope: Name or service not known"])));
        assert_eq!(fx, [Effect::Status(ConnStatus::Failed("Could not resolve hostname nope: Name or service not known".into()))]);
    }

    #[test]
    fn disconnect_from_anywhere_forgets_secrets() {
        for setup in [Event::Connect, Event::Up] {
            let mut m = Lifecycle::default();
            step(&mut m, Event::Connect);
            step(&mut m, setup);
            assert_eq!(
                quiet(step(&mut m, Event::Disconnect)),
                [Effect::Kill, Effect::ForgetSecrets, Effect::Status(ConnStatus::Disconnected)]
            );
            assert_eq!(m.phase, Phase::Disconnected);
            assert!(step(&mut m, exited(&[])).is_empty());
            assert!(step(&mut m, Event::RetryDue { attempt: 1 }).is_empty());
        }
        let mut m = up();
        step(&mut m, Event::Lost);
        step(&mut m, Event::Disconnect);
        assert!(step(&mut m, Event::RetryDue { attempt: 1 }).is_empty(), "no reconnect after a disconnect");
    }

    #[test]
    fn exit_reasons() {
        let s = |lines: &[&str]| lines.iter().map(|l| (*l).to_owned()).collect::<Vec<_>>();
        let changed = s(&[
            "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@",
            "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @",
            "IT IS POSSIBLE THAT SOMEONE IS DOING SOMETHING NASTY!",
            "Offending ECDSA key in /home/u/.ssh/known_hosts:3",
            "Host key for prod has changed and you have requested strict checking.",
            "Host key verification failed.",
        ]);
        let r = exit_reason(&changed, None);
        assert!(matches!(&r, ExitReason::HostKey(m) if m.contains("/home/u/.ssh/known_hosts:3") && m.contains("changed")), "{r:?}");
        assert!(r.is_refusal());
        assert_eq!(exit_reason(&s(&["Host key verification failed."]), None), ExitReason::HostKey("Host key verification failed.".into()));
        assert_eq!(
            exit_reason(&s(&["debug1: x", "Connection closed by 10.0.0.4 port 22", "debug1: y"]), None),
            ExitReason::Other("Connection closed by 10.0.0.4 port 22".into())
        );
        assert_eq!(exit_reason(&[], Some("SFTP didn't start")), ExitReason::Other("SFTP didn't start".into()));
        assert!(!exit_reason(&[], None).is_refusal());
    }
}
