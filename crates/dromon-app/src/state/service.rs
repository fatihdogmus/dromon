//! The boundary between the UI and whatever does the networking (§4.1): the UI sends
//! [`Command`]s, and [`ServiceEvent`]s come back and are applied with [`super::App::apply`].
//!
//! Two kinds of backend implement it. `fixture::FixtureBackend` answers synchronously from the
//! [`Backend::send`] call (scenes, tests, `--fixture`, `DROMON_MOCK=1`). `service::ChannelBackend` forwards
//! each command to the tokio service thread and returns nothing; the service pushes events
//! back through `AppHandle::update_if(|app, _| { app.apply(event); true })`, so the UI never
//! blocks.
//!
//! Secrets cross this boundary in exactly one place, [`Command::AnswerPrompt`]'s
//! [`PromptAnswer`], whose `Debug` is redacted. Events never carry secrets.
//!
//! Every main window (⌘N) has a backend of its own over the one service ([`Backend::another`]):
//! the backend says which window each command came from, so the service knows each window's
//! folder on screen and how it shows, and which window a prompt goes to. Events about hosts,
//! listings and transfers go to every window, and each takes what concerns it.

use dromon_core::{ConnStatus, Entry, Error, HostId, HostSpec, Prompt, PromptAnswer, PromptId, RemotePath};
use dromon_transfer::{ConflictChoice, JobId, JobRequest, JobView, TaskState};

pub use super::hostbook::{HostBookCommand, HostBookEvent};
pub use super::ops::{FileOp, FileOpEvent, FileOpKind, OpId};
pub use super::preview::{PreviewBytes, PreviewCommand, PreviewEvent, PreviewId, PreviewRange};
pub use super::properties::{SizeCommand, SizeEvent, SizeId, SizeTotal};
pub use super::remote_edit::{EditChoice, EditId, EditStage, RemoteEditCommand, RemoteEditEvent};
pub use super::search::{Found, SearchCommand, SearchEvent, SearchId};
pub use super::update::{UpdateCommand, UpdateEvent};

/// One main window of the app, for the session: the first is [`WindowId::FIRST`], each ⌘N
/// takes the next number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u32);

impl WindowId {
    /// The window the app starts with.
    pub const FIRST: WindowId = WindowId(0);
}

/// What the UI asks of the service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// The user's answer to the open prompt `id`. Answers to prompts that are gone are ignored.
    AnswerPrompt { id: PromptId, answer: PromptAnswer },
    /// Start a connection; answered with `Status` changes, maybe `Prompt`s, then `Opened`.
    Connect { host: HostId },
    /// Close the connection and forget the host's remembered secrets; answered with
    /// `Status(Disconnected)`.
    Disconnect { host: HostId },
    /// A one-shot file operation on the host's session (`state/ops.rs`); answered with `FileOp`
    /// events: [`FileOpEvent::Progress`] (deletes), then `Done` or `Failed`, then a fresh
    /// listing of the folder on screen when the operation changed it ([`FileOp::relists`]). A
    /// host that isn't connected answers `Failed` with [`Error::ConnectionLost`] at once.
    FileOp { id: OpId, host: HostId, op: FileOp },
    /// The Properties sheet's folder sizes (`state/properties.rs`); answered with `FolderSize`
    /// events.
    FolderSize(SizeCommand),
    /// Saved and recent hosts, Add to SSH config (`state/hostbook.rs`); answered with
    /// `HostBook` events.
    HostBook(HostBookCommand),
    /// List a folder; answered with `Listing` (whole) or `ListingPage`s, or `ListFailed`.
    List { host: HostId, path: RemotePath },
    /// Settings' "Live updates when the server supports them" (on by default): whether hosts
    /// watch their folder on screen live (`service/live.rs`), for every host from now on.
    /// Answered with `Live` events as hosts start or stop.
    LiveUpdates { on: bool },
    /// Poll the sending window's folder on screen on `host` now (it just gained focus):
    /// answered like any poll, with `Refreshed` or `ListFailed` when it changed, else nothing.
    Poll { host: HostId },
    /// Quick preview, Space (`state/preview.rs`); answered with `Preview` events.
    Preview(PreviewCommand),
    /// Edit in a local editor (`state/remote_edit.rs`); answered with `RemoteEdit` events, to
    /// every window.
    RemoteEdit(RemoteEditCommand),
    /// Search in subfolders (`state/search.rs`); answered with `Search` events.
    Search(SearchCommand),
    /// The reconnecting banner's Sign in (§3.9): the host's open, unanswered prompts again, to
    /// the sending window only; the host's later prompts go to it too. Nothing if none are open.
    ShowPrompts { host: HostId },
    /// Something for the transfer engine; answered with `Transfers` snapshots.
    Transfer(TransferCommand),
    /// Checking for and installing a newer Dromon (`state/update.rs`); answered with `Update`
    /// events.
    Update(UpdateCommand),
    /// The connection log's "Verbose logging" (§6.8): run ssh for `host` with `-v` (`on`) or
    /// without, from now on in this session, so its `debug1:` lines reach the log. A live
    /// connection reconnects at once to take it (silently, the folders on screen kept); a host
    /// that isn't connected takes it at its next connect. Answered with `Verbose` to every
    /// window, then the reconnect's `Status` changes and log lines.
    Verbose { host: HostId, on: bool },
    /// How the sending window shows (§4.5): a folder on screen is polled while a window that
    /// shows it is `visible`, at the focused interval while one of them is focused, else the
    /// slower unfocused one. Until a window's first one arrives its folders aren't polled.
    Window { visible: bool, focused: bool },
    /// The sending window closed: its folders aren't polled any more, and a prompt waiting
    /// in it moves to another window.
    WindowClosed,
}

/// Whether a host's folder on screen updates live (`service/live.rs`): the server tells the app
/// of each change in it as it happens, and polls only back that up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Live {
    /// Polled, as always (nothing to watch, or the watch isn't up yet or ended).
    #[default]
    Off,
    /// This folder is watched live.
    On(RemotePath),
    /// The server can't do it for this connection: polled.
    Unavailable(LiveUnavailable),
}

/// Why a host's connection has no live updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveUnavailable {
    /// The server has no `inotifywait` (or isn't Linux).
    NoInotify,
    /// Signing in a second time needs an answer Dromon doesn't remember (a code, a host key).
    SignIn,
    /// The second session ended before its watch started.
    Failed,
}

impl LiveUnavailable {
    /// Why, as the connection's tooltip and log say it.
    pub fn reason(self) -> &'static str {
        match self {
            LiveUnavailable::NoInotify => "inotifywait isn't installed on this server",
            LiveUnavailable::SignIn => "it needs a sign-in Dromon can't answer by itself",
            LiveUnavailable::Failed => "the server didn't start watching the folder",
        }
    }

    fn name(self) -> &'static str {
        match self {
            LiveUnavailable::NoInotify => "NoInotify",
            LiveUnavailable::SignIn => "SignIn",
            LiveUnavailable::Failed => "Failed",
        }
    }
}

/// What the UI asks of the transfer engine (§4.6). Unknown or finished job ids are ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferCommand {
    Start(JobRequest),
    Pause(JobId),
    Resume(JobId),
    Cancel(JobId),
    Retry(JobId),
    /// The answer to the job's name clash (`JobView::conflict`).
    Resolve {
        job: JobId,
        choice: ConflictChoice,
        apply_all: bool,
    },
    /// Forget finished, failed and cancelled jobs.
    ClearFinished,
    /// Settings' "Transfers at once": how many files may run at once per host (§8), for the
    /// starts from now on; running files aren't stopped. Answered like any transfer command.
    PerHost(usize),
}

/// What the service tells the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ServiceEvent {
    /// How a file operation goes (`state/ops.rs`).
    FileOp(FileOpEvent),
    /// How a folder size walk goes (`state/properties.rs`).
    FolderSize(SizeEvent),
    /// Saved hosts and the ssh config (`state/hostbook.rs`).
    HostBook(HostBookEvent),
    /// The hosts in the user's ssh config: replaces the sidebar's SSH Config section.
    Hosts(Vec<HostSpec>),
    ListFailed {
        host: HostId,
        path: RemotePath,
        error: Error,
    },
    /// A whole folder at once (tests that build listings by hand).
    Listing {
        host: HostId,
        path: RemotePath,
        entries: Vec<Entry>,
    },
    /// One page of a folder as it streams in. `first` starts a new listing of `path` (it
    /// replaces what was there); later pages add to it; `done` marks the last one (its
    /// `entries` may be empty).
    ListingPage {
        host: HostId,
        path: RemotePath,
        entries: Vec<Entry>,
        first: bool,
        done: bool,
    },
    /// Whether `host`'s folder on screen updates live now (`Command::LiveUpdates`), to every
    /// window when it changes, and to a window that opens a host already connected.
    Live {
        host: HostId,
        live: Live,
    },
    /// One line for the host's connection log: ssh's stderr or our own lifecycle notes.
    /// Never contains secrets.
    Log {
        host: HostId,
        line: String,
    },
    /// The session is up and its starting folder resolved (home, or the last folder used).
    Opened {
        host: HostId,
        start: RemotePath,
    },
    /// What a quick preview read brought, or why it couldn't (`state/preview.rs`).
    Preview(PreviewEvent),
    /// ssh asks something only the user can answer: show a sheet (or, for a notice, a toast).
    Prompt(Prompt),
    /// The prompt went away without an answer from here (ssh hung up, a notice ended).
    PromptClosed(PromptId),
    /// A poll found the folder on screen changed (§4.5): its whole listing now. Always a
    /// refresh of a folder already listed, never a navigation; polls that find nothing new
    /// send nothing.
    Refreshed {
        host: HostId,
        path: RemotePath,
        entries: Vec<Entry>,
    },
    /// How a file being edited in a local editor goes (`state/remote_edit.rs`).
    RemoteEdit(RemoteEditEvent),
    /// What a search in subfolders found, and how it ended (`state/search.rs`).
    Search(SearchEvent),
    Status {
        host: HostId,
        status: ConnStatus,
    },
    /// Every transfer job, newest first: replaces the list. Sent when something changed, at
    /// most about ten times a second while bytes move.
    Transfers(Vec<JobView>),
    /// The updater's news (`state/update.rs`).
    Update(UpdateEvent),
    /// Whether ssh for `host` runs with `-v` (the log's "Verbose logging"): after a
    /// `Command::Verbose` from any window, and to a window that connects while it's on.
    Verbose {
        host: HostId,
        on: bool,
    },
}

/// Set to `1` to have [`super::App::apply`] write one [`trace_line`] per event to stderr.
pub const TRACE_ENV: &str = "DROMON_TRACE";

/// Whether `DROMON_TRACE=1` (read once).
pub fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os(TRACE_ENV).is_some_and(|v| v == "1"))
}

/// One line naming what arrived: the variant, its host and a count or state name. Never any
/// text the event carries (paths, entries, prompt text, log lines, error messages).
pub fn trace_line(ev: &ServiceEvent) -> String {
    match ev {
        ServiceEvent::FileOp(ev) => super::ops::trace_line(ev),
        ServiceEvent::FolderSize(ev) => super::properties::trace_line(ev),
        ServiceEvent::HostBook(ev) => super::hostbook::trace_line(ev),
        ServiceEvent::Hosts(hosts) => format!("Hosts count={}", hosts.len()),
        ServiceEvent::ListFailed { host, .. } => format!("ListFailed host={host}"),
        ServiceEvent::Listing { host, entries, .. } => format!("Listing host={host} entries={}", entries.len()),
        ServiceEvent::ListingPage { host, entries, done, .. } => {
            format!("ListingPage host={host} entries={} done={done}", entries.len())
        }
        ServiceEvent::Live { host, live } => match live {
            Live::Off => format!("Live host={host} state=Off"),
            Live::On(_) => format!("Live host={host} state=On"),
            Live::Unavailable(why) => format!("Live host={host} state=Unavailable why={}", why.name()),
        },
        ServiceEvent::Log { host, .. } => format!("Log host={host}"),
        ServiceEvent::Opened { host, .. } => format!("Opened host={host}"),
        ServiceEvent::Preview(ev) => super::preview::trace_line(ev),
        ServiceEvent::Prompt(p) => format!("Prompt host={} id={}", p.host, p.id.0),
        ServiceEvent::PromptClosed(id) => format!("PromptClosed id={}", id.0),
        ServiceEvent::Refreshed { host, entries, .. } => format!("Refreshed host={host} entries={}", entries.len()),
        ServiceEvent::RemoteEdit(ev) => super::remote_edit::trace_line(ev),
        ServiceEvent::Search(ev) => super::search::trace_line(ev),
        ServiceEvent::Status { host, status } => {
            let state = match status {
                ConnStatus::Connecting => "Connecting".to_owned(),
                ConnStatus::Ready => "Ready".to_owned(),
                ConnStatus::Reconnecting { attempt } => format!("Reconnecting attempt={attempt}"),
                ConnStatus::NeedsInput => "NeedsInput".to_owned(),
                ConnStatus::Failed(_) => "Failed".to_owned(),
                ConnStatus::Disconnected => "Disconnected".to_owned(),
            };
            format!("Status host={host} status={state}")
        }
        ServiceEvent::Transfers(jobs) => {
            let active = jobs.iter().filter(|j| j.state.is_active()).count();
            let done = jobs.iter().filter(|j| j.state == TaskState::Done).count();
            format!("Transfers jobs={} active={active} done={done}", jobs.len())
        }
        ServiceEvent::Update(ev) => format!("Update {}", ev.name()),
        ServiceEvent::Verbose { host, on } => format!("Verbose host={host} on={on}"),
    }
}

/// Where commands go. `send` may answer at once with events (a fixture does); a threaded
/// backend returns an empty list and delivers events later.
pub trait Backend {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent>;

    /// The same service for another main window, `window` (⌘N): its commands are that
    /// window's. `None`: this backend serves one window only (scenes, most tests).
    fn another(&mut self, window: WindowId) -> Option<Box<dyn Backend>> {
        let _ = window;
        None
    }

    /// Events that arrived for this window since it last asked, from what another window did
    /// (a synchronous backend's; a threaded one delivers through its sink instead).
    fn pending(&mut self) -> Vec<ServiceEvent> {
        Vec::new()
    }

    /// Call `wake(window)` when events wait in [`Backend::pending`] for `window`, so it
    /// catches up without waiting for its own next command.
    fn set_wake(&mut self, wake: Box<dyn Fn(WindowId)>) {
        drop(wake);
    }
}

/// A backend that never answers: connections stay where the UI left them (the `connecting`
/// scene, and tests that drive events by hand).
#[derive(Debug, Default)]
pub struct Silent {
    pub sent: Vec<Command>,
}

impl Backend for Silent {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        self.sent.push(cmd);
        Vec::new()
    }
}

/// A silent backend whose commands a test can read after handing it to the app.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct Recorder(pub std::rc::Rc<std::cell::RefCell<Vec<Command>>>);

#[cfg(test)]
impl Recorder {
    /// Everything sent so far, oldest first.
    pub fn sent(&self) -> Vec<Command> {
        self.0.borrow().clone()
    }
}

#[cfg(test)]
impl Backend for Recorder {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        self.0.borrow_mut().push(cmd);
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use dromon_core::PromptKind;

    use super::*;

    #[test]
    fn trace_lines_name_the_event_and_never_its_text() {
        let host = HostId("prod".into());
        let events = [
            ServiceEvent::Hosts(vec![HostSpec::from_config_alias("prod"), HostSpec::from_config_alias("pi")]),
            ServiceEvent::Status { host: host.clone(), status: ConnStatus::Failed("secret reason".into()) },
            ServiceEvent::Status { host: host.clone(), status: ConnStatus::Reconnecting { attempt: 2 } },
            ServiceEvent::Opened { host: host.clone(), start: RemotePath::new("/secret/home") },
            ServiceEvent::ListFailed {
                host: host.clone(),
                path: RemotePath::new("/secret"),
                error: Error::Other("secret".into()),
            },
            ServiceEvent::Prompt(Prompt {
                id: PromptId(7),
                host: host.clone(),
                kind: PromptKind::Password,
                text: "secret@prod's password: ".into(),
                error: Some("secret".into()),
            }),
            ServiceEvent::PromptClosed(PromptId(7)),
            ServiceEvent::Log { host: host.clone(), line: "debug1: secret".into() },
            ServiceEvent::Verbose { host, on: true },
        ];
        let lines: Vec<String> = events.iter().map(trace_line).collect();
        assert_eq!(
            lines,
            [
                "Hosts count=2",
                "Status host=prod status=Failed",
                "Status host=prod status=Reconnecting attempt=2",
                "Opened host=prod",
                "ListFailed host=prod",
                "Prompt host=prod id=7",
                "PromptClosed id=7",
                "Log host=prod",
                "Verbose host=prod on=true",
            ]
        );
    }

    #[test]
    fn file_op_trace_lines_name_the_kind_host_and_id_only() {
        let host = HostId("prod".into());
        let events = [
            FileOpEvent::Progress { id: OpId(3), host: host.clone(), done: 12, total: Some(40) },
            FileOpEvent::Done { id: OpId(3), host: host.clone() },
            FileOpEvent::Failed { id: OpId(4), host, error: Error::PermissionDenied(RemotePath::new("/secret/dir")) },
        ];
        let lines: Vec<String> = events.into_iter().map(|e| trace_line(&ServiceEvent::FileOp(e))).collect();
        assert_eq!(
            lines,
            ["FileOpProgress host=prod id=3", "FileOpDone host=prod id=3", "FileOpFailed host=prod id=4"]
        );
    }

    #[test]
    fn folder_size_trace_lines_name_the_host_and_id_only() {
        let host = HostId("prod".into());
        let total = SizeTotal { bytes: 9, files: 2, folders: 1, unreadable: 1 };
        let events = [
            SizeEvent::Progress { id: SizeId(5), host: host.clone(), total },
            SizeEvent::Done { id: SizeId(5), host: host.clone(), total },
            SizeEvent::Failed {
                id: SizeId(6),
                host,
                total,
                error: Error::PermissionDenied(RemotePath::new("/secret")),
            },
        ];
        let lines: Vec<String> = events.into_iter().map(|e| trace_line(&ServiceEvent::FolderSize(e))).collect();
        assert_eq!(
            lines,
            ["FolderSizeProgress host=prod id=5", "FolderSizeDone host=prod id=5", "FolderSizeFailed host=prod id=6"]
        );
        assert_eq!(total.items(), 3);
    }

    #[test]
    fn preview_trace_lines_and_bytes_count_and_never_show_the_contents() {
        let host = HostId("prod".into());
        let bytes = PreviewBytes(b"DB_PASSWORD=secret\n".as_slice().into());
        let events = [
            PreviewEvent::Done { id: PreviewId(4), host: host.clone(), bytes: bytes.clone(), total: 1204 },
            PreviewEvent::Failed { id: PreviewId(5), host, error: Error::NotFound(RemotePath::new("/secret")) },
        ]
        .map(ServiceEvent::Preview);
        let lines: Vec<String> = events.iter().map(trace_line).collect();
        assert_eq!(lines, ["Preview host=prod id=4 bytes=19 total=1204", "PreviewFailed host=prod id=5"]);
        assert_eq!(format!("{bytes:?}"), "PreviewBytes(<19 bytes>)");
        assert!(!format!("{:?}", events[0]).contains("secret"), "{:?}", events[0]);
    }

    #[test]
    fn a_preview_range_is_the_head_or_the_tail_of_what_there_is() {
        assert_eq!(PreviewRange::Head(10).span(4), (0, 4));
        assert_eq!(PreviewRange::Head(10).span(25), (0, 10));
        assert_eq!(PreviewRange::Tail(10).span(25), (15, 10));
        assert_eq!(PreviewRange::Tail(10).span(4), (0, 4));
        assert_eq!(PreviewRange::Tail(10).span(0), (0, 0));
    }

    #[test]
    fn search_trace_lines_count_and_never_name_what_was_found() {
        let host = HostId("prod".into());
        let entry = Entry {
            name: "secret.log".into(),
            kind: dromon_core::EntryKind::File,
            size: 1,
            mtime: None,
            mode: None,
            owner: None,
            group: None,
        };
        let found = Found { folder: "secret/dir".into(), entry };
        assert_eq!(found.path(&RemotePath::new("/srv")), RemotePath::new("/srv/secret/dir/secret.log"));
        assert_eq!(
            Found { folder: ".".into(), ..found.clone() }.dir(&RemotePath::new("/srv")),
            RemotePath::new("/srv")
        );
        let events = [
            SearchEvent::Found { id: SearchId(3), host: host.clone(), entries: vec![found], folders: 12 },
            SearchEvent::Done { id: SearchId(3), host: host.clone(), folders: 40, unreadable: 1, capped: false },
            SearchEvent::Failed { id: SearchId(4), host, error: Error::PermissionDenied(RemotePath::new("/secret")) },
        ];
        let lines: Vec<String> = events.into_iter().map(|e| trace_line(&ServiceEvent::Search(e))).collect();
        assert_eq!(
            lines,
            [
                "SearchFound host=prod id=3 entries=1 folders=12",
                "SearchDone host=prod id=3 folders=40 unreadable=1 capped=false",
                "SearchFailed host=prod id=4",
            ]
        );
    }

    #[test]
    fn a_refresh_trace_line_counts_entries_and_names_none() {
        let entry = Entry {
            name: "secret.txt".into(),
            kind: dromon_core::EntryKind::File,
            size: 1,
            mtime: None,
            mode: None,
            owner: None,
            group: None,
        };
        let ev = ServiceEvent::Refreshed {
            host: HostId("prod".into()),
            path: RemotePath::new("/secret"),
            entries: vec![entry],
        };
        assert_eq!(trace_line(&ev), "Refreshed host=prod entries=1");
    }

    #[test]
    fn live_trace_lines_name_the_state_and_never_the_folder() {
        let host = HostId("prod".into());
        let live = |live| ServiceEvent::Live { host: host.clone(), live };
        let lines = [
            live(Live::On(RemotePath::new("/secret/folder"))),
            live(Live::Off),
            live(Live::Unavailable(LiveUnavailable::SignIn)),
        ]
        .map(|ev| trace_line(&ev));
        assert_eq!(
            lines,
            ["Live host=prod state=On", "Live host=prod state=Off", "Live host=prod state=Unavailable why=SignIn"]
        );
    }

    #[test]
    fn update_trace_lines_name_the_event_and_never_its_versions_or_errors() {
        use crate::state::update::{Manual, Method, Offer, Version};
        let offer = Offer {
            version: Version::parse("7.7.7").unwrap(),
            page: "https://example.invalid/secret".into(),
            method: Method::Page(Manual::Unknown),
        };
        let events = [
            UpdateEvent::Checking,
            UpdateEvent::UpToDate,
            UpdateEvent::Available(offer),
            UpdateEvent::CheckFailed("secret".into()),
            UpdateEvent::Installing,
            UpdateEvent::InstallFailed("secret".into()),
            UpdateEvent::Installed { relaunch: true },
        ];
        let lines: Vec<String> = events.into_iter().map(|e| trace_line(&ServiceEvent::Update(e))).collect();
        assert_eq!(
            lines,
            [
                "Update Checking",
                "Update UpToDate",
                "Update Available",
                "Update CheckFailed",
                "Update Installing",
                "Update InstallFailed",
                "Update Installed",
            ]
        );
        assert!(lines.iter().all(|l| !l.contains("secret") && !l.contains("7.7.7")));
    }
}
