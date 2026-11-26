//! Adaptive polling of the folder on screen (§4.5), one [`Poller`] per host task: when the
//! next poll is due, and what a poll found compared with what the UI was last told.
//!
//! Pure: the connection task (`conn.rs`) owns the clock and the SFTP session; it asks
//! [`Poller::due`] when to start a poll, starts it only while no other listing is in flight
//! (exactly one per host: a navigation replaces a poll), and hands the result back.
//!
//! - Nothing polls until the window says it's on screen ([`Poller::window`]), and nothing
//!   while it's hidden.
//! - The interval comes from `dromon_sftp`'s schedule: by entry count and focus, backing off
//!   while polls take more than a quarter of it. Folders over 20,000 entries are never polled
//!   (⌘R only), nor folders whose listing failed (until they're listed again).
//! - A poll that finds what the UI already has sends nothing ([`Outcome::Quiet`]).

use dromon_core::{Entry, Error, RemotePath};
use dromon_sftp::{PollSchedule, Visibility, base_interval, diff};
use tokio::time::{Duration, Instant};

#[cfg(test)]
#[path = "poll_tests.rs"]
mod tests;

/// Whether a folder of `n` entries is polled at all (the plan's "> 20,000: manual").
pub fn pollable(n: usize) -> bool {
    base_interval(n, Visibility::Focused).is_some()
}

/// A listing's entries as they arrive. A listing for the UI keeps them only while the folder
/// is small enough to poll (a bigger one is only counted).
#[derive(Debug, Default)]
pub struct Collector {
    entries: Vec<Entry>,
    count: usize,
}

impl Collector {
    /// A page a listing also sends on to the UI.
    pub fn copy(&mut self, page: &[Entry]) {
        self.count += page.len();
        if pollable(self.count) {
            self.entries.extend_from_slice(page);
        } else {
            self.entries = Vec::new();
        }
    }

    /// A page only the poll reads: kept whatever the count, since a folder that grew past the
    /// limit since it was listed still goes to the UI whole.
    pub fn take(&mut self, page: Vec<Entry>) {
        self.count += page.len();
        self.entries.extend(page);
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.count
    }
}

/// Why a listing failed, as far as telling the UI again goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    NotFound,
    PermissionDenied,
    Other,
}

impl Failure {
    fn of(e: &Error) -> Failure {
        match e {
            Error::NotFound(_) => Failure::NotFound,
            Error::PermissionDenied(_) => Failure::PermissionDenied,
            _ => Failure::Other,
        }
    }
}

/// What the UI was last told the folder on screen holds.
#[derive(Debug)]
enum Seen {
    /// Its entries: small enough to poll.
    Entries(Vec<Entry>),
    /// Too many to poll.
    TooBig,
    /// Listing it failed: not polled until it's listed again.
    Failed(Failure),
}

/// What a finished poll means for the UI.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The folder changed: here is all of it.
    Changed(Vec<Entry>),
    /// Listing it now fails (it's gone, say), which the UI doesn't know yet.
    Failed(Error),
    /// Nothing new (or a poll of a folder no longer on screen).
    Quiet,
}

/// One host's polling.
#[derive(Debug)]
pub struct Poller {
    vis: Visibility,
    schedule: PollSchedule,
    /// The folder on screen (the last one listed for the UI).
    path: Option<RemotePath>,
    /// What the UI has of it; `None` while its listing is still coming.
    seen: Option<Seen>,
    due: Option<Instant>,
}

impl Default for Poller {
    fn default() -> Self {
        Poller { vis: Visibility::Hidden, schedule: PollSchedule::new(), path: None, seen: None, due: None }
    }
}

impl Poller {
    /// When the next poll is due; `None`: none is.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    pub fn visibility(&self) -> Visibility {
        self.vis
    }

    /// The folder's size, while it's one that's polled.
    fn polled_size(&self) -> Option<usize> {
        match &self.seen {
            Some(Seen::Entries(e)) if pollable(e.len()) => Some(e.len()),
            _ => None,
        }
    }

    /// Whether a poll may start now (the window shows, and the folder on screen is listed and
    /// small enough).
    pub fn may_poll(&self) -> bool {
        self.vis != Visibility::Hidden && self.polled_size().is_some()
    }

    /// The window shows as `vis` now: the next poll moves to the new interval from `now`
    /// (none while hidden). The backoff stays.
    pub fn window(&mut self, vis: Visibility, now: Instant) {
        self.vis = vis;
        self.due = self.polled_size().and_then(|n| self.schedule.current(n, vis)).map(|d| now + d);
    }

    /// The session opened afresh: no folder on screen yet.
    pub fn forget(&mut self) {
        self.path = None;
        self.seen = None;
        self.due = None;
        self.schedule.reset();
    }

    /// A listing of `path` for the UI starts (a navigation, ⌘R, a relist after an operation or
    /// a reconnect). Another folder starts afresh; no poll is due until it's listed.
    pub fn listing(&mut self, path: &RemotePath) {
        if self.path.as_ref() != Some(path) {
            self.forget();
            self.path = Some(path.clone());
        }
        self.seen = None;
        self.due = None;
    }

    /// That listing ended with `got`, after `took`.
    pub fn listed(&mut self, path: &RemotePath, got: Collector, took: Duration, now: Instant) {
        if self.path.as_ref() != Some(path) {
            return;
        }
        self.settle(got, took, now);
    }

    /// That listing failed: the UI shows why, and the folder isn't polled.
    pub fn list_failed(&mut self, path: &RemotePath, error: &Error) {
        if self.path.as_ref() == Some(path) {
            self.seen = Some(Seen::Failed(Failure::of(error)));
            self.due = None;
        }
    }

    fn settle(&mut self, got: Collector, took: Duration, now: Instant) {
        let n = got.count;
        let seen = if pollable(n) && got.entries.len() == n { Seen::Entries(got.entries) } else { Seen::TooBig };
        self.seen = Some(seen);
        self.due = self.schedule.next_delay(n, self.vis, took).map(|d| now + d);
    }

    /// A poll of `path` ended with `result` after `took`: what to tell the UI. A connection
    /// lost is the caller's (the session ends).
    pub fn polled(
        &mut self,
        path: &RemotePath,
        result: Result<Collector, Error>,
        took: Duration,
        now: Instant,
    ) -> Outcome {
        if self.path.as_ref() != Some(path) {
            return Outcome::Quiet;
        }
        match result {
            Ok(got) => {
                let same = match &self.seen {
                    Some(Seen::Entries(old)) => pollable(got.count) && diff(old, &got.entries).is_empty(),
                    _ => false,
                };
                if same {
                    self.settle(got, took, now);
                    return Outcome::Quiet;
                }
                // The UI gets every entry; a folder that grew past the limit is no longer kept.
                let entries = got.entries.clone();
                self.settle(got, took, now);
                Outcome::Changed(entries)
            }
            Err(error) => {
                let failure = Failure::of(&error);
                let known = matches!(self.seen, Some(Seen::Failed(f)) if f == failure);
                self.seen = Some(Seen::Failed(failure));
                self.due = None;
                if known { Outcome::Quiet } else { Outcome::Failed(error) }
            }
        }
    }
}
