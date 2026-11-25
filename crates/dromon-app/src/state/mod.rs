//! App state (§4.7) and its updates. Everything here is plain data and functions of it: views
//! turn input into an [`Action`] and call [`App::act`]; the service's answers arrive as
//! [`ServiceEvent`]s through [`App::apply`]. Neither touches Rune, so both are unit-tested
//! directly (`state/tests.rs`).

pub mod conn;
pub mod history;
pub mod hosts;
pub mod keys;
pub mod selection;
pub mod service;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};

use dromon_core::{ConnStatus, HostId, RemotePath, SortKey};
use rune::TextEdit;

pub use conn::{ConnView, Filter, ListError, Listing, Nav, Sort};
pub use hosts::{HostDirectory, Section};
pub use keys::Shortcut;
pub use selection::{Selection, TypeSelect};
pub use service::{Backend, Command, ServiceEvent, Silent};

use crate::theme::{Appearance, size};

/// Window-level UI state that isn't about any one connection.
pub struct UiState {
    pub appearance: Appearance,
    pub sidebar_width: f32,
    /// The ⌘F field's text model; its query is mirrored into the active `ConnView::filter`.
    pub filter_edit: TextEdit,
    pub type_select: TypeSelect,
    /// A row to bring to the middle of the list once it mounts (a scene's scroll position).
    pub center_on_mount: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            appearance: Appearance::System,
            sidebar_width: size::SIDEBAR,
            filter_edit: TextEdit::new(""),
            type_select: TypeSelect::default(),
            center_on_mount: None,
        }
    }
}

pub struct App {
    pub hosts: HostDirectory,
    pub conns: BTreeMap<HostId, ConnView>,
    /// The host on screen; `None` is the welcome state.
    pub active: Option<HostId>,
    pub ui: UiState,
    /// Seconds since the epoch that relative dates are measured from.
    pub now: i64,
    /// Show dotfiles by default (§8: on).
    pub show_hidden_default: bool,
    /// Sort remembered globally (§12).
    pub sort: Sort,
    pub backend: Box<dyn Backend>,
    inbox: VecDeque<ServiceEvent>,
}

/// What the user did, in the app's terms.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    SelectHost(HostId),
    /// Double-click or Return on a row: navigate into folders (files will download in M2).
    Open(String),
    OpenSelection,
    GoUp,
    Back,
    Forward,
    GoTo(RemotePath),
    /// A click on a row; `extend` is Shift, `toggle` is ⌘ (Ctrl elsewhere).
    Click { name: String, extend: bool, toggle: bool },
    Move { delta: isize, extend: bool },
    End { last: bool, extend: bool },
    SelectAll,
    /// Type-to-select: `text` typed at `now` (seconds, any monotonic clock).
    Type { text: String, now: f64 },
    Sort(SortKey),
    ToggleHidden,
    OpenFilter,
    CloseFilter,
    /// The filter field's text changed to this.
    FilterChanged(String),
    Escape,
    /// The quick-connect popover (M4); nothing yet.
    QuickConnect,
}

/// What an action asks the view layer to do afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Scroll the row with this name into view.
    Reveal(String),
    /// Scroll the list back to its top (a new folder).
    ScrollTop,
    /// Hand the keyboard back from the filter field to the list.
    Blur,
}

impl App {
    pub fn new(hosts: HostDirectory, now: i64, backend: Box<dyn Backend>) -> App {
        App {
            hosts,
            conns: BTreeMap::new(),
            active: None,
            ui: UiState::default(),
            now,
            show_hidden_default: true,
            sort: Sort::default(),
            backend,
            inbox: VecDeque::new(),
        }
    }

    pub fn conn(&self) -> Option<&ConnView> {
        self.active.as_ref().and_then(|h| self.conns.get(h))
    }

    pub fn conn_mut(&mut self) -> Option<&mut ConnView> {
        let host = self.active.clone()?;
        self.conns.get_mut(&host)
    }

    /// Sends `cmd` and applies whatever the backend answers right away.
    fn send(&mut self, cmd: Command) {
        let events = self.backend.send(cmd);
        self.inbox.extend(events);
        while let Some(ev) = self.inbox.pop_front() {
            self.apply_one(ev);
        }
    }

    /// Applies one event from the service. This is the integration point for the real
    /// service thread: call it on the UI thread (from `AppHandle::update_if` or `poll`) for
    /// each event that arrives. It never blocks and may send follow-up commands.
    pub fn apply(&mut self, ev: ServiceEvent) {
        self.inbox.push_back(ev);
        while let Some(ev) = self.inbox.pop_front() {
            self.apply_one(ev);
        }
    }

    fn apply_one(&mut self, ev: ServiceEvent) {
        match ev {
            ServiceEvent::Status { host, status } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    c.status = status;
                }
            }
            ServiceEvent::Opened { host, start } => {
                let Some(c) = self.conns.get_mut(&host) else { return };
                c.status = ConnStatus::Ready;
                c.cwd = start.clone();
                c.listing = Listing::Loading;
                self.send(Command::List { host, path: start });
            }
            ServiceEvent::Listing { host, path, entries } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    c.set_listing(&path, entries);
                    if c.filter.is_on() && self.active.as_ref() == Some(&host) {
                        c.set_filter(Filter::On(self.ui.filter_edit.text.clone()));
                    }
                }
            }
            ServiceEvent::ListFailed { host, path, error } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    c.set_failed(&path, error.into());
                }
            }
        }
    }

    /// Carries out `action`; returns what the view should do next.
    pub fn act(&mut self, action: Action) -> Vec<Effect> {
        let mut fx = Vec::new();
        match action {
            Action::SelectHost(host) => self.select_host(host),
            Action::Open(name) => self.open(&name, &mut fx),
            Action::OpenSelection => {
                if let Some(name) = self.conn().and_then(|c| c.selection.cursor().map(str::to_owned)) {
                    self.open(&name, &mut fx);
                }
            }
            Action::GoUp => {
                if let Some(parent) = self.conn().and_then(|c| c.cwd.parent()) {
                    self.navigate(parent, Nav::Visit, &mut fx);
                }
            }
            Action::Back => self.history(Nav::Back, &mut fx),
            Action::Forward => self.history(Nav::Forward, &mut fx),
            Action::GoTo(path) => self.navigate(path, Nav::Visit, &mut fx),
            Action::Click { name, extend, toggle } => {
                self.ui.type_select.reset();
                if let Some(c) = self.conn_mut() {
                    if extend {
                        let rows = c.row_names().into_iter().map(str::to_owned).collect::<Vec<_>>();
                        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
                        c.selection.extend_to(&rows, &name, toggle);
                    } else if toggle {
                        c.selection.toggle(&name);
                    } else {
                        c.selection.only(&name);
                    }
                }
            }
            Action::Move { delta, extend } => self.with_rows(&mut fx, |sel, rows| sel.step(rows, delta, extend)),
            Action::End { last, extend } => self.with_rows(&mut fx, |sel, rows| sel.jump_to_end(rows, last, extend)),
            Action::SelectAll => self.with_rows(&mut fx, |sel, rows| {
                sel.select_all(rows);
                None
            }),
            Action::Type { text, now } => {
                let prefix = self.ui.type_select.push(&text, now).to_owned();
                self.with_rows(&mut fx, |sel, rows| {
                    let i = selection::find_prefix(rows, &prefix, sel.cursor())?;
                    sel.only(rows[i]);
                    Some(rows[i].to_owned())
                });
            }
            Action::Sort(key) => {
                self.sort = self.sort.toggled(key);
                let sort = self.sort;
                if let Some(c) = self.conn_mut() {
                    c.set_sort(sort);
                    if let Some(cur) = c.selection.cursor() {
                        fx.push(Effect::Reveal(cur.to_owned()));
                    }
                }
            }
            Action::ToggleHidden => {
                self.show_hidden_default = !self.show_hidden_default;
                let show = self.show_hidden_default;
                for c in self.conns.values_mut() {
                    if c.show_hidden != show {
                        c.toggle_hidden();
                    }
                }
            }
            Action::OpenFilter => {
                let text = self.ui.filter_edit.text.clone();
                if let Some(c) = self.conn_mut() {
                    c.set_filter(Filter::On(text));
                    self.ui.filter_edit.focused = true;
                }
            }
            Action::CloseFilter => {
                self.ui.filter_edit = TextEdit::new("");
                if let Some(c) = self.conn_mut() {
                    c.set_filter(Filter::Off);
                    if let Some(cur) = c.selection.cursor() {
                        fx.push(Effect::Reveal(cur.to_owned()));
                    }
                }
                fx.push(Effect::Blur);
            }
            Action::FilterChanged(text) => {
                if let Some(c) = self.conn_mut() {
                    c.set_filter(Filter::On(text));
                    fx.push(Effect::ScrollTop);
                }
            }
            Action::Escape => {
                if self.conn().is_some_and(|c| c.filter.is_on()) {
                    return self.act(Action::CloseFilter);
                }
                if let Some(c) = self.conn_mut() {
                    c.selection.clear();
                }
            }
            Action::QuickConnect => {}
        }
        fx
    }

    /// Maps a shortcut to its action (`now` is for type-to-select).
    pub fn shortcut(&mut self, sc: Shortcut, now: f64) -> Vec<Effect> {
        let action = match sc {
            Shortcut::Back => Action::Back,
            Shortcut::Forward => Action::Forward,
            Shortcut::Up => Action::GoUp,
            Shortcut::Open => Action::OpenSelection,
            Shortcut::SelectAll => Action::SelectAll,
            Shortcut::ToggleHidden => Action::ToggleHidden,
            Shortcut::Filter => Action::OpenFilter,
            Shortcut::Escape => Action::Escape,
            Shortcut::Move { delta, extend } => Action::Move { delta, extend },
            Shortcut::Page { down, extend } => Action::Move { delta: if down { keys::PAGE_ROWS } else { -keys::PAGE_ROWS }, extend },
            Shortcut::End { last, extend } => Action::End { last, extend },
            Shortcut::Type(text) => Action::Type { text, now },
        };
        self.act(action)
    }

    fn select_host(&mut self, host: HostId) {
        if self.hosts.get(&host).is_none() {
            return;
        }
        self.ui.filter_edit = TextEdit::new("");
        self.active = Some(host.clone());
        let live = self.conns.get(&host).is_some_and(|c| hosts::is_live(&c.status));
        if !live {
            let view = ConnView::connecting(host.clone(), self.show_hidden_default, self.sort);
            self.conns.insert(host.clone(), view);
            self.send(Command::Connect { host });
        }
    }

    fn open(&mut self, name: &str, fx: &mut Vec<Effect>) {
        let Some(c) = self.conn() else { return };
        let Some(e) = c.find(name) else { return };
        if e.kind.is_dir_like() {
            let path = c.cwd.join(name);
            self.navigate(path, Nav::Visit, fx);
        }
    }

    fn navigate(&mut self, path: RemotePath, how: Nav, fx: &mut Vec<Effect>) {
        let Some(c) = self.conn_mut() else { return };
        if !c.interactive() || !c.go(path, how) {
            return;
        }
        let (host, path) = (c.host.clone(), c.cwd.clone());
        self.ui.filter_edit = TextEdit::new("");
        self.ui.type_select.reset();
        fx.push(Effect::ScrollTop);
        fx.push(Effect::Blur);
        self.send(Command::List { host, path });
        if let Some(cur) = self.conn().and_then(|c| c.selection.cursor().map(str::to_owned)) {
            fx.push(Effect::Reveal(cur));
        }
    }

    fn history(&mut self, how: Nav, fx: &mut Vec<Effect>) {
        let Some(c) = self.conn_mut() else { return };
        if !c.interactive() {
            return;
        }
        let moved = match how {
            Nav::Back => c.back(),
            Nav::Forward => c.forward(),
            Nav::Visit => false,
        };
        if moved {
            let (host, path) = (c.host.clone(), c.cwd.clone());
            self.ui.filter_edit = TextEdit::new("");
            fx.push(Effect::ScrollTop);
            fx.push(Effect::Blur);
            self.send(Command::List { host, path });
            if let Some(cur) = self.conn().and_then(|c| c.selection.cursor().map(str::to_owned)) {
                fx.push(Effect::Reveal(cur));
            }
        }
    }

    /// Runs a selection change over the rows on screen; a returned name is revealed.
    fn with_rows(&mut self, fx: &mut Vec<Effect>, f: impl FnOnce(&mut Selection, &[&str]) -> Option<String>) {
        let Some(c) = self.conn_mut() else { return };
        let names: Vec<String> = c.row_names().into_iter().map(str::to_owned).collect();
        let rows: Vec<&str> = names.iter().map(String::as_str).collect();
        if let Some(name) = f(&mut c.selection, &rows) {
            fx.push(Effect::Reveal(name));
        }
    }
}
