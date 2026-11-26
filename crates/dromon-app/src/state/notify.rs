//! What the OS shows of transfers outside the window (§2, §5 M2): the Dock badge (macOS), the
//! taskbar button's bar and badge (Windows), and a system notification when a job that ran
//! more than 10 s ends while the window is in the background. Linux shows no badge (Rune draws
//! none there); its notifications go to the desktop's notification server.
//!
//! Pure: [`Watch`] sees each snapshot of the jobs (`Transfers::set`) and each change of window
//! focus, works out what to show, and hands it to its [`Os`] as [`OsEffect`]s: the taskbar only
//! when what it shows changes, a notification once per job that ends. The window's sink
//! (`views::transfers::os_sink`) carries them out; without one (tests, scenes, headless runs)
//! they stay in [`Os::held`] for a test to read, and nothing reaches the OS.
//!
//! Notification text never holds more than the popover shows: the job's title (an item's name
//! or "3 items"), the host's alias and the engine's reason for a failure.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use dromon_transfer::{JobId, JobKind, JobView, TaskState};

use crate::state::system_ms;

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;

/// A job has to have run longer than this (from when the app first saw it until it ended) for
/// its end to be worth a notification, in milliseconds.
pub const NOTIFY_AFTER_MS: i64 = 10_000;

/// The most effects [`Os::held`] keeps without a sink (the newest).
const HELD: usize = 64;

/// The bar under the taskbar button (Windows; the Dock has none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bar {
    /// Nothing runs.
    #[default]
    None,
    /// Something runs. Rune's taskbar has no fraction yet, so Windows shows its indeterminate
    /// bar rather than the active jobs' bytes.
    Busy,
    /// Everything left waits: paused, waiting for its connection, or for the user to settle a
    /// name clash (which alone is enough).
    Paused,
    /// A job failed while the window was in the background, and the user hasn't looked since.
    Error,
}

/// The taskbar button's (and the Dock icon's) state: how many jobs are active, and the bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dock {
    pub count: u32,
    pub bar: Bar,
}

impl Dock {
    /// What a screen reader says for the badge.
    pub fn description(&self) -> String {
        match self.count {
            0 => String::new(),
            1 => "1 transfer in progress".to_owned(),
            n => format!("{n} transfers in progress"),
        }
    }
}

/// The taskbar's state for these jobs; `failed_unseen` is whether a job failed out of sight.
pub fn dock(jobs: &[JobView], failed_unseen: bool) -> Dock {
    let active: Vec<&JobView> = jobs.iter().filter(|j| j.state.is_active()).collect();
    let count = u32::try_from(active.len()).unwrap_or(u32::MAX);
    let waits = |j: &&JobView| matches!(j.state, TaskState::Paused) || j.waiting_for_connection;
    let bar = if failed_unseen {
        Bar::Error
    } else if active.is_empty() {
        Bar::None
    } else if active.iter().any(|j| j.conflict.is_some()) || active.iter().all(waits) {
        Bar::Paused
    } else {
        Bar::Busy
    };
    Dock { count, bar }
}

/// A system notification about one job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The notification's id: a later one about the same job replaces it.
    pub id: String,
    /// Clicking it shows this job in the transfers popover.
    pub job: JobId,
    /// "Downloaded report.pdf", "Uploaded 3 items to prod", "Couldn't download syslog".
    pub title: String,
    /// Why it failed, or what was left out; may be empty.
    pub body: String,
}

/// The notification for a job that ended: done or failed (a cancel was the user's own doing,
/// so it says nothing). The title names what happened, the body why a job failed.
pub fn notice(j: &JobView) -> Option<Notice> {
    let (what, host) = (&j.title, j.host.as_str());
    let (title, body) = match &j.state {
        TaskState::Done => {
            let title = match j.kind {
                JobKind::Download => format!("Downloaded {what}"),
                JobKind::Upload => format!("Uploaded {what} to {host}"),
                JobKind::RemoteCopy => format!("Copied {what} on {host}"),
                JobKind::Move => format!("Moved {what} on {host}"),
                JobKind::CrossHostCopy => format!("Copied {what} to {host}"),
                JobKind::CrossHostMove => format!("Moved {what} to {host}"),
            };
            let body = match j.skipped.len() {
                0 => String::new(),
                1 => "1 item skipped".to_owned(),
                n => format!("{} items skipped", crate::fmt::thousands(n as u64)),
            };
            (title, body)
        }
        TaskState::Failed { error } => {
            let title = match j.kind {
                JobKind::Download => format!("Couldn't download {what}"),
                JobKind::Upload => format!("Couldn't upload {what} to {host}"),
                JobKind::RemoteCopy => format!("Couldn't copy {what} on {host}"),
                JobKind::Move => format!("Couldn't move {what} on {host}"),
                JobKind::CrossHostCopy => format!("Couldn't copy {what} to {host}"),
                JobKind::CrossHostMove => format!("Couldn't move {what} to {host}"),
            };
            (title, error.clone())
        }
        _ => return None,
    };
    Some(Notice { id: format!("transfer-{}", j.id.0), job: j.id, title, body })
}

impl Notice {
    /// The notice as one line: "Couldn't download syslog: Permission denied".
    pub fn line(&self) -> String {
        if self.body.is_empty() { self.title.clone() } else { format!("{}: {}", self.title, self.body) }
    }
}

/// Whether a job's end is worth a notification: it ran longer than [`NOTIFY_AFTER_MS`] and the
/// window isn't focused (the user isn't looking at the app).
pub fn worth_telling(ran_ms: i64, focused: bool) -> bool {
    !focused && ran_ms > NOTIFY_AFTER_MS
}

/// Something for the OS to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OsEffect {
    /// Show this on the taskbar button and the Dock icon.
    Taskbar(Dock),
    /// Post this notification.
    Notify(Notice),
}

/// Where [`OsEffect`]s go: the window's sink once one is connected, else [`Os::held`].
#[derive(Clone, Default)]
pub struct Os {
    sink: Option<Rc<dyn Fn(OsEffect)>>,
    /// What came without a sink to take it, oldest first (at most the newest 64).
    pub held: Vec<OsEffect>,
}

impl std::fmt::Debug for Os {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Os").field("sink", &self.sink.is_some()).field("held", &self.held).finish()
    }
}

impl Os {
    /// From now on effects go to `sink` (on the UI thread).
    pub fn connect(&mut self, sink: impl Fn(OsEffect) + 'static) {
        self.sink = Some(Rc::new(sink));
    }

    fn emit(&mut self, fx: OsEffect) {
        match &self.sink {
            Some(sink) => sink(fx),
            None => {
                if self.held.len() == HELD {
                    self.held.remove(0);
                }
                self.held.push(fx);
            }
        }
    }

    /// What was held, emptied.
    pub fn take(&mut self) -> Vec<OsEffect> {
        std::mem::take(&mut self.held)
    }
}

/// The jobs as the OS sees them: when each started, the failures nobody has seen, the window's
/// focus and what the taskbar shows.
#[derive(Clone, Debug)]
pub struct Watch {
    /// When each active job was first seen active (milliseconds on `clock`).
    started: HashMap<JobId, i64>,
    /// Jobs that failed while the window was in the background, until it gains focus.
    unseen: HashSet<JobId>,
    /// The window has focus (`views::window_focus`); unknown counts as not.
    pub focused: bool,
    /// What the taskbar shows now; nothing until the first change.
    shown: Dock,
    /// Wall time in milliseconds: how long jobs ran.
    pub clock: fn() -> i64,
    pub os: Os,
}

impl Default for Watch {
    fn default() -> Self {
        Watch { started: HashMap::new(), unseen: HashSet::new(), focused: false, shown: Dock::default(), clock: system_ms, os: Os::default() }
    }
}

impl Watch {
    /// A new snapshot `jobs` replaces `before`, at `now` (milliseconds on [`Watch::clock`]):
    /// jobs that start (or start again, retried) are timed; one that ends after more than
    /// 10 s, while the window isn't focused, is notified; a failure out of sight turns the bar
    /// red until the window gains focus; the taskbar follows.
    pub fn update(&mut self, before: &[JobView], jobs: &[JobView], now: i64) {
        for j in jobs {
            let was_active = before.iter().find(|o| o.id == j.id).map(|o| o.state.is_active());
            if j.state.is_active() {
                if was_active != Some(true) {
                    self.started.insert(j.id, now);
                }
                continue;
            }
            // Ended now: was active, or is new and already over (never timed: it ran 0 s).
            if was_active == Some(false) {
                continue;
            }
            let ran = self.started.remove(&j.id).map_or(0, |t| now - t);
            if matches!(j.state, TaskState::Failed { .. }) && !self.focused {
                self.unseen.insert(j.id);
            }
            if worth_telling(ran, self.focused)
                && let Some(n) = notice(j)
            {
                self.os.emit(OsEffect::Notify(n));
            }
        }
        self.started.retain(|id, _| jobs.iter().any(|j| j.id == *id && j.state.is_active()));
        self.unseen.retain(|id| jobs.iter().any(|j| j.id == *id));
        self.show(jobs);
    }

    /// The window gained or lost focus. Gaining it is the user looking: failures are seen.
    pub fn window_focus(&mut self, focused: bool, jobs: &[JobView]) {
        self.focused = focused;
        if focused {
            self.unseen.clear();
        }
        self.show(jobs);
    }

    /// What the taskbar shows now.
    pub fn shown(&self) -> &Dock {
        &self.shown
    }

    /// Tells the taskbar when what it should show changed.
    fn show(&mut self, jobs: &[JobView]) {
        let now = dock(jobs, !self.unseen.is_empty());
        if now != self.shown {
            self.shown = now.clone();
            self.os.emit(OsEffect::Taskbar(now));
        }
    }
}
