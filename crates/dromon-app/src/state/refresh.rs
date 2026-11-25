//! The folder on screen changing under the user (§4.5, §6.8): what the app does when a poll,
//! ⌘R, or a relist after an operation or a reconnect brings a new listing of it, and when the
//! folder turns out to be gone.
//!
//! - [`Fresh`]: the rows the last refresh brought, which the list highlights in mint as they
//!   slide in. The listing itself changes in place in `ConnView::refresh` (the selection keeps
//!   what's still there, a cursor whose row left moves to a neighbour, nothing scrolls).
//! - [`WindowState`]: how the window shows, which the service polls by (`Command::Window`);
//!   gaining focus polls the host on screen at once (`Command::Poll`).
//! - Folder vanished: a refresh of the folder on screen that finds it gone goes up to the
//!   nearest folder that still exists, one listing per level, and then says
//!   "/var/www/old was removed" in a toast.

use std::collections::HashSet;

use dromon_core::{Error, HostId, RemotePath};

use super::conn::{Listing, Nav};
use super::service::Command;
use super::App;

#[cfg(test)]
#[path = "refresh_tests.rs"]
mod tests;

/// How long new rows stay highlighted, in seconds (§4.5: "decays over about 1.2 s").
pub const HIGHLIGHT: f32 = 1.2;

/// The rows the last refresh that changed anything inserted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fresh {
    /// Counts refreshes that changed rows (0: none yet), so each one's highlight is new.
    pub seq: u64,
    pub inserted: HashSet<String>,
    /// The highlight has been put on screen and is fading (`Fresh::shown`).
    pub seen: bool,
    /// Keep the highlight where it starts: for the `poll-added` scene's picture, since
    /// `--snap` draws the frame in which it would already have begun to fade.
    pub hold: bool,
}

impl Fresh {
    /// The next refresh's rows.
    pub fn next(&self, inserted: Vec<String>) -> Fresh {
        Fresh { seq: self.seq + 1, inserted: inserted.into_iter().collect(), seen: false, hold: false }
    }

    /// Whether `name` came with the last refresh.
    pub fn is_new(&self, name: &str) -> bool {
        self.inserted.contains(name)
    }

    /// The highlight of refresh `seq` mounted: from now on it fades. True when that's news.
    pub fn shown(&mut self, seq: u64) -> bool {
        if seq == self.seq && !self.seen && !self.hold {
            self.seen = true;
            return true;
        }
        false
    }
}

/// Where the cursor goes when its row `gone` left: the first row after it (in the order
/// `before` had) that's still there, else the nearest one before it.
pub fn neighbour<'a>(before: &'a [String], gone: &str, still: impl Fn(&str) -> bool) -> Option<&'a str> {
    let at = before.iter().position(|n| n == gone)?;
    let after = before[at + 1..].iter().find(|n| still(n));
    let prior = || before[..at].iter().rev().find(|n| still(n));
    after.or_else(prior).map(String::as_str)
}

/// How the window shows, as last told to the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowState {
    pub visible: bool,
    pub focused: bool,
}

/// The polling side of the app: the window's state and whether the next commit animates.
#[derive(Clone, Debug)]
pub struct PollUi {
    /// What the window is now. It starts on screen, focus unknown (Rune reports it).
    pub window: WindowState,
    /// What the service was last told; `None` until the first report.
    told: Option<WindowState>,
    /// A refresh changed rows since the view last ran: the commit animates (`App::take_animate`).
    animate: bool,
}

impl Default for PollUi {
    fn default() -> Self {
        PollUi { window: WindowState { visible: true, focused: false }, told: None, animate: false }
    }
}

impl App {
    /// Whether a refresh changed rows since the last call: the caller makes its commit animate,
    /// which one no input caused wouldn't otherwise (Rune lands those at once).
    pub fn take_animate(&mut self) -> bool {
        std::mem::take(&mut self.poll.animate)
    }

    /// A refresh changed rows: the commit that shows it animates.
    pub(super) fn refreshed(&mut self) {
        self.poll.animate = true;
    }

    /// The window gained or lost focus. Gaining it polls the host on screen at once (§4.5).
    pub fn window_focus(&mut self, focused: bool) {
        let gained = focused && !self.poll.window.focused;
        self.poll.window.focused = focused;
        self.tell_window();
        if gained && self.poll.window.visible {
            let live = self.conn().filter(|c| c.interactive() && c.opened).map(|c| c.host.clone());
            if let Some(host) = live {
                self.send(Command::Poll { host });
            }
        }
    }

    /// The window went off screen (minimised, hidden, covered whole) or came back.
    pub fn window_visibility(&mut self, visible: bool) {
        self.poll.window.visible = visible;
        self.tell_window();
    }

    fn tell_window(&mut self) {
        let now = self.poll.window;
        if self.poll.told != Some(now) {
            self.poll.told = Some(now);
            self.send(Command::Window { visible: now.visible, focused: now.focused });
        }
    }

    /// A listing of `host`'s `path` failed with `error`. When that's the folder on screen gone
    /// (it had listed, or it's on the way up from one that had), goes up a level and lists that;
    /// true when it did. A folder navigated to that doesn't exist keeps its own state.
    pub(super) fn vanished(&mut self, host: &HostId, path: &RemotePath, error: &Error) -> bool {
        if !matches!(error, Error::NotFound(_)) {
            return false;
        }
        let Some(c) = self.conns.get_mut(host) else { return false };
        if *path != c.cwd || !c.interactive() {
            return false;
        }
        let gone = match (&c.vanished, &c.listing) {
            (Some(first), _) => first.clone(),
            (None, Listing::Ready(_)) => path.clone(),
            _ => return false,
        };
        let Some(parent) = path.parent() else { return false };
        // No history entry: Back shouldn't lead into a folder that's gone, nor stay put.
        c.go(parent.clone(), Nav::Replace);
        c.history.landed(&parent);
        c.vanished = Some(gone);
        let host = host.clone();
        self.remember_folder(&host, &parent);
        self.send(Command::List { host, path: parent });
        true
    }

    /// Something arrived for `host`'s `path`: if it ends a climb out of a vanished folder (the
    /// folder listed, or failed for another reason), the toast says what went.
    pub(super) fn vanish_landed(&mut self, host: &HostId, path: &RemotePath) {
        let Some(c) = self.conns.get_mut(host) else { return };
        if *path != c.cwd || matches!(c.listing, Listing::Loading) {
            return;
        }
        if let Some(gone) = c.vanished.take() {
            self.files.toasts.push(format!("{gone} was removed"));
        }
    }
}
