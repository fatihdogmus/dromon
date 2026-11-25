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
}

/// What the service tells the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// ssh asks something only the user can answer: show a sheet (or, for a notice, a toast).
    Prompt(Prompt),
    /// The prompt went away without an answer from here (ssh hung up, a notice ended).
    PromptClosed(PromptId),
    /// One line for the host's connection log: ssh's stderr or our own lifecycle notes.
    /// Never contains secrets.
    Log { host: HostId, line: String },
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
        ServiceEvent::Prompt(p) => format!("Prompt host={} id={}", p.host, p.id.0),
        ServiceEvent::PromptClosed(id) => format!("PromptClosed id={}", id.0),
        ServiceEvent::Log { host, .. } => format!("Log host={host}"),
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
}
