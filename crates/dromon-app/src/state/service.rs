//! The boundary between the UI and whatever does the networking (§4.1): the UI sends
//! [`Command`]s, and [`ServiceEvent`]s come back and are applied with [`super::App::apply`].
//!
//! Today the only backend is `mock::MockService`, which answers synchronously from the
//! [`Backend::send`] call. The real one will be a channel to the tokio service thread:
//! `send` forwards the command and returns nothing, the service thread pushes events through
//! `AppHandle::update_if(|app| { app.apply(event); true })` (or a queue drained in `poll`), and
//! the UI never blocks.

use dromon_core::{ConnStatus, Entry, Error, HostId, RemotePath};

/// What the UI asks of the service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Start a connection; answered with `Status` changes and then `Opened`.
    Connect { host: HostId },
    /// List a folder; answered with `Listing` or `ListFailed`.
    List { host: HostId, path: RemotePath },
    /// Part of the contract the service implements; no UI sends it yet.
    #[allow(dead_code)]
    Disconnect { host: HostId },
}

/// What the service tells the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceEvent {
    Status { host: HostId, status: ConnStatus },
    /// The session is up and its starting folder resolved (home, or the last folder used).
    Opened { host: HostId, start: RemotePath },
    Listing { host: HostId, path: RemotePath, entries: Vec<Entry> },
    ListFailed { host: HostId, path: RemotePath, error: Error },
}

/// Where commands go. `send` may answer at once with events (the mock does); a threaded
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
