//! The main windows as one host's connection task sees them (§4.5, §5 M4 "⌘N new window").
//! Pure, like `poll.rs`: the task owns the clock and the session.
//!
//! - [`Watch`]: the folder each window has on screen on this host. Every one of them is
//!   polled (each with its own [`Poller`]) while a window showing it is visible, at the focused
//!   interval while one of those is focused; a window's own focus polls its folder at once. One
//!   of them, the focused (else a visible) window's, is the one live updates watch
//!   ([`Watch::live_folder`]), polled less often while they do.
//! - [`Route`]: which window a prompt goes to: the one that answered this host's last prompt
//!   (so a wrong password asks again in place), else the focused one, else the one that
//!   started the connection, else every window. Answering in one closes it in all; a window that
//!   closes hands its prompts on.

use std::collections::{BTreeMap, BTreeSet};

use dromon_core::{ConnStatus, Entry, Error, Prompt, PromptId, RemotePath};
use dromon_sftp::Visibility;
use tokio::time::{Duration, Instant};

use super::lifecycle::Phase;
use super::poll::{Collector, Outcome, Poller};
use crate::state::WindowId;

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;

/// Better shown first: focused, visible, hidden.
fn rank(v: Visibility) -> u8 {
    match v {
        Visibility::Focused => 2,
        Visibility::VisibleUnfocused => 1,
        Visibility::Hidden => 0,
    }
}

/// The folders on screen on one host, by window, and their polling.
#[derive(Debug, Default)]
pub struct Watch {
    /// Each window's folder on screen here (the last it listed).
    shown: BTreeMap<WindowId, RemotePath>,
    /// How each window shows, as it last said (`Command::Window`).
    vis: BTreeMap<WindowId, Visibility>,
    /// One poller per folder some window shows.
    polls: BTreeMap<RemotePath, Poller>,
    /// The folder watched live (`live.rs`), polled less often.
    live: Option<RemotePath>,
}

impl Watch {
    /// How the windows showing `path` show, at best.
    fn seen_as(&self, path: &RemotePath) -> Visibility {
        let showing = self.shown.iter().filter(|(_, p)| *p == path);
        showing
            .map(|(w, _)| self.vis.get(w).copied().unwrap_or(Visibility::Hidden))
            .max_by_key(|v| rank(*v))
            .unwrap_or(Visibility::Hidden)
    }

    /// Drops the pollers of folders no window shows; gives each other one the visibility of
    /// the windows showing it (a change reschedules it from `now`).
    fn resync(&mut self, now: Instant) {
        let paths: BTreeSet<RemotePath> = self.shown.values().cloned().collect();
        self.polls.retain(|p, _| paths.contains(p));
        for path in paths {
            let vis = self.seen_as(&path);
            let live = self.live.as_ref() == Some(&path);
            let poller = self.polls.entry(path).or_default();
            poller.live(live, now);
            if poller.visibility() != vis {
                poller.window(vis, now);
            }
        }
    }

    /// `path` is watched live from `now` (`None`: no folder is): its polls stretch, and those
    /// of a folder that was go back to normal.
    pub fn set_live(&mut self, path: Option<RemotePath>, now: Instant) {
        self.live = path;
        for (p, poller) in &mut self.polls {
            poller.live(self.live.as_ref() == Some(p), now);
        }
    }

    /// The folder to watch live: the focused window's on this host, else a visible one's. None
    /// while no window shows one here, or when that folder isn't polled (too big, or its
    /// listing failed).
    pub fn live_folder(&self) -> Option<RemotePath> {
        let (_, path) = self
            .shown
            .iter()
            .map(|(w, p)| (self.vis.get(w).copied().unwrap_or(Visibility::Hidden), p))
            .filter(|(v, _)| *v != Visibility::Hidden)
            .max_by_key(|(v, _)| rank(*v))?;
        self.polls.get(path).filter(|p| p.watchable()).map(|_| path.clone())
    }

    /// `window` has `path` on screen now (it asked for its listing).
    pub fn show(&mut self, window: WindowId, path: RemotePath, now: Instant) {
        self.shown.insert(window, path);
        self.resync(now);
    }

    /// `window` shows as `vis` now.
    pub fn window(&mut self, window: WindowId, vis: Visibility, now: Instant) {
        self.vis.insert(window, vis);
        self.resync(now);
    }

    /// `window` closed.
    pub fn closed(&mut self, window: WindowId, now: Instant) {
        self.vis.remove(&window);
        self.shown.remove(&window);
        self.resync(now);
    }

    /// The session opened afresh: no folder is on screen until a window lists one.
    pub fn forget(&mut self) {
        self.shown.clear();
        self.polls.clear();
    }

    /// `window`'s folder on screen here.
    pub fn folder(&self, window: WindowId) -> Option<&RemotePath> {
        self.shown.get(&window)
    }

    /// Whether some window shows `path`.
    pub fn shows(&self, path: &RemotePath) -> bool {
        self.polls.contains_key(path)
    }

    /// Every folder on screen, each once.
    pub fn folders(&self) -> Vec<RemotePath> {
        self.polls.keys().cloned().collect()
    }

    /// Whether a window showing `path` is on screen.
    pub fn visible(&self, path: &RemotePath) -> bool {
        self.seen_as(path) != Visibility::Hidden
    }

    /// The focused window, if it's one of the app's (only one is).
    pub fn focused(&self) -> Option<WindowId> {
        self.vis.iter().find(|(_, v)| **v == Visibility::Focused).map(|(w, _)| *w)
    }

    /// When the next poll is due, of any folder.
    pub fn due(&self) -> Option<Instant> {
        self.polls.values().filter_map(Poller::due).min()
    }

    /// The folder whose poll is due first.
    pub fn next_due(&self) -> Option<RemotePath> {
        self.polls.iter().filter_map(|(p, x)| x.due().map(|d| (d, p))).min().map(|(_, p)| p.clone())
    }

    /// Whether `path` may be polled now (shown, listed, small enough).
    pub fn may_poll(&self, path: &RemotePath) -> bool {
        self.polls.get(path).is_some_and(Poller::may_poll)
    }

    /// A listing of `path` for the windows starts.
    pub fn listing(&mut self, path: &RemotePath) {
        if let Some(p) = self.polls.get_mut(path) {
            p.listing(path);
        }
    }

    pub fn listed(&mut self, path: &RemotePath, got: Collector, took: Duration, now: Instant) {
        if let Some(p) = self.polls.get_mut(path) {
            p.listed(path, got, took, now);
        }
    }

    pub fn list_failed(&mut self, path: &RemotePath, error: &Error) {
        if let Some(p) = self.polls.get_mut(path) {
            p.list_failed(path, error);
        }
    }

    /// A poll of `path` ended: what to tell the windows (nothing for a folder none shows).
    pub fn polled(
        &mut self,
        path: &RemotePath,
        result: Result<Collector, Error>,
        took: Duration,
        now: Instant,
    ) -> Outcome {
        match self.polls.get_mut(path) {
            Some(p) => p.polled(path, result, took, now),
            None => Outcome::Quiet,
        }
    }

    /// Like [`Watch::polled`], with a whole listing in hand (a fixture's).
    pub fn polled_entries(&mut self, path: &RemotePath, result: Result<Vec<Entry>, Error>, now: Instant) -> Outcome {
        let result = result.map(|entries| {
            let mut got = Collector::default();
            got.take(entries);
            got
        });
        self.polled(path, result, Duration::ZERO, now)
    }
}

/// Where one host's prompts go.
#[derive(Debug, Default)]
pub struct Route {
    /// The window whose Connect started the connection.
    starter: Option<WindowId>,
    /// The window that answered this host's last prompt, until the connection settles.
    answered: Option<WindowId>,
    /// The prompts on screen, and the window each went to (`None`: every window).
    open: Vec<(Prompt, Option<WindowId>)>,
    /// Windows that asked to connect while the connection was on its way back: they hear
    /// `Opened` once it is.
    pub joining: BTreeSet<WindowId>,
}

impl Route {
    /// `window` started the connection.
    pub fn started_by(&mut self, window: WindowId) {
        self.starter = Some(window);
        self.answered = None;
    }

    /// The window a new prompt goes to (`None`: every window).
    pub fn target(&self, watch: &Watch) -> Option<WindowId> {
        self.answered.or_else(|| watch.focused()).or(self.starter)
    }

    /// `prompt` went to `to`.
    pub fn asked(&mut self, prompt: Prompt, to: Option<WindowId>) {
        self.open.push((prompt, to));
    }

    /// `by` answered prompt `id`: the host's next prompt goes there too.
    pub fn answered(&mut self, id: PromptId, by: WindowId) {
        self.gone(id);
        self.answered = Some(by);
    }

    /// Prompt `id` is off screen (answered, or ssh hung up).
    pub fn gone(&mut self, id: PromptId) {
        self.open.retain(|(p, _)| p.id != id);
    }

    /// The connection settled (up, failed, disconnected).
    pub fn settled(&mut self) {
        self.answered = None;
    }

    /// `window` asked to see the host's open prompts (the banner's Sign in): they are shown
    /// there now, and it takes the host's later prompts too, until the connection settles.
    /// Returns the prompts to send it.
    pub fn show_in(&mut self, window: WindowId) -> Vec<Prompt> {
        self.answered = Some(window);
        self.open
            .iter_mut()
            .map(|(p, at)| {
                *at = Some(window);
                p.clone()
            })
            .collect()
    }

    /// `window` closed: the prompts it had, each with the window to show it in now.
    pub fn closed(&mut self, window: WindowId, watch: &Watch) -> Vec<(Prompt, Option<WindowId>)> {
        self.joining.remove(&window);
        if self.starter == Some(window) {
            self.starter = None;
        }
        if self.answered == Some(window) {
            self.answered = None;
        }
        let to = self.target(watch);
        let mut moved = Vec::new();
        for (p, at) in &mut self.open {
            if *at == Some(window) {
                *at = to;
                moved.push((p.clone(), to));
            }
        }
        moved
    }
}

/// What the windows are told the connection is in `phase`, for a window that joins it.
pub fn status_of(phase: &Phase) -> Option<ConnStatus> {
    Some(match phase {
        Phase::Idle => return None,
        Phase::Starting { prompts, .. } if *prompts > 0 => ConnStatus::NeedsInput,
        Phase::Starting { attempt: 0, .. } => ConnStatus::Connecting,
        Phase::Starting { attempt, .. } | Phase::Waiting { attempt } => ConnStatus::Reconnecting { attempt: *attempt },
        Phase::Ready => ConnStatus::Ready,
        Phase::Failed(why) => ConnStatus::Failed(why.clone()),
        Phase::Disconnected => ConnStatus::Disconnected,
    })
}
