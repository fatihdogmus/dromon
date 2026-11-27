//! What a screen reader is told without focus moving (§6.9): a transfer that finished or
//! failed, a delete that's done or stopped, a connection lost and back, a note such as "the
//! folder on screen was removed". The view's poll asks [`Announcer::news`] each frame and says
//! it through the window's live region (`Tx::announce`, polite).
//!
//! It compares what it last saw with the state now, so it says each change once, whichever
//! event brought it, and nothing for what was there when it first looked (a window's start, a
//! scene).

use std::collections::{BTreeMap, BTreeSet};

use dromon_core::{ConnStatus, HostId};
use dromon_transfer::{JobId, JobKind, JobView, TaskState};

use super::App;
use super::service::FileOpKind;
use super::toast::{ToastId, Tone};

#[cfg(test)]
#[path = "focus_tests.rs"]
mod tests;

/// Where a job is, as far as announcing goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Going,
    Done,
    Failed,
    Cancelled,
}

/// Where a connection is, as far as announcing goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Up,
    /// Lost after it was open: reconnecting, or waiting for a sign-in to.
    Down,
    Failed,
    Other,
}

#[derive(Debug, Default)]
pub struct Announcer {
    /// It has looked once: what was there then isn't news.
    primed: bool,
    jobs: BTreeMap<JobId, End>,
    links: BTreeMap<HostId, Link>,
    /// Deletes already said done (their `OpId`s).
    ops: BTreeSet<u64>,
    /// Failed deletes' and notes' toasts already said.
    toasts: BTreeSet<ToastKey>,
}

/// [`ToastId`], ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ToastKey {
    Op(u64),
    Note(u64),
}

impl From<ToastId> for ToastKey {
    fn from(id: ToastId) -> Self {
        match id {
            ToastId::Op(op) => ToastKey::Op(op.0),
            ToastId::Note(n) => ToastKey::Note(n),
        }
    }
}

fn end(j: &JobView) -> End {
    match j.state {
        TaskState::Done => End::Done,
        TaskState::Failed { .. } => End::Failed,
        TaskState::Cancelled => End::Cancelled,
        _ => End::Going,
    }
}

fn link(status: &ConnStatus, opened: bool) -> Link {
    match status {
        ConnStatus::Ready => Link::Up,
        ConnStatus::Reconnecting { .. } => Link::Down,
        ConnStatus::NeedsInput if opened => Link::Down,
        ConnStatus::Failed(_) => Link::Failed,
        _ => Link::Other,
    }
}

/// "Download", "Upload", "Copy", "Move".
fn kind_word(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Download => "Download",
        JobKind::Upload => "Upload",
        JobKind::RemoteCopy | JobKind::CrossHostCopy => "Copy",
        JobKind::Move | JobKind::CrossHostMove => "Move",
    }
}

impl Announcer {
    /// What changed since the last look, in words, or `None`.
    pub fn news(&mut self, s: &App) -> Option<String> {
        let mut said = Vec::new();
        let primed = std::mem::replace(&mut self.primed, true);

        let mut jobs = BTreeMap::new();
        for j in &s.transfers.jobs {
            let now = end(j);
            if primed && self.jobs.get(&j.id) != Some(&now) {
                let what = format!("{} of {}", kind_word(j.kind), j.title);
                match &j.state {
                    TaskState::Done => said.push(format!("{what} finished")),
                    TaskState::Failed { .. } => {
                        said.push(format!("{what} failed: {}", super::transfers::failure_text(j)));
                    }
                    _ => {}
                }
            }
            jobs.insert(j.id, now);
        }
        self.jobs = jobs;

        let mut links = BTreeMap::new();
        for (id, c) in &s.conns {
            let now = link(&c.status, c.opened);
            let before = self.links.get(id).copied().unwrap_or(Link::Other);
            if primed && now != before {
                let host = s.hosts.get(id).map_or(id.as_str(), |h| h.display_name());
                match (before, now) {
                    (Link::Up, Link::Down) => said.push(format!("Connection to {host} lost. Reconnecting…")),
                    (Link::Down, Link::Up) => said.push(format!("Reconnected to {host}")),
                    (_, Link::Failed) => said.push(format!("Couldn't connect to {host}")),
                    _ => {}
                }
            }
            links.insert(id.clone(), now);
        }
        self.links = links;

        for op in s.ops.ops.iter().filter(|o| o.kind == FileOpKind::Delete && o.succeeded()) {
            if self.ops.insert(op.id.0) && primed {
                said.push(format!("Deleted {}", crate::fmt::items(op.items)));
            }
        }
        // A failed delete's toast and a note say what happened; a running delete's says it's
        // under way, which the user just asked for.
        for t in s.toasts().into_iter().filter(|t| t.tone != Tone::Busy) {
            if self.toasts.insert(t.id.into()) && primed {
                said.push(t.text);
            }
        }

        (!said.is_empty()).then(|| said.join(". "))
    }
}
