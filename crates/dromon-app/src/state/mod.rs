//! App state (§4.7) and its updates. Everything here is plain data and functions of it: views
//! turn input into an [`Action`] and call [`App::act`]; the service's answers arrive as
//! [`ServiceEvent`]s through [`App::apply`]. Neither touches Rune, so both are unit-tested
//! directly (`state/tests.rs`).

pub mod conn;
pub mod history;
pub mod hosts;
pub mod keys;
pub mod log;
pub mod prefs;
pub mod selection;
pub mod service;
pub mod sheets;
pub mod transfers;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use dromon_core::{ConnStatus, HostId, HostSource, RemotePath, SortKey};
use rune::TextEdit;

pub use conn::{ConnView, Filter, ListError, Listing, Nav, Sort};
pub use hosts::{HostDirectory, Section};
pub use keys::Shortcut;
pub use prefs::{NoStore, Saved, Store};
pub use selection::{Selection, TypeSelect};
#[cfg(test)]
pub use service::Recorder;
pub use service::{Backend, Command, ServiceEvent, Silent, TransferCommand};
pub use sheets::{Sheet, Sheets};
pub use transfers::{DropHover, DropTarget, Tab, Transfers};

use dromon_transfer::{ConflictChoice, JobId};

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
    /// The open sheet, the prompts queued behind it, and notices.
    pub sheets: Sheets,
    /// "3 transfers in progress. Quit anyway?" is on screen ([`App::close_requested`]).
    pub confirm_quit: bool,
    /// The user chose Quit: the next close request goes through.
    pub quit_approved: bool,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            appearance: Appearance::System,
            sidebar_width: size::SIDEBAR,
            filter_edit: TextEdit::new(""),
            type_select: TypeSelect::default(),
            center_on_mount: None,
            sheets: Sheets::default(),
            confirm_quit: false,
            quit_approved: false,
        }
    }
}

/// Milliseconds since the epoch, from the system clock: when log lines arrived.
pub fn system_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
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
    /// The clock that stamps log lines (fixed in scenes and tests).
    pub wall_ms: fn() -> i64,
    /// What's remembered between launches, and where changes go ([`App::remember`]).
    pub saved: Saved,
    store: Box<dyn Store>,
    /// The transfer engine's jobs and the popover.
    pub transfers: Transfers,
    /// Where downloads go (§3.5: Downloads, or the folder set in Settings).
    pub downloads: PathBuf,
    inbox: VecDeque<ServiceEvent>,
}

/// What the user did, in the app's terms.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    SelectHost(HostId),
    /// Double-click or Return on a row: navigate into a folder, download a file.
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
    /// The open sheet's primary button: Connect, Trust and connect, Yes.
    SubmitPrompt,
    /// A yes/no sheet's No.
    DeclinePrompt,
    /// Cancel or Escape on the open sheet (a prompt is cancelled; the log closes).
    DismissSheet,
    /// Show the connection log of the host on screen.
    OpenLog,
    /// Close a connection and forget its remembered secrets.
    Disconnect(HostId),
    /// ⌘D or the context menu: download the selected rows (or the cursor's).
    Download,
    /// Files dropped or pasted from Finder/Explorer: upload them into the folder on screen, or
    /// into its subfolder `folder` (dropped on a folder row).
    Upload { paths: Vec<PathBuf>, folder: Option<String> },
    /// A transfer row's buttons: pause, resume, cancel, retry, clear finished.
    Transfer(TransferCommand),
    /// The conflict sheet's buttons, for the clash on screen.
    ResolveConflict(ConflictChoice),
    /// The conflict sheet's "Apply to all".
    ToggleApplyAll,
    /// The toolbar's transfers button.
    ToggleTransfers,
    TransfersTab(Tab),
    /// A click on a finished row: show the file in Finder/Explorer, or the folder it was
    /// uploaded to.
    RevealTransfer(JobId),
    /// A file drag from another app came onto a drop target (`over`) or left it; `count` is
    /// how many items it carries.
    DropHover { target: DropTarget, over: bool, count: usize },
    /// The quit sheet's Quit: close the window although transfers run.
    QuitAnyway,
    /// The quit sheet's Cancel (or Escape): keep going.
    CancelQuit,
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
    /// Show this local file in Finder/Explorer (selected in its folder).
    ShowInFolder(PathBuf),
    /// Close the window (and so quit): the user confirmed it.
    CloseWindow,
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
            wall_ms: system_ms,
            saved: Saved::default(),
            store: Box::new(NoStore),
            transfers: Transfers::default(),
            // `run` puts the user's real Downloads folder here; scenes and tests keep this.
            downloads: PathBuf::from("Downloads"),
            inbox: VecDeque::new(),
        }
    }

    /// Starts from settings remembered last time and saves later changes to `store`.
    pub fn remember(&mut self, saved: Saved, store: Box<dyn Store>) {
        if let Some(sort) = saved.sort {
            self.sort = sort;
        }
        if let Some(show) = saved.show_hidden {
            self.show_hidden_default = show;
        }
        if let Some(w) = saved.sidebar_width.filter(|w| w.is_finite()) {
            self.ui.sidebar_width = w.clamp(size::SIDEBAR_MIN, size::SIDEBAR_MAX);
        }
        self.saved = saved;
        self.store = store;
    }

    /// Hands the remembered settings to the store, after a change.
    fn save(&mut self) {
        self.store.save(&self.saved);
    }

    /// The sidebar's resize grip was let go.
    pub fn sidebar_resized(&mut self) {
        self.saved.sidebar_width = Some(self.ui.sidebar_width);
        self.save();
    }

    /// Remembers the folder on screen for `host`.
    fn remember_folder(&mut self, host: &HostId, path: &RemotePath) {
        if self.saved.folder(host.as_str()) != Some(path.as_str()) {
            self.saved.set_folder(host.as_str(), path.as_str());
            self.save();
        }
    }

    /// Replaces the store, returning the old one (quitting drops the real store, which writes
    /// what it still holds).
    pub fn take_store(&mut self) -> Box<dyn Store> {
        std::mem::replace(&mut self.store, Box::new(NoStore))
    }

    pub fn conn(&self) -> Option<&ConnView> {
        self.active.as_ref().and_then(|h| self.conns.get(h))
    }

    pub fn conn_mut(&mut self) -> Option<&mut ConnView> {
        let host = self.active.clone()?;
        self.conns.get_mut(&host)
    }

    /// Sends `cmd` and applies whatever the backend answers right away (traced like
    /// [`App::apply`]'s).
    fn send(&mut self, cmd: Command) {
        let events = self.backend.send(cmd);
        if service::tracing() {
            for ev in &events {
                eprintln!("dromon-trace: {}", service::trace_line(ev));
            }
        }
        self.inbox.extend(events);
        while let Some(ev) = self.inbox.pop_front() {
            self.apply_one(ev);
        }
    }

    /// Applies one event from the service. This is the integration point for the real
    /// service thread: call it on the UI thread (from `AppHandle::update_if` or `poll`) for
    /// each event that arrives. It never blocks and may send follow-up commands.
    ///
    /// With `DROMON_TRACE=1` each event is named on stderr ([`service::trace_line`]: kind and
    /// host, never what it carries), so a run without input tooling can be observed.
    pub fn apply(&mut self, ev: ServiceEvent) {
        if service::tracing() {
            eprintln!("dromon-trace: {}", service::trace_line(&ev));
        }
        self.inbox.push_back(ev);
        while let Some(ev) = self.inbox.pop_front() {
            self.apply_one(ev);
        }
    }

    fn apply_one(&mut self, ev: ServiceEvent) {
        match ev {
            ServiceEvent::Hosts(specs) => {
                // Discovery owns the SSH Config section; saved and recent hosts stay.
                let mut hosts: Vec<_> = specs.into_iter().filter(|h| h.source == HostSource::SshConfig).collect();
                hosts.extend(self.hosts.hosts.drain(..).filter(|h| h.source != HostSource::SshConfig));
                self.hosts.hosts = hosts;
            }
            ServiceEvent::Status { host, status } => {
                if matches!(status, ConnStatus::Ready | ConnStatus::Failed(_) | ConnStatus::Disconnected) {
                    self.ui.sheets.on_settled(&host);
                }
                if let Some(c) = self.conns.get_mut(&host) {
                    c.status = status;
                }
            }
            ServiceEvent::Opened { host, start } => {
                self.ui.sheets.on_settled(&host);
                // The folder used last time on this host, with home to fall back on.
                let remembered = self.saved.folder(host.as_str()).map(RemotePath::new).filter(|p| *p != start);
                let Some(c) = self.conns.get_mut(&host) else { return };
                c.status = ConnStatus::Ready;
                c.opened = true;
                c.fallback = remembered.is_some().then(|| start.clone());
                let path = remembered.unwrap_or(start);
                c.cwd = path.clone();
                c.listing = Listing::Loading;
                c.paging = conn::Paging::Idle;
                self.remember_folder(&host, &path);
                self.send(Command::List { host, path });
            }
            ServiceEvent::Listing { host, path, entries } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    if path == c.cwd {
                        c.fallback = None;
                    }
                    c.set_listing(&path, entries);
                    if c.filter.is_on() && self.active.as_ref() == Some(&host) {
                        c.set_filter(Filter::On(self.ui.filter_edit.text.clone()));
                    }
                }
            }
            ServiceEvent::ListingPage { host, path, entries, first, done } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    if path == c.cwd {
                        c.fallback = None;
                    }
                    if c.apply_page(&path, entries, first, done) && c.filter.is_on() && self.active.as_ref() == Some(&host) {
                        c.set_filter(Filter::On(self.ui.filter_edit.text.clone()));
                    }
                }
            }
            ServiceEvent::ListFailed { host, path, error } => {
                let Some(c) = self.conns.get_mut(&host) else { return };
                // The folder remembered from last time is gone or closed now: go home instead.
                if path == c.cwd
                    && let Some(home) = c.fallback.take()
                    && c.go(home.clone(), Nav::Replace)
                {
                    self.remember_folder(&host, &home);
                    self.send(Command::List { host, path: home });
                    return;
                }
                c.set_failed(&path, error.into());
            }
            ServiceEvent::Prompt(prompt) => self.ui.sheets.on_prompt(prompt),
            ServiceEvent::PromptClosed(id) => self.ui.sheets.on_closed(id),
            ServiceEvent::Log { host, line } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    c.log.push((self.wall_ms)(), line);
                }
            }
            ServiceEvent::Transfers(jobs) => self.transfers.set(jobs),
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
                self.saved.sort = Some(sort);
                self.save();
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
                self.saved.show_hidden = Some(show);
                self.save();
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
                if self.transfers.open {
                    self.transfers.open = false;
                    return fx;
                }
                if self.conn().is_some_and(|c| c.filter.is_on()) {
                    return self.act(Action::CloseFilter);
                }
                if let Some(c) = self.conn_mut() {
                    c.selection.clear();
                }
            }
            Action::QuickConnect => {}
            Action::SubmitPrompt => {
                if let Some((id, answer)) = self.ui.sheets.submit() {
                    self.send(Command::AnswerPrompt { id, answer });
                }
            }
            Action::DeclinePrompt => {
                if let Some((id, answer)) = self.ui.sheets.decline() {
                    self.send(Command::AnswerPrompt { id, answer });
                }
            }
            Action::DismissSheet => {
                if std::mem::take(&mut self.ui.confirm_quit) {
                    return fx;
                }
                if let Some((id, answer)) = self.ui.sheets.cancel() {
                    self.send(Command::AnswerPrompt { id, answer });
                }
            }
            Action::OpenLog => {
                if let Some(host) = self.active.clone() {
                    self.ui.sheets.open_log(host);
                }
            }
            Action::Disconnect(host) => {
                if self.conns.contains_key(&host) {
                    self.ui.sheets.forget_host(&host);
                    self.send(Command::Disconnect { host });
                }
            }
            Action::Download
            | Action::Upload { .. }
            | Action::Transfer(_)
            | Action::ResolveConflict(_)
            | Action::ToggleApplyAll
            | Action::ToggleTransfers
            | Action::TransfersTab(_)
            | Action::RevealTransfer(_)
            | Action::DropHover { .. } => self.transfer_action(action, &mut fx),
            Action::QuitAnyway => {
                self.ui.confirm_quit = false;
                self.ui.quit_approved = true;
                fx.push(Effect::CloseWindow);
            }
            Action::CancelQuit => self.ui.confirm_quit = false,
        }
        fx
    }

    /// The window's close button (⌘W, Alt+F4) was pressed: whether to close now. With
    /// transfers running it asks first (the quit sheet) and says no; Quit there approves it.
    /// macOS ⌘Q and logging out don't come through here.
    pub fn close_requested(&mut self) -> bool {
        if self.ui.quit_approved || self.transfers.active() == 0 {
            return true;
        }
        self.ui.confirm_quit = true;
        false
    }

    /// Maps a shortcut to its action (`now` is for type-to-select). The conflict sheet is
    /// modal like the prompt sheets: while it's up, the list's shortcuts do nothing.
    pub fn shortcut(&mut self, sc: Shortcut, now: f64) -> Vec<Effect> {
        if self.conflict_on_screen().is_some() || self.ui.confirm_quit {
            return Vec::new();
        }
        let action = match sc {
            Shortcut::Download => Action::Download,
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
            let mut view = ConnView::connecting(host.clone(), self.show_hidden_default, self.sort);
            // The log outlives a failed or closed connection, so "Show log" still explains it.
            if let Some(old) = self.conns.remove(&host) {
                view.log = old.log;
            }
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
        } else {
            let names = vec![name.to_owned()];
            self.download(&names);
        }
    }

    fn navigate(&mut self, path: RemotePath, how: Nav, fx: &mut Vec<Effect>) {
        self.navigate_selecting(path, how, None, fx);
    }

    /// [`App::navigate`], selecting the row `select` once the folder lists. Set before the
    /// listing is asked for, since a backend may answer at once.
    fn navigate_selecting(&mut self, path: RemotePath, how: Nav, select: Option<&str>, fx: &mut Vec<Effect>) {
        let Some(c) = self.conn_mut() else { return };
        if !c.interactive() || !c.go(path, how) {
            return;
        }
        if let Some(name) = select {
            c.reveal_after(name);
        }
        c.fallback = None;
        let (host, path) = (c.host.clone(), c.cwd.clone());
        self.ui.filter_edit = TextEdit::new("");
        self.ui.type_select.reset();
        fx.push(Effect::ScrollTop);
        fx.push(Effect::Blur);
        self.remember_folder(&host, &path);
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
            Nav::Visit | Nav::Replace => false,
        };
        if moved {
            c.fallback = None;
            let (host, path) = (c.host.clone(), c.cwd.clone());
            self.ui.filter_edit = TextEdit::new("");
            fx.push(Effect::ScrollTop);
            fx.push(Effect::Blur);
            self.remember_folder(&host, &path);
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
