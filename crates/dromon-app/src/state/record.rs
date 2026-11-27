//! Record steps (the palette's "Record steps for a bug report…", the report sheet's Record
//! Steps…, Help › Record Steps…): while a window records, each action the user takes
//! ([`App::act`]) is kept as a [`Step`], and what the servers answer ([`App::apply`]) as each
//! host's [`Server`]: where its sessions start, how signing in went (the kinds of question only)
//! and the first listing of every folder listed. Stop (the sidebar's pill), 50 steps or 15
//! minutes end it and open the bug report sheet with it (`report.rs`), which turns it into a
//! fixture (`record_fixture.rs`) and a script (`record_script.rs`) through the report's one
//! [`Redactor`], so both name everything by the same stand-ins.
//!
//! Nothing here touches the disk, a clock (the window's `wall_ms` times the limit) or the prompt
//! sheet's field: what the user types to sign in is never read. The replay types a placeholder
//! the fixture accepts, or a wrong one where the server refused. A recording lives in the
//! window's memory and goes with it.
//!
//! An action that another runs counts once: what the palette or a context menu runs is the step
//! (they only choose it), while what an action runs itself (Escape closing the filter) is part of
//! that action's step.

use std::collections::BTreeMap;

use dromon_core::{ConnStatus, Entry, Error, HostId, Prompt, PromptKind, RemotePath, SortKey};

use super::ReportAction;
use crate::state::redact::Redactor;
use crate::state::{Action, App, FileAction, ListError, Listing, MenuKind, PreviewAction, ServiceEvent, Sheet, Sort};

#[path = "record_fixture.rs"]
mod fixture;
#[path = "record_script.rs"]
mod script;

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;

/// The most steps a recording takes; then it stops and the sheet opens.
pub const MAX_STEPS: usize = 50;
/// The longest a recording runs, in milliseconds of the window's clock.
pub const MAX_MS: i64 = 15 * 60 * 1000;

/// A row of the folder on screen, as a step names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// A folder, or a link to one.
    pub dir: bool,
}

/// Where the window was: the host on screen and, once its session opened, its folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub host: HostId,
    pub folder: Option<RemotePath>,
}

/// How a prompt was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Password,
    Passphrase,
    Code,
    /// Trust and connect, Yes.
    Yes,
}

/// A question a server asked while signing in, by kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    HostKey { key_type: Option<String> },
    Password,
    Passphrase,
    Code,
    Confirm,
    Notice,
}

/// One thing the user did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Connect(HostId),
    /// A prompt's primary button; `wrong` once the server asked again.
    Answer {
        host: HostId,
        kind: Answer,
        wrong: bool,
    },
    Decline(HostId),
    /// Cancel or Escape on the sheet on screen (a prompt's, `Some` its host).
    Dismiss(Option<HostId>),
    Open(Row),
    Up,
    Back,
    Forward,
    GoTo(RemotePath),
    Click {
        row: Row,
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
    /// Type-to-select, and the row it landed on.
    TypeSelect(Option<Row>),
    Sort(SortKey),
    Hidden,
    Escape,
    Refresh,
    NewFolder,
    Rename(Row),
    /// The name field's Return, with what it held.
    Named {
        name: String,
        dir: bool,
    },
    EditCancel,
    /// Delete on the selection (the row it's asked from).
    Delete(Row),
    /// The delete sheet's Delete: what it named.
    ConfirmDelete(Vec<String>),
    Properties(Row),
    /// Space on a row, or the panel closing (`None`).
    Preview(Option<Row>),
    Download,
    OpenLog,
    Disconnect(HostId),
    Transfers,
    /// What this version doesn't replay, in words.
    NotReplayed(&'static str),
}

/// A step, where it was taken and where the window was after it.
#[derive(Clone, Debug)]
pub struct Taken {
    pub step: Step,
    pub at: Option<Place>,
    pub after: Option<Place>,
}

/// What a folder listed the first time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Folder {
    Listed(Vec<Entry>),
    Denied,
    Missing,
    Lost,
}

/// Where a host's sign-in stands after the user's last move on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pending {
    #[default]
    None,
    /// Answered (the step's index); `refused` answers before it in a row.
    Answered {
        step: usize,
        refused: u32,
    },
    Cancelled,
}

/// What one host's server answered.
#[derive(Clone, Debug, Default)]
pub struct Server {
    pub home: Option<RemotePath>,
    /// The questions of the first sign-in, in order.
    pub auth: Vec<Ask>,
    /// The first sign-in is over (up or failed).
    settled: bool,
    pub opened: bool,
    /// It failed for a reason other than the answers.
    pub failed: bool,
    pending: Pending,
    pub folders: BTreeMap<RemotePath, Folder>,
    /// A listing's pages so far.
    streaming: BTreeMap<RemotePath, Vec<Entry>>,
}

impl Server {
    fn listed(&mut self, path: &RemotePath, entries: &[Entry]) {
        self.folders.entry(path.clone()).or_insert_with(|| Folder::Listed(entries.to_vec()));
    }

    fn page(&mut self, path: &RemotePath, entries: &[Entry], first: bool, done: bool) {
        if self.folders.contains_key(path) {
            return;
        }
        let pages = self.streaming.entry(path.clone()).or_default();
        if first {
            pages.clear();
        }
        pages.extend_from_slice(entries);
        if done {
            let all = self.streaming.remove(path).unwrap_or_default();
            self.folders.insert(path.clone(), Folder::Listed(all));
        }
    }

    fn unlisted(&mut self, path: &RemotePath, error: &Error) {
        let folder = match error {
            Error::PermissionDenied(_) => Folder::Denied,
            Error::ConnectionLost => Folder::Lost,
            _ => Folder::Missing,
        };
        self.folders.entry(path.clone()).or_insert(folder);
    }
}

/// A window's recording.
#[derive(Debug)]
pub struct Recording {
    started_ms: i64,
    steps: Vec<Taken>,
    servers: BTreeMap<HostId, Server>,
    /// Hosts in the order the recording met them.
    order: Vec<HostId>,
    /// The sort and hidden files when it started.
    sort: Sort,
    hidden: bool,
    /// The host on screen, and its folder, when it started: the replay goes there first.
    start: Option<Place>,
    /// Whether the actions running now are inside one that lets what it runs count (the
    /// palette, a context menu), one level each.
    nesting: Vec<bool>,
}

/// What [`App::observe`] saw, for [`App::observed`].
pub enum Observed {
    /// Not recording.
    Off,
    /// Recording: the step this action made, if it made one.
    Frame(Option<usize>),
}

/// Actions that only choose another, which is the step.
fn chooses(action: &Action) -> bool {
    matches!(
        action,
        Action::Palette(_)
            | Action::File(FileAction::OpenMenu { .. } | FileAction::CloseMenu | FileAction::Pick { .. })
    )
}

impl Recording {
    /// A recording of `app` from now: what it already shows of its hosts.
    pub fn start(app: &App) -> Recording {
        let mut rec = Recording {
            started_ms: (app.wall_ms)(),
            steps: Vec::new(),
            servers: BTreeMap::new(),
            order: Vec::new(),
            sort: app.sort,
            hidden: app.show_hidden_default,
            start: app.place().filter(|p| p.folder.is_some()),
            nesting: Vec::new(),
        };
        if let Some(host) = rec.start.as_ref().map(|p| p.host.clone()) {
            rec.server(&host);
        }
        for (host, c) in app.conns.iter().filter(|(_, c)| c.opened) {
            let s = rec.server(host);
            s.home.clone_from(&c.home);
            s.opened = true;
            s.settled = true;
            let folder = match &c.listing {
                Listing::Ready(entries) => Folder::Listed(entries.clone()),
                Listing::Failed(ListError::PermissionDenied) => Folder::Denied,
                Listing::Failed(_) => Folder::Missing,
                Listing::Loading => continue,
            };
            s.folders.insert(c.cwd.clone(), folder);
        }
        rec
    }

    /// How many steps it has.
    pub fn taken(&self) -> usize {
        self.steps.len()
    }

    /// It's taken all it may, at `now_ms`.
    fn full(&self, now_ms: i64) -> bool {
        self.steps.len() >= MAX_STEPS || now_ms - self.started_ms >= MAX_MS
    }

    fn server(&mut self, host: &HostId) -> &mut Server {
        if !self.order.contains(host) {
            self.order.push(host.clone());
        }
        self.servers.entry(host.clone()).or_default()
    }

    /// Adds `step`, or folds it into the last one (more letters typed to select, the same
    /// thing that isn't replayed again); its index.
    fn push(&mut self, step: Step, at: Option<Place>) -> usize {
        if let Some(last) = self.steps.last() {
            let again = match (&last.step, &step) {
                (Step::TypeSelect(_), Step::TypeSelect(_)) => true,
                (Step::NotReplayed(a), Step::NotReplayed(b)) => a == b,
                _ => false,
            };
            if again {
                return self.steps.len() - 1;
            }
        }
        let i = self.steps.len();
        match &step {
            Step::Answer { host, .. } => {
                let s = self.server(host);
                let refused = match s.pending {
                    Pending::Answered { refused, .. } => refused,
                    _ => 0,
                };
                s.pending = Pending::Answered { step: i, refused };
            }
            Step::Decline(host) | Step::Dismiss(Some(host)) => self.server(host).pending = Pending::Cancelled,
            Step::Connect(host) | Step::Disconnect(host) => {
                self.server(host);
            }
            _ => {}
        }
        if let Some(p) = &at {
            self.server(&p.host);
        }
        self.steps.push(Taken { step, at, after: None });
        i
    }

    /// The answer at step `i` was refused.
    fn refused(&mut self, i: usize) {
        if let Some(Step::Answer { wrong, .. }) = self.steps.get_mut(i).map(|t| &mut t.step) {
            *wrong = true;
        }
    }

    /// ssh asked something: the first sign-in's questions, and a question asked again says the
    /// answer before it was refused.
    fn asked(&mut self, p: &Prompt) {
        let s = self.server(&p.host);
        if p.error.is_some() {
            if let Pending::Answered { step, refused } = s.pending {
                s.pending = Pending::Answered { step, refused: refused + 1 };
                self.refused(step);
            }
            return;
        }
        if let Pending::Answered { step, .. } = s.pending {
            s.pending = Pending::Answered { step, refused: 0 };
        }
        if s.settled {
            return;
        }
        s.auth.push(match &p.kind {
            PromptKind::HostKey { key_type, .. } => Ask::HostKey { key_type: key_type.clone() },
            PromptKind::Password => Ask::Password,
            PromptKind::Passphrase { .. } => Ask::Passphrase,
            PromptKind::Other => Ask::Code,
            PromptKind::Confirm => Ask::Confirm,
            PromptKind::Notice => Ask::Notice,
        });
    }

    /// `host` couldn't connect: after a third refused answer that's the answers' doing (the
    /// fixture refuses them the same way), after a cancel the user's; else the server's.
    fn failed(&mut self, host: &HostId) {
        let s = self.server(host);
        s.settled = true;
        match s.pending {
            Pending::Answered { step, refused } if refused >= 2 => {
                s.pending = Pending::None;
                self.refused(step);
            }
            Pending::Cancelled => s.pending = Pending::None,
            _ if !s.opened => s.failed = true,
            _ => {}
        }
    }

    /// What a server answered.
    fn hear(&mut self, ev: &ServiceEvent) {
        match ev {
            ServiceEvent::Opened { host, start } => {
                let s = self.server(host);
                s.home.get_or_insert_with(|| start.clone());
                s.opened = true;
                s.settled = true;
                s.pending = Pending::None;
            }
            ServiceEvent::Status { host, status: ConnStatus::Failed(_) } => self.failed(host),
            ServiceEvent::Prompt(p) => self.asked(p),
            ServiceEvent::Listing { host, path, entries } => self.server(host).listed(path, entries),
            ServiceEvent::ListingPage { host, path, entries, first, done } => {
                self.server(host).page(path, entries, *first, *done);
            }
            ServiceEvent::ListFailed { host, path, error } => self.server(host).unlisted(path, error),
            _ => {}
        }
    }

    /// The recording as the report's blocks and steps, names replaced through `r`.
    pub fn build(&self, app: &App, r: &mut Redactor) -> Built {
        let steps = script::compact(&self.steps);
        let version = app.report.about.clone().unwrap_or_else(super::About::here).version;
        let (fixture, mut model) = fixture::build(self, &steps, app, r, &version);
        let (script, lines) = script::build(self, &steps, &mut model, r, &version);
        Built {
            steps: lines,
            count: steps.len(),
            fixture,
            script,
            summarised: model.summarised,
            mid_session: self.start.is_some(),
        }
    }
}

/// A recording, ready for the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    /// The steps in words, names replaced, one a line.
    pub steps: Vec<String>,
    /// How many steps.
    pub count: usize,
    /// The fixture and the script, each in its fenced block (`scripts/repro.sh` finds them).
    pub fixture: String,
    pub script: String,
    /// How many folders were too big to go whole: their rows the steps didn't touch are
    /// summed up.
    pub summarised: usize,
    /// It started on a host already open: its sign-in isn't in it.
    pub mid_session: bool,
}

/// What the steps of the filter and of a search in subfolders say (neither is replayed: their
/// text is pieces of names).
const FILTER: &str = "Filtered the folder";
const SEARCH: &str = "Searched in subfolders";

impl App {
    /// How many steps this window's recording has, while it records.
    pub fn recording_steps(&self) -> Option<usize> {
        self.report.recording.as_ref().map(Recording::taken)
    }

    /// The host on screen and its folder, once open.
    fn place(&self) -> Option<Place> {
        let host = self.active.clone()?;
        let folder = self.conns.get(&host).filter(|c| c.opened).map(|c| c.cwd.clone());
        Some(Place { host, folder })
    }

    /// The row `name` of the folder on screen.
    fn row_named(&self, name: &str) -> Option<Row> {
        let e = self.conn()?.find(name)?;
        Some(Row { name: e.name.clone(), dir: e.kind.is_dir_like() })
    }

    /// The cursor's row.
    fn cursor_row_named(&self) -> Option<Row> {
        self.row_named(self.conn()?.selection.cursor()?)
    }

    /// A selected row: the cursor's if it's selected, else the first selected on screen.
    fn selected_row(&self) -> Option<Row> {
        let c = self.conn()?;
        let name = match c.selection.cursor().filter(|n| c.selection.contains(n)) {
            Some(n) => n.to_owned(),
            None => c.rows().find(|e| c.selection.contains(&e.name))?.name.clone(),
        };
        self.row_named(&name)
    }

    /// Before an action runs (`App::act`): a bug report lists the last actions by kind (its own
    /// sheet's aren't news), and a recording takes the step it makes.
    pub(in crate::state) fn observe(&mut self, action: &Action) -> Observed {
        if !matches!(action, Action::Report(_)) {
            self.report.recent.action(action.kind());
        }
        let Some(rec) = &self.report.recording else { return Observed::Off };
        let counts = rec.nesting.iter().all(|chooser| *chooser);
        let step = if counts { self.step_of(action) } else { None };
        let at = self.place();
        let Some(rec) = &mut self.report.recording else { return Observed::Off };
        rec.nesting.push(chooses(action));
        Observed::Frame(step.map(|s| rec.push(s, at)))
    }

    /// After it ran: where the window is now, the row type-to-select landed on, and a recording
    /// that's full stops and opens the sheet.
    pub(in crate::state) fn observed(&mut self, seen: Observed) {
        let Observed::Frame(step) = seen else { return };
        let after = self.place();
        let cursor = self.cursor_row_named();
        let now = (self.wall_ms)();
        let Some(rec) = &mut self.report.recording else { return };
        rec.nesting.pop();
        if let Some(t) = step.and_then(|i| rec.steps.get_mut(i)) {
            t.after = after;
            if let Step::TypeSelect(row) = &mut t.step {
                *row = cursor;
            }
        }
        if rec.nesting.is_empty() && rec.full(now) {
            self.report_action(ReportAction::StopRecording, &mut Vec::new());
        }
    }

    /// An event arrived (`App::apply`): a bug report lists the last events by their trace
    /// lines, and a recording keeps what the servers answered.
    pub(in crate::state) fn heard(&mut self, ev: &ServiceEvent) {
        self.report.recent.event(crate::state::service::trace_line(ev));
        if let Some(rec) = &mut self.report.recording {
            rec.hear(ev);
        }
    }

    /// The step `action` makes, if any.
    fn step_of(&self, action: &Action) -> Option<Step> {
        // While a search's results are on screen, the list's actions are theirs (`search.rs`).
        if self.results().is_some() && on_rows(action) {
            return Some(Step::NotReplayed(SEARCH));
        }
        Some(match action {
            Action::SelectHost(h) => Step::Connect(h.clone()),
            Action::Open(name) => Step::Open(self.row_named(name)?),
            Action::OpenSelection => Step::Open(self.cursor_row_named()?),
            Action::GoUp => Step::Up,
            Action::Back => Step::Back,
            Action::Forward => Step::Forward,
            Action::GoTo(path) => Step::GoTo(path.clone()),
            Action::Click { name, extend, toggle } => {
                Step::Click { row: self.row_named(name)?, extend: *extend, toggle: *toggle }
            }
            Action::Move { delta, extend } => Step::Move { delta: *delta, extend: *extend },
            Action::End { last, extend } => Step::End { last: *last, extend: *extend },
            Action::SelectAll => Step::SelectAll,
            Action::Type { .. } => Step::TypeSelect(None),
            Action::Sort(key) => Step::Sort(*key),
            Action::ToggleHidden => Step::Hidden,
            Action::OpenFilter | Action::CloseFilter | Action::FilterChanged(_) => Step::NotReplayed(FILTER),
            Action::Escape if self.conn().is_some_and(|c| c.filter.is_on()) => Step::NotReplayed(FILTER),
            Action::Escape => Step::Escape,
            Action::Search(_) => Step::NotReplayed(SEARCH),
            Action::SubmitPrompt | Action::DeclinePrompt | Action::DismissSheet => return self.sheet_step(action),
            Action::OpenLog => Step::OpenLog,
            Action::Disconnect(h) => Step::Disconnect(h.clone()),
            Action::Download | Action::Preview(PreviewAction::Download) => Step::Download,
            Action::ToggleTransfers => Step::Transfers,
            Action::Preview(PreviewAction::Toggle) if !self.ui.preview.is_open() => {
                Step::Preview(Some(self.cursor_row_named()?))
            }
            Action::Preview(_) => Step::Preview(None),
            Action::File(f) => return self.file_step(f),
            Action::Palette(_)
            | Action::DropHover { .. }
            | Action::Report(_)
            | Action::TerminalFailed(_)
            | Action::TerminalsFound(_) => return None,
            other => Step::NotReplayed(not_replayed(other)),
        })
    }

    /// The step of a sheet's buttons. Only which prompt it was is read, never its field.
    fn sheet_step(&self, action: &Action) -> Option<Step> {
        let prompt = match self.ui.sheets.open() {
            Some(Sheet::Ask { prompt, .. }) => Some(prompt),
            _ => None,
        };
        match action {
            Action::SubmitPrompt => {
                let p = prompt?;
                let kind = match p.kind {
                    PromptKind::Password => Answer::Password,
                    PromptKind::Passphrase { .. } => Answer::Passphrase,
                    PromptKind::Other => Answer::Code,
                    PromptKind::HostKey { .. } | PromptKind::Confirm => Answer::Yes,
                    PromptKind::Notice => return None,
                };
                Some(Step::Answer { host: p.host.clone(), kind, wrong: false })
            }
            Action::DeclinePrompt => Some(Step::Decline(prompt?.host.clone())),
            _ => Some(Step::Dismiss(prompt.map(|p| p.host.clone()))),
        }
    }

    /// The step of a file operation's action.
    fn file_step(&self, f: &FileAction) -> Option<Step> {
        use FileAction as F;
        Some(match f {
            F::NewFolder => Step::NewFolder,
            F::Rename => Step::Rename(self.selected_row()?),
            F::EditSubmit => {
                let field = self.files.edit.field()?;
                let dir = match field.renaming() {
                    Some(from) => self.row_named(from).is_some_and(|r| r.dir),
                    None => true,
                };
                Step::Named { name: field.edit.text.clone(), dir }
            }
            F::EditCancel => Step::EditCancel,
            F::Delete => Step::Delete(self.selected_row()?),
            F::ConfirmDelete => match self.ui.sheets.open() {
                Some(Sheet::Delete { names, .. }) => Step::ConfirmDelete(names.clone()),
                _ => return None,
            },
            F::Refresh => Step::Refresh,
            F::Properties => Step::Properties(self.selected_row()?),
            // A right-click selects its row first when it isn't selected.
            F::OpenMenu { kind: MenuKind::Row(name), .. } => {
                if self.conn()?.selection.contains(name) {
                    return None;
                }
                Step::Click { row: self.row_named(name)?, extend: false, toggle: false }
            }
            // A press on a row that didn't move ends its drag before it started.
            F::OpenMenu { .. }
            | F::CloseMenu
            | F::Pick { .. }
            | F::EditChanged
            | F::Scrolled(_)
            | F::DragOver { .. }
            | F::DragCancel => return None,
            F::Copy | F::Cut | F::CopyPath | F::Paste { .. } | F::RequestPaste { .. } => {
                Step::NotReplayed("Copied, cut or pasted")
            }
            F::DragStart(_) | F::DragDrop => Step::NotReplayed("Dragged rows"),
            F::EditBlur => Step::NotReplayed("Clicked away from a name being typed"),
            F::DismissToast(_) => Step::NotReplayed("Dismissed a note"),
            F::Calculate => Step::NotReplayed("Calculated a folder's size"),
        })
    }
}

/// The list's own actions, which are a search's while its results are on screen.
fn on_rows(action: &Action) -> bool {
    matches!(
        action,
        Action::Open(_)
            | Action::OpenSelection
            | Action::Click { .. }
            | Action::Move { .. }
            | Action::End { .. }
            | Action::SelectAll
            | Action::Type { .. }
            | Action::Sort(_)
            | Action::Download
            | Action::Preview(_)
            | Action::File(_)
    )
}

/// What an action this version doesn't replay did, in words.
fn not_replayed(action: &Action) -> &'static str {
    match action {
        Action::QuickConnect | Action::HostBook(_) => "Used quick connect or a host's menu",
        Action::SignIn(_) => "Signed in again from the banner",
        Action::Verbose { .. } => "Turned verbose logging on or off",
        Action::Upload { .. } | Action::PickUpload | Action::PickUploadFolder => "Uploaded files",
        Action::Transfer(_) | Action::TransfersTab(_) | Action::RevealTransfer(_) => "Used the transfers",
        Action::ResolveConflict(_) | Action::ToggleApplyAll => "Answered a name clash",
        Action::OpenSettings | Action::LeaveSettings | Action::SettingsSection(_) | Action::Setting(_) => {
            "Used Settings"
        }
        Action::NewWindow | Action::CloseWindow | Action::QuitAnyway | Action::CancelQuit => {
            "Opened or closed a window"
        }
        Action::Update(_) => "Used the update",
        Action::OpenTerminal(_) => "Opened a terminal",
        Action::RemoteEdit(_) => "Edited a file in a local editor",
        _ => "Did something else",
    }
}
