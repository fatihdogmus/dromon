//! What every main window shares (§4.7): one in-app clipboard, one [`Saved`] and the store it
//! goes to, the windows' frames, which windows are open and which has focus, and the way to
//! the others ([`Peers`]). Each window's [`App`](super::App) holds a [`Common`]; the state of
//! a window opened with ⌘N gets a clone of its opener's. It all lives on the UI thread, where
//! the windows take turns, never two at once.
//!
//! Settings, the Recent section and the sidebar's hosts stay in each window's own state, and
//! a change in one reaches the others as [`News`](super::windows::News), so every window
//! redraws with it.

use std::cell::{Ref, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use dromon_core::{HostId, RemotePath};

use super::App;
use super::clip::Clip;
use super::frames::Frame;
use super::prefs::{NoStore, Saved, Store};
use super::service::WindowId;
use super::windows::{News, NoPeers, Peers};
use crate::theme::size;

impl App {
    /// Starts from settings remembered last time and saves later changes to `store`.
    pub fn remember(&mut self, saved: Saved, store: Box<dyn Store>) {
        if let Some(sort) = saved.sort {
            self.sort = sort;
        }
        if let Some(w) = saved.sidebar_width.filter(|w| w.is_finite()) {
            self.ui.sidebar_width = w.clamp(size::SIDEBAR_MIN, size::SIDEBAR_MAX);
        }
        self.common.remember(saved, store);
        self.restore_settings();
        self.restore_recent();
    }

    /// What's remembered between launches (shared by every window).
    pub fn saved(&self) -> Ref<'_, Saved> {
        self.common.saved()
    }

    /// Changes what's remembered and hands it to the store.
    pub(super) fn save(&mut self, f: impl FnOnce(&mut Saved)) {
        self.common.update_saved(f);
    }

    /// The in-app clipboard (§3.7), one for every window.
    pub fn clip(&self) -> Ref<'_, Option<Clip>> {
        self.common.clip()
    }

    /// Puts `clip` on the in-app clipboard (or empties it); the other windows redraw with it.
    pub fn set_clip(&mut self, clip: Option<Clip>) {
        self.common.set_clip(clip);
        self.tell(News::Clip);
    }

    /// The sidebar's resize grip was let go.
    pub fn sidebar_resized(&mut self) {
        let width = self.ui.sidebar_width;
        self.save(|s| s.sidebar_width = Some(width));
    }

    /// Remembers the folder on screen for `host`.
    /// `host` was opened: the welcome screen offers it at the next launch.
    pub(super) fn remember_host(&mut self, host: &HostId) {
        if self.saved().last_host.as_deref() != Some(host.as_str()) {
            self.save(|s| s.last_host = Some(host.as_str().to_owned()));
        }
    }

    /// The host the welcome screen offers: the last one opened, while the sidebar still has it.
    pub fn last_host(&self) -> Option<HostId> {
        let id = HostId(self.saved().last_host.clone()?);
        self.hosts.get(&id).is_some().then_some(id)
    }

    pub(super) fn remember_folder(&mut self, host: &HostId, path: &RemotePath) {
        if self.saved().folder(host.as_str()) != Some(path.as_str()) {
            self.save(|s| s.set_folder(host.as_str(), path.as_str()));
        }
    }

    /// Replaces the store, returning the old one (quitting drops the real store, which writes
    /// what it still holds).
    pub fn take_store(&mut self) -> Box<dyn Store> {
        self.common.take_store()
    }

    /// The real windows (`crate::windows`), once the app runs.
    pub fn set_peers(&mut self, peers: Rc<dyn Peers>) {
        self.common.set_peers(peers);
    }
}

/// The app-wide state, shared by clones.
#[derive(Clone)]
pub struct Common(Rc<RefCell<Inner>>);

struct Inner {
    clip: Option<Clip>,
    saved: Saved,
    store: Box<dyn Store>,
    peers: Rc<dyn Peers>,
    /// The main windows open now.
    open: BTreeSet<WindowId>,
    /// The main windows that have focus, as each last said (one at most, but a window's loss
    /// and the next one's gain may come in either order).
    focused: BTreeSet<WindowId>,
    /// The windows whose quit hook ran in the quit under way.
    quitting: BTreeSet<WindowId>,
    /// Each open window's frame as last recorded (and the last window's after it closed).
    frames: BTreeMap<WindowId, Frame>,
    /// The id the next window gets.
    next: u32,
}

impl Default for Common {
    fn default() -> Self {
        Common(Rc::new(RefCell::new(Inner {
            clip: None,
            saved: Saved::default(),
            store: Box::new(NoStore),
            peers: Rc::new(NoPeers),
            open: BTreeSet::from([WindowId::FIRST]),
            focused: BTreeSet::new(),
            quitting: BTreeSet::new(),
            frames: BTreeMap::new(),
            next: WindowId::FIRST.0 + 1,
        })))
    }
}

impl Common {
    pub fn clip(&self) -> Ref<'_, Option<Clip>> {
        Ref::map(self.0.borrow(), |i| &i.clip)
    }

    pub fn set_clip(&self, clip: Option<Clip>) {
        self.0.borrow_mut().clip = clip;
    }

    pub fn saved(&self) -> Ref<'_, Saved> {
        Ref::map(self.0.borrow(), |i| &i.saved)
    }

    /// Changes what's remembered and hands it to the store.
    pub fn update_saved(&self, f: impl FnOnce(&mut Saved)) {
        let mut inner = self.0.borrow_mut();
        let Inner { saved, store, .. } = &mut *inner;
        f(saved);
        store.save(saved);
    }

    /// Starts from `saved` and saves later changes to `store`.
    pub fn remember(&self, saved: Saved, store: Box<dyn Store>) {
        let mut inner = self.0.borrow_mut();
        inner.saved = saved;
        inner.store = store;
    }

    /// Replaces the store, returning the old one (dropping the real store writes what it holds).
    pub fn take_store(&self) -> Box<dyn Store> {
        std::mem::replace(&mut self.0.borrow_mut().store, Box::new(NoStore))
    }

    pub fn peers(&self) -> Rc<dyn Peers> {
        self.0.borrow().peers.clone()
    }

    /// The real windows (`crate::windows`); until then there are no others.
    pub fn set_peers(&self, peers: Rc<dyn Peers>) {
        self.0.borrow_mut().peers = peers;
    }

    /// A number for a new window, never handed out before.
    pub fn next_id(&self) -> WindowId {
        let mut inner = self.0.borrow_mut();
        let id = WindowId(inner.next);
        inner.next += 1;
        id
    }

    /// `window` counts as open from now on.
    pub fn opened(&self, window: WindowId) {
        self.0.borrow_mut().open.insert(window);
    }

    /// How many main windows are open.
    pub fn open_count(&self) -> usize {
        self.0.borrow().open.len()
    }

    /// The open window with the lowest number.
    pub fn first_open(&self) -> Option<WindowId> {
        self.0.borrow().open.first().copied()
    }

    /// `window` gained or lost focus.
    pub fn focus(&self, window: WindowId, focused: bool) {
        let mut inner = self.0.borrow_mut();
        if focused {
            inner.focused.insert(window);
        } else {
            inner.focused.remove(&window);
        }
    }

    /// The main window that has focus, if one does.
    pub fn focused(&self) -> Option<WindowId> {
        self.0.borrow().focused.first().copied()
    }

    /// `window` closed; `last` when it was the only one open.
    pub fn closed(&self, window: WindowId) -> bool {
        let mut inner = self.0.borrow_mut();
        inner.open.remove(&window);
        inner.focused.remove(&window);
        let last = inner.open.is_empty();
        if !last {
            // Only one window reopens next time: the first of those open at the end.
            inner.frames.remove(&window);
            let frames = inner.frames.values().copied().collect();
            let Inner { saved, store, .. } = &mut *inner;
            saved.frames = frames;
            store.save(saved);
        }
        last
    }

    /// `window`'s quit hook ran: true once every open window's has (the store may go).
    pub fn quit(&self, window: WindowId) -> bool {
        let mut inner = self.0.borrow_mut();
        inner.quitting.insert(window);
        inner.open.is_subset(&inner.quitting)
    }

    /// Remembers where `window` is, among the frames saved for next time.
    pub fn record_frame(&self, window: WindowId, frame: Frame) {
        let mut inner = self.0.borrow_mut();
        if inner.frames.get(&window) == Some(&frame) {
            return;
        }
        inner.frames.insert(window, frame);
        let frames = inner.frames.values().copied().collect();
        let Inner { saved, store, .. } = &mut *inner;
        saved.frames = frames;
        store.save(saved);
    }
}
