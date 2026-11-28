//! App state (§4.7) and its updates. Everything here is plain data and functions of it: views
//! turn input into an [`Action`] and call [`App::act`]; the service's answers arrive as
//! [`ServiceEvent`]s through [`App::apply`]. Neither touches Rune, so both are unit-tested
//! directly (`state/tests.rs`).

mod browse;
pub mod clip;
pub mod common;
pub mod conn;
pub mod drag;
pub mod edit;
pub mod files;
pub mod focus;
pub mod frames;
pub mod fuzzy;
pub mod history;
pub mod hostbook;
pub mod hosts;
pub mod keys;
pub mod log;
pub mod menu;
pub mod ops;
pub mod page;
pub mod palette;
pub mod prefs;
pub mod preview;
pub mod properties;
pub mod quick;
pub mod redact;
pub mod refresh;
pub mod remote_edit;
pub mod report;
pub mod search;
pub mod selection;
pub mod service;
pub mod settings;
pub mod sheets;
mod shortcuts;
pub mod toast;
pub mod transfers;
pub mod update;
pub mod windows;

#[cfg(test)]
#[path = "props.rs"]
mod props;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use dromon_core::{ConnStatus, HostId, RemotePath, SortKey};
use rune::TextEdit;

pub use conn::{ConnView, Filter, ListError, Listing, Nav, Sort};
pub use drag::DragTarget as RowTarget;
pub use files::{FileAction, FileState};
pub use hostbook::HostAction;
pub use hosts::{HostDirectory, Section};
pub use keys::Shortcut;
pub use menu::MenuKind;
pub use ops::{OpView, Ops};
pub use page::{Page, SettingsSection};
pub use palette::PaletteAction;
pub use prefs::{NoStore, Saved, Store};
pub use preview::PreviewAction;
pub use search::{Scope, SearchAction};
pub use selection::{Selection, TypeSelect};
#[cfg(test)]
pub use service::Recorder;
pub use service::{
    Backend, Command, EditChoice, EditId, EditStage, FileOp, FileOpEvent, FileOpKind, Found, HostBookCommand,
    HostBookEvent, Live, LiveUnavailable, OpId, PreviewBytes, PreviewCommand, PreviewEvent, PreviewId, PreviewRange,
    RemoteEditCommand, RemoteEditEvent, SearchCommand, SearchEvent, SearchId, ServiceEvent, Silent, SizeCommand,
    SizeEvent, SizeId, SizeTotal, TransferCommand, WindowId,
};
pub use settings::{Density, Settings, SettingsChange, SettingsView};
pub use sheets::{Sheet, Sheets};
pub use transfers::{DropHover, DropTarget, Tab, Transfers};
pub use windows::{News, NoPeers, Peers};

use common::Common;

use dromon_transfer::{ConflictChoice, JobId};

use crate::theme::{Access, Appearance, size};

/// Window-level UI state that isn't about any one connection.
pub struct UiState {
    pub appearance: Appearance,
    /// Display preferences forced on over the system's (scenes and tests; later a setting):
    /// the view ORs them with what the OS asks for (`views::access`).
    pub access: Access,
    pub sidebar_width: f32,
    /// The ⌘F field's text model; its query is mirrored into the active `ConnView::filter`, or
    /// searched for below the folder in the Subfolders scope (`search.rs`).
    pub filter_edit: TextEdit,
    /// Search in subfolders: the field's scope, and the search and its results (`search.rs`).
    pub search: search::Search,
    pub type_select: TypeSelect,
    /// A row to bring to the middle of the list once it mounts (a scene's scroll position).
    pub center_on_mount: Option<String>,
    /// The open sheet, the prompts queued behind it, and notices.
    pub sheets: Sheets,
    /// "3 transfers in progress. Quit anyway?" is on screen ([`App::close_requested`]).
    pub confirm_quit: bool,
    /// The user chose Quit: the next close request goes through.
    pub quit_approved: bool,
    /// The quick-connect popover (`quick.rs`).
    pub quick: quick::QuickConnect,
    /// The ⌘K palette (`palette.rs`).
    pub palette: palette::Palette,
    /// The folder view, or the Settings page in its place (`page.rs`).
    pub page: Page,
    pub settings_page: page::SettingsPage,
    /// What screen readers are told as it happens (`focus.rs`, §6.9).
    pub announcer: focus::Announcer,
    /// The quick preview panel (Space, `preview.rs`).
    pub preview: preview::Preview,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            appearance: Appearance::System,
            access: Access::default(),
            sidebar_width: size::SIDEBAR,
            filter_edit: TextEdit::new(""),
            search: search::Search::default(),
            type_select: TypeSelect::default(),
            center_on_mount: None,
            sheets: Sheets::default(),
            confirm_quit: false,
            quit_approved: false,
            quick: quick::QuickConnect::default(),
            palette: palette::Palette::default(),
            page: Page::Files,
            settings_page: page::SettingsPage::default(),
            announcer: focus::Announcer::default(),
            preview: preview::Preview::default(),
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
    /// Show dotfiles in hosts opened next: Settings' default, switched for the session by ⌘⇧.
    pub show_hidden_default: bool,
    /// Theme, density, download folder, transfers at once, hidden files (`settings.rs`, §8).
    pub settings: Settings,
    /// Sort remembered globally (§12).
    pub sort: Sort,
    pub backend: Box<dyn Backend>,
    /// The clock that stamps log lines (fixed in scenes and tests).
    pub wall_ms: fn() -> i64,
    /// Which main window this is (`windows.rs`).
    pub window: WindowId,
    /// What every window shares: the clip, what's remembered between launches and where
    /// changes go ([`App::remember`]), the other windows.
    common: Common,
    /// The transfer engine's jobs and the popover.
    pub transfers: Transfers,
    /// New folder, rename and delete operations and how each is going.
    pub ops: Ops,
    /// The name field, the clip, a row drag, the drawn menu and the toasts (`files.rs`).
    pub files: FileState,
    /// The OS Downloads folder: where downloads go unless Settings chose another (§3.5,
    /// [`App::download_folder`]).
    pub downloads: PathBuf,
    inbox: VecDeque<ServiceEvent>,
    /// The window's state as the service polls by, and refresh animation (`refresh.rs`).
    pub poll: refresh::PollUi,
    /// A newer version, and installing it (`update.rs`).
    pub update: update::Update,
    /// The files being edited in a local editor (`remote_edit.rs`).
    pub edits: remote_edit::Edits,
    /// "Report a Bug…": the last events and actions, and the sheet (`report.rs`).
    pub report: report::Report,
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
    Click {
        name: String,
        extend: bool,
        toggle: bool,
    },
    Move {
        delta: isize,
        extend: bool,
    },
    End {
        last: bool,
        extend: bool,
    },
    SelectAll,
    /// Type-to-select: `text` typed at `now` (seconds, any monotonic clock).
    Type {
        text: String,
        now: f64,
    },
    Sort(SortKey),
    ToggleHidden,
    OpenFilter,
    CloseFilter,
    /// The filter field's text changed to this.
    FilterChanged(String),
    /// Search in subfolders: the filter field's scope and typing, ⌘⇧F (`search.rs`).
    Search(SearchAction),
    Escape,
    /// Open the quick-connect popover (the sidebar's +, the welcome button, ⌘⇧K).
    QuickConnect,
    /// Quick connect's fields and buttons, the host menus, Add to SSH config (`hostbook.rs`).
    HostBook(HostAction),
    /// The open sheet's primary button: Connect, Trust and connect, Yes.
    SubmitPrompt,
    /// A yes/no sheet's No.
    DeclinePrompt,
    /// Cancel or Escape on the open sheet (a prompt is cancelled; the log closes).
    DismissSheet,
    /// Show the connection log of the host on screen.
    OpenLog,
    /// The reconnecting banner's Sign in: show `host`'s waiting prompt in this window (§3.9).
    SignIn(HostId),
    /// Close a connection and forget its remembered secrets.
    Disconnect(HostId),
    /// The log's "Verbose logging" switch: ssh with `-v` or without, reconnecting a live host.
    Verbose {
        host: HostId,
        on: bool,
    },
    /// ⌘D or the context menu: download the selected rows (or the cursor's).
    Download,
    /// Files dropped or pasted from Finder/Explorer: upload them into the folder on screen, or
    /// into its subfolder `folder` (dropped on a folder row).
    Upload {
        paths: Vec<PathBuf>,
        folder: Option<String>,
    },
    /// The toolbar's add menu, ⌘U, the palette: ask for the native file picker
    /// ([`Effect::PickUpload`]); what it picks goes to [`Action::Upload`].
    PickUpload,
    /// The toolbar's add menu, the palette: ask for the native folder picker
    /// ([`Effect::PickUploadFolder`]); the folder picked goes to [`Action::Upload`].
    PickUploadFolder,
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
    DropHover {
        target: DropTarget,
        over: bool,
        count: usize,
    },
    /// The quit sheet's Quit: close the window although transfers run.
    QuitAnyway,
    /// The quit sheet's Cancel (or Escape): keep going.
    CancelQuit,
    /// New folder, rename, delete, copy path, the clip, row drags and context menus.
    File(FileAction),
    /// The ⌘K palette (`palette.rs`).
    Palette(PaletteAction),
    /// Show the Settings page in this window, or keep it (⌘,, the app menu, the palette, the
    /// sidebar's gear; `page.rs`).
    OpenSettings,
    /// The page's back button, Escape: back to the folder view.
    LeaveSettings,
    /// A section picked in the page's list.
    SettingsSection(SettingsSection),
    /// A control on the Settings page (`settings.rs`).
    Setting(SettingsChange),
    /// Space, the quick preview panel's buttons (`preview.rs`).
    Preview(PreviewAction),
    /// ⌘N, the File menu: another main window, on the welcome state (`windows.rs`).
    NewWindow,
    /// ⌘W, the File menu: close this window (the last one asks first while transfers run).
    CloseWindow,
    /// The update toast's, the Settings page's and the update sheet's buttons (`update.rs`).
    Update(update::UpdateAction),
    /// "Open in Terminal" (the menus, the palette, ⌘⇧T): ssh in the user's terminal app, in the
    /// row `Some(name)` when it's a folder, else in the folder on screen (`menu.rs`).
    OpenTerminal(Option<String>),
    /// The terminal app couldn't be opened, and why (from the launcher's thread): a toast.
    TerminalFailed(String),
    /// The terminal apps found on this machine (from the thread that looked), for Settings.
    TerminalsFound(Vec<settings::Terminal>),
    /// Edit in a local editor, Stop editing, the edit conflict sheet, Retry (`remote_edit.rs`).
    RemoteEdit(remote_edit::RemoteEditAction),
    /// "Report a Bug…", an error's Report, and the report sheet's buttons (`report.rs`).
    Report(report::ReportAction),
}

impl Action {
    /// The variant's name and nothing it carries: what a bug report lists of recent actions.
    pub fn kind(&self) -> &'static str {
        // One arm per variant, so a new one can't be left out (`{ .. }` matches every shape).
        macro_rules! names {
            ($($v:ident)*) => { match self { $(Action::$v { .. } => stringify!($v),)* } };
        }
        names!(SelectHost Open OpenSelection GoUp Back Forward GoTo Click Move End SelectAll Type Sort
            ToggleHidden OpenFilter CloseFilter FilterChanged Search Escape QuickConnect HostBook SubmitPrompt
            DeclinePrompt DismissSheet OpenLog SignIn Disconnect Verbose Download Upload PickUpload PickUploadFolder
            Transfer ResolveConflict ToggleApplyAll ToggleTransfers TransfersTab RevealTransfer DropHover QuitAnyway
            CancelQuit File Palette OpenSettings LeaveSettings SettingsSection Setting Preview NewWindow CloseWindow
            Update OpenTerminal TerminalFailed TerminalsFound RemoteEdit Report)
    }
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
    /// Put this text on the OS clipboard.
    Copy(String),
    /// Read the OS clipboard, then paste the clip if it still holds it (`FileAction::Paste`).
    ReadClipboard { as_move: bool },
    /// Open the context menu of `kind` anchored at its row (a row not on screen, and the empty
    /// area: the list's top-left): the view knows where things are.
    ContextMenu(MenuKind),
    /// Open the native multi-file picker, titled for `folder` (the folder on screen); the files
    /// picked upload into it (`Action::Upload` with no folder).
    PickUpload { folder: String },
    /// Open the native folder picker, titled for `folder` (the folder on screen); the folder
    /// picked uploads into it, with all it holds (`Action::Upload` with no folder).
    PickUploadFolder { folder: String },
    /// The preview panel opened: the keyboard goes into it.
    FocusPreview,
    /// The preview panel closed: the keyboard, if it was in it, goes back to the list.
    FocusList,
    /// Open this https link in the browser (a release's page).
    OpenUrl(String),
    /// Open this local file in the app the system has for its type (a copy being edited).
    OpenFile(PathBuf),
    /// Quit the app (every window), starting it again once it has exited when `relaunch`: an
    /// update is in place (`update.rs`).
    Quit { relaunch: bool },
    /// Open `terminal` running ssh to `host` in `folder`, off the UI thread (`crate::terminal`);
    /// a failure comes back to `window` as [`Action::TerminalFailed`].
    OpenTerminal { window: WindowId, terminal: settings::Terminal, host: dromon_core::HostSpec, folder: RemotePath },
    /// Look for the terminal apps on this machine off the UI thread; they come back to `window`
    /// as [`Action::TerminalsFound`].
    DetectTerminals { window: WindowId },
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
            settings: Settings::default(),
            sort: Sort::default(),
            backend,
            wall_ms: system_ms,
            window: WindowId::FIRST,
            common: Common::default(),
            transfers: Transfers::default(),
            ops: Ops::default(),
            files: FileState::default(),
            // `run` puts the user's real Downloads folder here; scenes and tests keep this.
            downloads: PathBuf::from("Downloads"),
            inbox: VecDeque::new(),
            poll: refresh::PollUi::default(),
            update: update::Update::default(),
            edits: remote_edit::Edits::default(),
            report: report::Report::default(),
        }
    }

    pub fn conn(&self) -> Option<&ConnView> {
        self.active.as_ref().and_then(|h| self.conns.get(h))
    }

    pub fn conn_mut(&mut self) -> Option<&mut ConnView> {
        let host = self.active.clone()?;
        self.conns.get_mut(&host)
    }

    /// Sends `cmd` and applies whatever the backend answers right away (traced like
    /// [`App::apply`]'s), after what came meanwhile from other windows' commands.
    fn send(&mut self, cmd: Command) {
        let mut events = self.backend.pending();
        events.extend(self.backend.send(cmd));
        self.take(events);
    }

    /// Applies what another window's commands brought this one (a synchronous backend's
    /// `pending`), as a window does when it's woken for it.
    pub fn catch_up(&mut self) {
        let events = self.backend.pending();
        self.take(events);
    }

    fn take(&mut self, events: Vec<ServiceEvent>) {
        if service::tracing() {
            for ev in &events {
                eprintln!("dromon-trace: {}", service::trace_line(ev));
            }
        }
        self.inbox.extend(events);
        while let Some(ev) = self.inbox.pop_front() {
            self.apply_one(ev);
            self.sync();
        }
    }

    /// After every action and event: what follows the list catches up with it (`files.rs`,
    /// `preview.rs`).
    fn sync(&mut self) {
        self.sync_files();
        self.sync_preview();
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
            self.sync();
        }
    }

    fn apply_one(&mut self, ev: ServiceEvent) {
        // A bug report lists the last events, and a recording keeps what servers answer (`record.rs`).
        self.heard(&ev);
        match ev {
            ServiceEvent::FileOp(ev) => self.ops.event(ev),
            ServiceEvent::FolderSize(ev) => self.size_event(ev),
            ServiceEvent::HostBook(ev) => self.host_event(ev),
            // Discovery owns the SSH Config section; saved and recent hosts stay.
            ServiceEvent::Hosts(specs) => self.set_config_hosts(specs),
            ServiceEvent::ListFailed { host, path, error } => {
                // The folder on screen is gone: up to the nearest one that exists (§6.8).
                if self.vanished(&host, &path, &error) {
                    return;
                }
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
                self.vanish_landed(&host, &path);
            }
            ServiceEvent::Listing { host, path, entries } | ServiceEvent::Refreshed { host, path, entries } => {
                let query = self.folder_query();
                if let Some(c) = self.conns.get_mut(&host) {
                    if path == c.cwd {
                        c.fallback = None;
                    }
                    let seq = c.fresh.seq;
                    c.set_listing(&path, entries);
                    if c.filter.is_on() && self.active.as_ref() == Some(&host) {
                        c.set_filter(Filter::On(query));
                    }
                    if c.fresh.seq != seq {
                        self.refreshed();
                    }
                }
                self.vanish_landed(&host, &path);
            }
            ServiceEvent::ListingPage { host, path, entries, first, done } => {
                let query = self.folder_query();
                if let Some(c) = self.conns.get_mut(&host) {
                    if path == c.cwd {
                        c.fallback = None;
                    }
                    let seq = c.fresh.seq;
                    if c.apply_page(&path, entries, first, done)
                        && c.filter.is_on()
                        && self.active.as_ref() == Some(&host)
                    {
                        c.set_filter(Filter::On(query));
                    }
                    if c.fresh.seq != seq {
                        self.refreshed();
                    }
                }
                self.vanish_landed(&host, &path);
            }
            ServiceEvent::Live { host, live } => {
                self.conns.entry(host).and_modify(|c| c.live = live);
            }
            ServiceEvent::Log { host, line } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    c.log.push((self.wall_ms)(), line);
                }
            }
            ServiceEvent::Opened { host, start } => self.opened(host, start),
            ServiceEvent::Preview(ev) => self.preview_event(ev),
            ServiceEvent::Prompt(prompt) => self.ui.sheets.on_prompt(prompt),
            ServiceEvent::PromptClosed(id) => self.ui.sheets.on_closed(id),
            ServiceEvent::RemoteEdit(ev) => self.remote_edit_event(ev),
            ServiceEvent::Search(ev) => self.search_event(ev),
            ServiceEvent::Status { host, status } => {
                if matches!(status, ConnStatus::Ready | ConnStatus::Failed(_) | ConnStatus::Disconnected) {
                    self.ui.sheets.on_settled(&host);
                }
                if let Some(c) = self.conns.get_mut(&host) {
                    c.status = status;
                }
            }
            ServiceEvent::Transfers(jobs) => {
                // Every window sees every snapshot; the badge and notifications come from the
                // leading one alone (`state/windows.rs`).
                self.transfers.watch.lead(self.leads());
                self.log_failures(&jobs);
                // A folder on screen that a job just finished changing is listed again (until
                // polling, §4.5, the only way to see what a paste, move or upload made).
                for (host, path) in self.transfers.set(jobs) {
                    if self.conns.get(&host).is_some_and(|c| c.interactive() && c.cwd == path) {
                        self.send(Command::List { host, path });
                    }
                }
            }
            ServiceEvent::Update(ev) => self.update_event(ev),
            ServiceEvent::Verbose { host, on } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    c.log.verbose = on;
                }
            }
        }
    }

    /// `host` is up in this window, its home `start`: it lists the folder used there last time,
    /// with home to fall back on.
    fn opened(&mut self, host: HostId, start: RemotePath) {
        self.ui.sheets.on_settled(&host);
        let remembered = self.saved().folder(host.as_str()).map(RemotePath::new).filter(|p| *p != start);
        let Some(c) = self.conns.get_mut(&host) else { return };
        c.status = ConnStatus::Ready;
        c.opened = true;
        c.home = Some(start.clone());
        c.fallback = remembered.is_some().then(|| start.clone());
        let path = remembered.unwrap_or(start);
        c.cwd = path.clone();
        c.listing = Listing::Loading;
        c.paging = conn::Paging::Idle;
        self.remember_folder(&host, &path);
        self.send(Command::List { host, path });
    }

    /// Carries out `action`; returns what the view should do next.
    pub fn act(&mut self, action: Action) -> Vec<Effect> {
        // A bug report lists the last actions, and a recording takes their steps (`record.rs`).
        let seen = self.observe(&action);
        let fx = self.act_one(action);
        self.sync();
        self.observed(seen);
        fx
    }

    fn act_one(&mut self, action: Action) -> Vec<Effect> {
        let mut fx = Vec::new();
        // While a search's results are on screen, the list's actions are theirs (`search.rs`).
        let Some(action) = self.results_action(action, &mut fx) else { return fx };
        match action {
            Action::SelectHost(host) => self.select_host(host),
            Action::Open(_)
            | Action::OpenSelection
            | Action::GoUp
            | Action::Back
            | Action::Forward
            | Action::GoTo(_)
            | Action::Click { .. }
            | Action::Move { .. }
            | Action::End { .. }
            | Action::SelectAll
            | Action::Type { .. }
            | Action::Sort(_)
            | Action::ToggleHidden
            | Action::OpenFilter
            | Action::CloseFilter
            | Action::FilterChanged(_)
            | Action::Escape => self.browse_action(action, &mut fx),
            Action::Search(a) => self.search_action(a, &mut fx),
            Action::QuickConnect => self.open_quick(),
            Action::HostBook(a) => self.host_action(a, &mut fx),
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
            Action::DismissSheet => self.dismiss_sheet(&mut fx),
            Action::OpenLog => {
                if let Some(host) = self.active.clone() {
                    self.ui.sheets.open_log(host);
                }
            }
            Action::SignIn(host) => {
                // A prompt this window already has comes to the front; else the service sends
                // the host's open prompts here.
                if !self.ui.sheets.bring_forward(&host) && self.conns.contains_key(&host) {
                    self.send(Command::ShowPrompts { host });
                }
            }
            Action::Disconnect(host) => {
                if self.conns.contains_key(&host) {
                    self.ui.sheets.forget_host(&host);
                    self.send(Command::Disconnect { host });
                }
            }
            Action::Verbose { host, on } => {
                if let Some(c) = self.conns.get_mut(&host) {
                    c.log.verbose = on;
                    self.send(Command::Verbose { host, on });
                }
            }
            Action::Download
            | Action::Upload { .. }
            | Action::PickUpload
            | Action::PickUploadFolder
            | Action::Transfer(_)
            | Action::ResolveConflict(_)
            | Action::ToggleApplyAll
            | Action::ToggleTransfers
            | Action::TransfersTab(_)
            | Action::RevealTransfer(_)
            | Action::DropHover { .. } => self.transfer_action(action, &mut fx),
            Action::QuitAnyway => self.quit_anyway(&mut fx),
            Action::CancelQuit => self.ui.confirm_quit = false,
            Action::File(a) => self.file_action(a, &mut fx),
            Action::Palette(a) => self.palette_action(a, &mut fx),
            Action::OpenSettings => self.open_settings(),
            Action::LeaveSettings => self.leave_settings(),
            Action::SettingsSection(section) => self.show_section(section, &mut fx),
            Action::Setting(change) => self.change_setting(change),
            Action::Preview(a) => self.preview_action(a, &mut fx),
            Action::NewWindow => self.new_window(),
            Action::CloseWindow => self.close_window(&mut fx),
            Action::Update(a) => self.update_action(a, &mut fx),
            Action::OpenTerminal(row) => self.open_terminal(row, &mut fx),
            Action::TerminalFailed(why) => self.terminal_failed(&why),
            Action::TerminalsFound(found) => self.ui.settings_page.terminals = found,
            Action::RemoteEdit(a) => self.remote_edit_action(a, &mut fx),
            Action::Report(a) => self.report_action(a, &mut fx),
        }
        fx
    }

    /// Cancel or Escape on the sheet on screen, in the order `views::sheets::layer` stacks them:
    /// quitting's or updating's question, then a prompt (cancelled) or another sheet of
    /// [`Sheets`], then the bug report.
    fn dismiss_sheet(&mut self, fx: &mut Vec<Effect>) {
        if std::mem::take(&mut self.ui.confirm_quit) {
            return;
        }
        if self.update.confirming() {
            return self.update_action(update::UpdateAction::Cancel, fx);
        }
        if self.ui.sheets.is_open() {
            if let Some((id, answer)) = self.ui.sheets.cancel() {
                self.send(Command::AnswerPrompt { id, answer });
            }
            return;
        }
        self.report_action(report::ReportAction::Cancel, fx);
    }

    /// The window's close button (⌘W, Alt+F4) was pressed: whether to close now. Closing the last
    /// window quits, so then, with transfers running or edits not on the server yet, it asks first
    /// (the quit sheet) and says no; Quit there approves it. macOS ⌘Q and logging out don't come here.
    pub fn close_requested(&mut self) -> bool {
        if self.ui.quit_approved || self.transfers.active() + self.edits.unsaved() == 0 || self.windows_open() > 1 {
            return true;
        }
        self.ui.confirm_quit = true;
        false
    }

    /// The quit sheet's Quit: closing is approved, and the window closes (quitting the app).
    fn quit_anyway(&mut self, fx: &mut Vec<Effect>) {
        self.ui.confirm_quit = false;
        self.ui.quit_approved = true;
        fx.push(Effect::CloseWindow);
    }

    fn select_host(&mut self, host: HostId) {
        if self.hosts.get(&host).is_none() {
            return;
        }
        // Picking a host leaves the Settings page for its folder view.
        self.leave_settings();
        self.remember_host(&host);
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
}
