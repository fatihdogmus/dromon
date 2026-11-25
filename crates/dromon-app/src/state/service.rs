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

use dromon_core::{ConnStatus, Entry, Error, HostId, HostSpec, Prompt, PromptAnswer, PromptId, RemotePath};
use dromon_transfer::{ConflictChoice, JobId, JobRequest, JobView, TaskState};

pub use super::hostbook::{HostBookCommand, HostBookEvent};

/// What the UI asks of the service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Start a connection; answered with `Status` changes, maybe `Prompt`s, then `Opened`.
    Connect { host: HostId },
    /// List a folder; answered with `Listing` (whole) or `ListingPage`s, or `ListFailed`.
    List { host: HostId, path: RemotePath },
    /// Close the connection and forget the host's remembered secrets; answered with
    /// `Status(Disconnected)`.
    Disconnect { host: HostId },
    /// The user's answer to the open prompt `id`. Answers to prompts that are gone are ignored.
    AnswerPrompt { id: PromptId, answer: PromptAnswer },
    /// Something for the transfer engine; answered with `Transfers` snapshots.
    Transfer(TransferCommand),
    /// A one-shot file operation on the host's session; answered with `FileOpProgress`
    /// (deletes), then `FileOpDone` or `FileOpFailed`, then a fresh listing of the folder on
    /// screen when the operation changed it ([`FileOp::relists`]). A host that isn't connected
    /// answers `FileOpFailed` with [`Error::ConnectionLost`] at once.
    FileOp { id: OpId, host: HostId, op: FileOp },
    /// How the window shows (§4.5): the folder on screen of each connected host is polled
    /// only while `visible`, at the focused or the slower unfocused interval. Until the first
    /// one arrives the service polls nothing.
    Window { visible: bool, focused: bool },
    /// Poll `host`'s folder on screen now (the window just gained focus): answered like any
    /// poll, with `Refreshed` or `ListFailed` when it changed, else nothing.
    Poll { host: HostId },
    /// Saved and recent hosts, Add to SSH config (`state/hostbook.rs`); answered with
    /// `HostBook` events.
    HostBook(HostBookCommand),
}

/// Identifies one file operation, chosen by the UI (unique per session).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpId(pub u64);

/// A simple file operation (§5 M3): new folder, rename, delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    /// Create one folder (its parent must exist).
    Mkdir { path: RemotePath },
    /// Rename an item within its folder (`to` is a sibling of `from`); fails with AlreadyExists if `to` exists.
    Rename { from: RemotePath, to: RemotePath },
    /// Delete items; folders recursively. Symlinks are removed themselves, never followed.
    Delete { paths: Vec<RemotePath> },
}

/// What kind of [`FileOp`] it is, without its paths (for trace lines and the ops list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOpKind {
    Mkdir,
    Rename,
    Delete,
}

impl FileOpKind {
    pub fn name(self) -> &'static str {
        match self {
            FileOpKind::Mkdir => "Mkdir",
            FileOpKind::Rename => "Rename",
            FileOpKind::Delete => "Delete",
        }
    }
}

impl FileOp {
    pub fn kind(&self) -> FileOpKind {
        match self {
            FileOp::Mkdir { .. } => FileOpKind::Mkdir,
            FileOp::Rename { .. } => FileOpKind::Rename,
            FileOp::Delete { .. } => FileOpKind::Delete,
        }
    }

    /// Every path the operation changes.
    pub fn affected(&self) -> Vec<&RemotePath> {
        match self {
            FileOp::Mkdir { path } => vec![path],
            FileOp::Rename { from, to } => vec![from, to],
            FileOp::Delete { paths } => paths.iter().collect(),
        }
    }

    /// The paths that no longer exist once the operation succeeded.
    pub fn removed(&self) -> Vec<&RemotePath> {
        match self {
            FileOp::Mkdir { .. } => Vec::new(),
            FileOp::Rename { from, to } if from != to => vec![from],
            FileOp::Rename { .. } => Vec::new(),
            FileOp::Delete { paths } => paths.iter().collect(),
        }
    }

    /// Whether the folder on screen, `on_screen`, is listed again after the operation ended
    /// (`ok`: it succeeded). It is when it's the parent of an affected path, or affected itself
    /// (or lies inside one) — except after a success that removed it (it, or an ancestor, was
    /// deleted or renamed away): a path that's gone isn't listed, and the UI moves away from it.
    pub fn relists(&self, on_screen: &RemotePath, ok: bool) -> bool {
        let touched = self.affected().iter().any(|a| a.parent().as_ref() == Some(on_screen) || on_screen.starts_with(a));
        let gone = ok && self.removed().iter().any(|r| on_screen.starts_with(r));
        touched && !gone
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
    Resolve { job: JobId, choice: ConflictChoice, apply_all: bool },
    /// Forget finished, failed and cancelled jobs.
    ClearFinished,
}

/// What the service tells the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ServiceEvent {
    /// The hosts in the user's ssh config: replaces the sidebar's SSH Config section.
    Hosts(Vec<HostSpec>),
    Status { host: HostId, status: ConnStatus },
    /// The session is up and its starting folder resolved (home, or the last folder used).
    Opened { host: HostId, start: RemotePath },
    /// A whole folder at once (tests that build listings by hand).
    Listing { host: HostId, path: RemotePath, entries: Vec<Entry> },
    /// One page of a folder as it streams in. `first` starts a new listing of `path` (it
    /// replaces what was there); later pages add to it; `done` marks the last one (its
    /// `entries` may be empty).
    ListingPage { host: HostId, path: RemotePath, entries: Vec<Entry>, first: bool, done: bool },
    ListFailed { host: HostId, path: RemotePath, error: Error },
    /// A poll found the folder on screen changed (§4.5): its whole listing now. Always a
    /// refresh of a folder already listed, never a navigation; polls that find nothing new
    /// send nothing.
    Refreshed { host: HostId, path: RemotePath, entries: Vec<Entry> },
    /// ssh asks something only the user can answer: show a sheet (or, for a notice, a toast).
    Prompt(Prompt),
    /// The prompt went away without an answer from here (ssh hung up, a notice ended).
    PromptClosed(PromptId),
    /// One line for the host's connection log: ssh's stderr or our own lifecycle notes.
    /// Never contains secrets.
    Log { host: HostId, line: String },
    /// Every transfer job, newest first: replaces the list. Sent when something changed, at
    /// most about ten times a second while bytes move.
    Transfers(Vec<JobView>),
    /// How far a delete got, at most about ten times a second: while it counts what's there,
    /// `done` is the entries found so far and `total` is `None`; while it removes them, `done`
    /// is the entries removed and `total` all of them.
    FileOpProgress { id: OpId, host: HostId, done: u64, total: Option<u64> },
    /// The operation succeeded.
    FileOpDone { id: OpId, host: HostId },
    /// The operation stopped at its first error (a delete may have removed some entries).
    FileOpFailed { id: OpId, host: HostId, error: Error },
    /// Saved hosts and the ssh config (`state/hostbook.rs`).
    HostBook(HostBookEvent),
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
        ServiceEvent::Hosts(hosts) => format!("Hosts count={}", hosts.len()),
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
        ServiceEvent::Opened { host, .. } => format!("Opened host={host}"),
        ServiceEvent::Listing { host, entries, .. } => format!("Listing host={host} entries={}", entries.len()),
        ServiceEvent::ListingPage { host, entries, done, .. } => {
            format!("ListingPage host={host} entries={} done={done}", entries.len())
        }
        ServiceEvent::ListFailed { host, .. } => format!("ListFailed host={host}"),
        ServiceEvent::Refreshed { host, entries, .. } => format!("Refreshed host={host} entries={}", entries.len()),
        ServiceEvent::Prompt(p) => format!("Prompt host={} id={}", p.host, p.id.0),
        ServiceEvent::PromptClosed(id) => format!("PromptClosed id={}", id.0),
        ServiceEvent::Log { host, .. } => format!("Log host={host}"),
        ServiceEvent::Transfers(jobs) => {
            let active = jobs.iter().filter(|j| j.state.is_active()).count();
            let done = jobs.iter().filter(|j| j.state == TaskState::Done).count();
            format!("Transfers jobs={} active={active} done={done}", jobs.len())
        }
        ServiceEvent::FileOpProgress { id, host, .. } => format!("FileOpProgress host={host} id={}", id.0),
        ServiceEvent::FileOpDone { id, host } => format!("FileOpDone host={host} id={}", id.0),
        ServiceEvent::FileOpFailed { id, host, .. } => format!("FileOpFailed host={host} id={}", id.0),
        ServiceEvent::HostBook(ev) => super::hostbook::trace_line(ev),
    }
}

/// Where commands go. `send` may answer at once with events (a fixture does); a threaded
/// backend returns an empty list and delivers events later.
pub trait Backend {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent>;
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
            ServiceEvent::ListFailed { host: host.clone(), path: RemotePath::new("/secret"), error: Error::Other("secret".into()) },
            ServiceEvent::Prompt(Prompt {
                id: PromptId(7),
                host: host.clone(),
                kind: PromptKind::Password,
                text: "secret@prod's password: ".into(),
                error: Some("secret".into()),
            }),
            ServiceEvent::PromptClosed(PromptId(7)),
            ServiceEvent::Log { host, line: "debug1: secret".into() },
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
            ]
        );
    }

    #[test]
    fn file_op_trace_lines_name_the_kind_host_and_id_only() {
        let host = HostId("prod".into());
        let events = [
            ServiceEvent::FileOpProgress { id: OpId(3), host: host.clone(), done: 12, total: Some(40) },
            ServiceEvent::FileOpDone { id: OpId(3), host: host.clone() },
            ServiceEvent::FileOpFailed { id: OpId(4), host, error: Error::PermissionDenied(RemotePath::new("/secret/dir")) },
        ];
        let lines: Vec<String> = events.iter().map(trace_line).collect();
        assert_eq!(lines, ["FileOpProgress host=prod id=3", "FileOpDone host=prod id=3", "FileOpFailed host=prod id=4"]);
    }

    #[test]
    fn a_refresh_trace_line_counts_entries_and_names_none() {
        let entry = Entry { name: "secret.txt".into(), kind: dromon_core::EntryKind::File, size: 1, mtime: None, mode: None };
        let ev = ServiceEvent::Refreshed { host: HostId("prod".into()), path: RemotePath::new("/secret"), entries: vec![entry] };
        assert_eq!(trace_line(&ev), "Refreshed host=prod entries=1");
    }

    fn p(s: &str) -> RemotePath {
        RemotePath::new(s)
    }

    #[test]
    fn file_ops_name_what_they_change() {
        let mk = FileOp::Mkdir { path: p("/a/new") };
        assert_eq!((mk.kind(), mk.affected(), mk.removed()), (FileOpKind::Mkdir, vec![&p("/a/new")], vec![]));
        let mv = FileOp::Rename { from: p("/a/x"), to: p("/a/y") };
        assert_eq!((mv.kind(), mv.affected(), mv.removed()), (FileOpKind::Rename, vec![&p("/a/x"), &p("/a/y")], vec![&p("/a/x")]));
        assert!(FileOp::Rename { from: p("/a/x"), to: p("/a/x") }.removed().is_empty());
        let rm = FileOp::Delete { paths: vec![p("/a/x"), p("/b")] };
        assert_eq!((rm.kind(), rm.removed()), (FileOpKind::Delete, vec![&p("/a/x"), &p("/b")]));
        assert_eq!(FileOpKind::Delete.name(), "Delete");
    }

    #[test]
    fn the_folder_on_screen_is_listed_again_when_it_changed_and_still_exists() {
        let rm = FileOp::Delete { paths: vec![p("/srv/www/old"), p("/srv/logs")] };
        // The parent of a deleted item: listed again, whether or not the delete got through.
        assert!(rm.relists(&p("/srv/www"), true) && rm.relists(&p("/srv/www"), false));
        assert!(rm.relists(&p("/srv"), true));
        // Unrelated folders aren't.
        assert!(!rm.relists(&p("/srv/www/new"), true) && !rm.relists(&p("/"), true));
        // The deleted folder itself, or one inside it: gone after a success, so not listed;
        // after a failure it may still be there, and is.
        assert!(!rm.relists(&p("/srv/www/old"), true) && !rm.relists(&p("/srv/www/old/a/b"), true));
        assert!(rm.relists(&p("/srv/www/old"), false) && rm.relists(&p("/srv/logs/x"), false));

        let mk = FileOp::Mkdir { path: p("/home/u/new") };
        assert!(mk.relists(&p("/home/u"), true) && mk.relists(&p("/home/u"), false));
        assert!(!mk.relists(&p("/home"), true));

        let mv = FileOp::Rename { from: p("/home/u/a"), to: p("/home/u/b") };
        assert!(mv.relists(&p("/home/u"), true));
        // Renaming the folder on screen: its old path is gone.
        assert!(!mv.relists(&p("/home/u/a"), true) && mv.relists(&p("/home/u/a"), false));
        assert!(!mv.relists(&p("/home/u/c"), true));
    }
}
