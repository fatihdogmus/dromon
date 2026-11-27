//! Several main windows (⌘N, ⌘W; §5 M4, §7, §12 "⌘N for more windows").
//!
//! Every main window is an [`App`] of its own: its own host on screen, folder on each host,
//! history, selection, filter, sort, palette, popovers and sheets. The windows share the
//! service (one connection per host, one transfer engine, one set of hosts:
//! [`Backend::another`](super::Backend::another)) and what [`Common`](super::common::Common)
//! holds (the in-app clip, what's remembered, the store). The service's events about hosts,
//! listings and transfers reach every window, and each takes what concerns it. What one window
//! changes that the others show too (settings, the Recent section, the clip) it tells them as
//! [`News`].
//!
//! - ⌘N opens a window on the welcome state ("Pick a host"), cascaded from the one it came from
//!   (`crate::windows` places it).
//! - ⌘W closes the window. Closing the last one quits the app, on macOS too; only then do
//!   running transfers and edited files' saves not on the server yet ask first ("N transfers
//!   in progress").
//! - One window leads ([`App::leads`]): the first one open. App-wide signs, like the Dock
//!   badge, taskbar progress and notifications, go through it alone, so there's one of each:
//!   every window keeps its transfers' `Watch` current, and only the leader's speaks. When it
//!   closes, its badge goes and the next window's comes ([`News::Windows`]).
//! - The app has focus while any main window has it ([`App::focus_moved`]): only then is a
//!   finished job's notification held back. A notification's click opens the popover in the
//!   focused window, else the leading one ([`App::notice_window`]).
//! - The done flourish plays in every window that shows the job.

use dromon_core::{HostSource, HostSpec};

use super::frames::Frame;
use super::ops::Ops;
use super::service::{Command, Silent, WindowId};
use super::settings::Settings;
use super::{App, Effect};
use crate::theme::Appearance;

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;

/// What one window tells the others when it changed something they show too.
#[derive(Clone, Debug, PartialEq)]
pub enum News {
    /// The settings now (a change on one window's Settings page, applied there already).
    Settings(Settings),
    /// The Recent section, newest first.
    Recent(Vec<HostSpec>),
    /// The in-app clip changed: cut rows dim, or stop dimming.
    Clip,
    /// A window closed or its focus changed: which window leads, and whether the app has
    /// focus, may have too (each window reads them from `Common`, as they are by then).
    Windows,
}

/// The app's main windows, as one window's state reaches them (`crate::windows` in the app).
pub trait Peers {
    /// Whether another window can open at all (not in scenes, tests and headless runs).
    fn can_open(&self) -> bool;
    /// Opens a window on `app`, a new window's state ([`App::spawn`]), near window `from`.
    fn open_window(&self, from: WindowId, app: App);
    /// Hands `news` to every main window but `from`.
    fn tell(&self, from: WindowId, news: News);
    /// Where `window` is on the desktop now, if that's known.
    fn frame(&self, window: WindowId) -> Option<Frame>;
    /// `window` closed: nothing goes to it any more.
    fn closed(&self, window: WindowId);
    /// The theme is `theme` now (at launch, and after a change on a Settings page): every
    /// window's own frame (title bar, the macOS appearance) follows it.
    fn theme(&self, _theme: Appearance) {}
}

/// No other windows: scenes, tests and headless runs.
pub struct NoPeers;

impl Peers for NoPeers {
    fn can_open(&self) -> bool {
        false
    }
    fn open_window(&self, _: WindowId, _: App) {}
    fn tell(&self, _: WindowId, _: News) {}
    fn frame(&self, _: WindowId) -> Option<Frame> {
        None
    }
    fn closed(&self, _: WindowId) {}
}

impl App {
    /// Whether this window speaks for the app: the first one open. The Dock badge, taskbar
    /// progress and transfer notifications are set from it alone (one for the whole app); when
    /// it closes, the next window leads.
    pub fn leads(&self) -> bool {
        self.common.first_open() == Some(self.window)
    }

    /// How many main windows are open, this one included.
    pub fn windows_open(&self) -> usize {
        self.common.open_count()
    }

    /// The window a click on a transfer's notification shows its job in: the focused main
    /// window, else the leading one.
    pub fn notice_window(&self) -> Option<WindowId> {
        self.common.focused().or_else(|| self.common.first_open())
    }

    /// This window gained or lost focus (`views::window_focus`). The app has it while any main
    /// window does: every window's transfers learn so, and the leader's notifies only without.
    pub fn focus_moved(&mut self, focused: bool) {
        self.common.focus(self.window, focused);
        self.follow_windows();
        self.tell(News::Windows);
    }

    /// Catches up on the windows: whether this one leads now (one that has just come to lead
    /// shows the badge), and whether the app has focus.
    fn follow_windows(&mut self) {
        self.transfers.watch.lead(self.leads());
        self.transfers.window_focus(self.common.focused().is_some());
    }

    /// The state of a new main window on the same service: the welcome state, this window's
    /// hosts, settings and transfers, the sort remembered last. `None` when the backend serves
    /// one window only.
    pub fn spawn(&mut self) -> Option<App> {
        let id = self.common.next_id();
        let backend = self.backend.another(id)?;
        let mut app = App::new(self.hosts.clone(), self.now, backend);
        app.window = id;
        app.common = self.common.clone();
        app.common.opened(id);
        // Its file operations' ids are its own: the service's answers go to every window.
        app.ops = Ops::numbered_from(u64::from(id.0) << 32);
        app.settings = self.settings.clone();
        app.ui.appearance = self.settings.theme;
        app.ui.access = self.ui.access;
        app.ui.sidebar_width = self.ui.sidebar_width;
        app.show_hidden_default = self.settings.show_hidden;
        app.sort = self.common.saved().sort.unwrap_or(self.sort);
        // Its Settings page writes the download folder as this one's does.
        app.ui.settings_page.home = self.ui.settings_page.home.clone();
        app.wall_ms = self.wall_ms;
        app.downloads = self.downloads.clone();
        app.transfers.jobs = self.transfers.jobs.clone();
        // The service said how this copy was installed once, before the window was there.
        app.update.install = self.update.install.clone();
        app.edits = self.edits.for_window();
        // It times the jobs from when this window saw them start, and speaks once it leads.
        app.transfers.watch = self.transfers.watch.follower();
        Some(app)
    }

    /// ⌘N, the File menu's New Window.
    pub(super) fn new_window(&mut self) {
        let peers = self.common.peers();
        if !peers.can_open() {
            return;
        }
        if let Some(app) = self.spawn() {
            peers.open_window(self.window, app);
        }
    }

    /// ⌘W, the File menu's Close Window: closes now unless it's the last window with transfers
    /// running (then the quit sheet asks).
    pub(super) fn close_window(&mut self, fx: &mut Vec<Effect>) {
        if self.close_requested() {
            fx.push(Effect::CloseWindow);
        }
    }

    /// Tells the other windows.
    pub(super) fn tell(&self, news: News) {
        self.common.peers().tell(self.window, news);
    }

    /// Another window changed something this one shows too.
    pub fn hear(&mut self, news: News) {
        match news {
            News::Settings(settings) => self.adopt_settings(settings),
            News::Recent(list) => self.replace_section(HostSource::Recent, list),
            // Nothing to keep: the clip is shared; the view redraws with it.
            News::Clip => {}
            News::Windows => self.follow_windows(),
        }
    }

    /// Remembers where the window is, for the next launch.
    fn record_frame(&self) {
        if let Some(frame) = self.common.peers().frame(self.window) {
            self.common.record_frame(self.window, frame);
        }
    }

    /// The window closes (`on_close`): its frame is kept if it's the last, the service forgets
    /// it, and its end of the service's channel goes. If it led, its badge goes, and the next
    /// window leads.
    pub fn window_closed(&mut self) {
        self.record_frame();
        self.send(Command::WindowClosed);
        self.transfers.watch.resign();
        self.common.closed(self.window);
        self.tell(News::Windows);
        self.common.peers().closed(self.window);
        self.backend = Box::new(Silent::default());
    }

    /// The app quits (`on_quit`, in every window): the frame is kept, this window's end of the
    /// service's channel goes (the service stops once every window's has: connections close and
    /// secrets are wiped), and the last window to hear it writes what's remembered.
    pub fn quitting(&mut self) {
        self.record_frame();
        self.backend = Box::new(Silent::default());
        if self.common.quit(self.window) {
            // Dropping the settings writer waits for its last write.
            drop(self.common.take_store());
        }
    }
}
