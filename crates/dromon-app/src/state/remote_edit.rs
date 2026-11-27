//! Edit in a local editor (§11): a remote text file opens in the app the system uses for its
//! type, and every save there uploads it back.
//!
//! Edit (a file row's menu, the palette's "Edit in local editor", ⌘E) is offered for the regular
//! files quick preview reads as text ([`App::editable`], by `preview::plan`), while the host is
//! connected. The service downloads the file into a private folder of the app's cache, watches
//! the copy and uploads each save, checking the server's copy first (`service/remote_edit.rs`);
//! the fixture backend makes all of it up (`fixture/remote_edit.rs`). The window that started
//! the edit opens the copy ([`Effect::OpenFile`]); Edit on a file being edited already opens its
//! copy again.
//!
//! Each file being edited is a [`Session`] whose [`Phase`] moves by the pure [`step`]:
//! Downloading → Open → Uploading → Open, Conflict or Failed, and Closed once it's stopped, its
//! host disconnected or its download failed. A conflict (the server's copy changed since the
//! edit downloaded or last uploaded it) waits for its sheet (Replace, Keep both, Discard my
//! changes; [`App::edit_conflict_on_screen`]); a failed upload keeps the copy and its toast
//! offers Retry. Every window follows every session from the service's events: the transfers
//! popover's Editing section lists them, with Stop editing. Quitting while a change isn't on the
//! server yet asks first ([`Edits::unsaved`]).
//!
//! A save's toast ("Saved notes.txt to prod") goes by itself after [`SAVED_FOR`] seconds on the
//! window's clock ([`App::remote_edit_tick`], from the view's poll); a failure's stays until
//! it's dismissed or retried. What a file holds never reaches the UI.

use std::path::PathBuf;

use dromon_core::{EntryKind, Error, HostId, RemotePath};
use dromon_transfer::Side;

use super::preview::{Plan, plan};
use super::service::Command;
use super::toast::Tone;
use super::transfers::Stepped;
use super::{App, Effect};

#[path = "remote_edit_types.rs"]
mod types;
pub use types::{EditChoice, EditId, EditStage, RemoteEditCommand, RemoteEditEvent, trace_line};

#[cfg(test)]
#[path = "remote_edit_tests.rs"]
mod tests;

/// Edit's chord: ⌘E (Ctrl+E elsewhere).
pub const EDIT_CHORD: &str = "mod+e";
/// How long a save's toast stays, in seconds.
pub const SAVED_FOR: f64 = 4.0;

/// Where one file being edited stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Its download runs (only the window that started it knows of it yet).
    Downloading,
    /// The local copy is open in the user's app, watched for saves.
    Open,
    /// A save is being checked and uploaded.
    Uploading,
    /// The server's copy changed since: the sheet asks what to do.
    Conflict { server: Side, local: Side },
    /// The sheet was answered: the service carries `choice` out.
    Resolving { choice: EditChoice, server: Side, local: Side },
    /// The last upload failed; the copy keeps the change, and Retry uploads it again.
    Failed { error: Error },
    /// Over: stopped, disconnected, or its download failed.
    Closed,
}

/// One file being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: EditId,
    pub host: HostId,
    /// The file on the server the edit uploads to.
    pub path: RemotePath,
    /// The local copy, once it's downloaded.
    pub local: Option<PathBuf>,
    pub phase: Phase,
}

impl Session {
    /// The file's name.
    pub fn name(&self) -> &str {
        self.path.file_name().unwrap_or_default()
    }

    /// Whether a change of the local copy isn't on the server (yet): an upload running, failed
    /// or waiting for the conflict sheet. Quitting now loses it.
    pub fn unsaved(&self) -> bool {
        match &self.phase {
            Phase::Uploading | Phase::Conflict { .. } | Phase::Failed { .. } => true,
            Phase::Resolving { choice, .. } => *choice != EditChoice::Discard,
            Phase::Downloading | Phase::Open | Phase::Closed => false,
        }
    }
}

/// What happens to a session: the service's news, or the user.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Opened {
        local: PathBuf,
    },
    Uploading,
    Saved {
        to: RemotePath,
    },
    Conflict {
        server: Side,
        local: Side,
    },
    Reloaded,
    Failed {
        stage: EditStage,
        error: Error,
    },
    /// The service ended it.
    Closed,
    /// Stop editing.
    Stop,
    /// The conflict sheet's answer.
    Resolve(EditChoice),
    /// The failed upload's toast's Retry.
    Retry,
}

/// What a session's owner says about it.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// "Saved notes.txt to prod": `to` is where it went.
    Saved {
        to: RemotePath,
    },
    SaveFailed(Error),
    /// The download failed: the edit never started.
    OpenFailed(Error),
    Reloaded,
    ReloadFailed(Error),
}

/// What a step asks its owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Fx {
    Send(RemoteEditCommand),
    /// Open the local copy in the user's app.
    OpenEditor(PathBuf),
    Note(Note),
}

/// Steps `s` by `ev`: what its owner does next. Anything that doesn't fit the phase (a stale or
/// repeated event) changes nothing; nothing moves a session out of Closed.
pub fn step(s: &mut Session, ev: Event) -> Vec<Fx> {
    use Phase as P;
    let mut fx = Vec::new();
    let (id, host) = (s.id, s.host.clone());
    let phase = std::mem::replace(&mut s.phase, P::Closed);
    s.phase = match (phase, ev) {
        (P::Closed, _) => P::Closed,
        (_, Event::Stop) => {
            fx.push(Fx::Send(RemoteEditCommand::Stop { id, host }));
            P::Closed
        }
        (_, Event::Closed) => P::Closed,
        (P::Downloading, Event::Opened { local }) => {
            s.local = Some(local.clone());
            fx.push(Fx::OpenEditor(local));
            P::Open
        }
        (P::Downloading, Event::Failed { error, .. }) => {
            fx.push(Fx::Note(Note::OpenFailed(error)));
            P::Closed
        }
        (P::Open | P::Failed { .. }, Event::Uploading) => P::Uploading,
        (P::Open | P::Uploading | P::Resolving { .. }, Event::Saved { to }) => {
            fx.push(Fx::Note(Note::Saved { to }));
            P::Open
        }
        (P::Open | P::Uploading | P::Failed { .. }, Event::Conflict { server, local }) => P::Conflict { server, local },
        (P::Conflict { server, local }, Event::Resolve(choice)) => {
            fx.push(Fx::Send(RemoteEditCommand::Resolve { id, host, choice }));
            P::Resolving { choice, server, local }
        }
        (P::Resolving { .. }, Event::Reloaded) => {
            fx.push(Fx::Note(Note::Reloaded));
            P::Open
        }
        (P::Resolving { server, local, .. }, Event::Failed { stage: EditStage::Reload, error }) => {
            fx.push(Fx::Note(Note::ReloadFailed(error)));
            P::Conflict { server, local }
        }
        (P::Uploading | P::Resolving { .. }, Event::Failed { stage: EditStage::Upload, error }) => {
            fx.push(Fx::Note(Note::SaveFailed(error.clone())));
            P::Failed { error }
        }
        (P::Failed { .. }, Event::Retry) => {
            fx.push(Fx::Send(RemoteEditCommand::Retry { id, host }));
            P::Uploading
        }
        (phase, _) => phase,
    };
    fx
}

/// When a save's toast goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hide {
    /// It just showed; the next tick starts its time (actions and events have no clock).
    Fresh,
    /// It goes at this time, on the window's clock.
    At(f64),
}

/// The files being edited, as one window knows them.
#[derive(Clone, Debug, Default)]
pub struct Edits {
    /// Oldest first.
    pub sessions: Vec<Session>,
    last: u64,
    /// What the service's news asked of the view (opening a copy); [`App::take_edit_effects`].
    pending: Vec<Effect>,
    /// Save toasts (their note numbers) that go by themselves.
    hide: Vec<(u64, Hide)>,
}

impl Edits {
    /// What a new main window (⌘N) starts with: the same sessions but the downloads under way
    /// (it hears of those once their copies are open; only their own window opens them).
    pub fn for_window(&self) -> Edits {
        let sessions = self.sessions.iter().filter(|s| s.phase != Phase::Downloading).cloned().collect();
        Edits { sessions, ..Edits::default() }
    }

    pub fn get(&self, id: EditId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// The session editing `path` of `host`, if there is one.
    pub fn of(&self, host: &HostId, path: &RemotePath) -> Option<&Session> {
        self.sessions.iter().find(|s| &s.host == host && &s.path == path)
    }

    /// How many files have a change that isn't on the server yet ([`Session::unsaved`]).
    pub fn unsaved(&self) -> usize {
        self.sessions.iter().filter(|s| s.unsaved()).count()
    }
}

/// The user's side of editing (`Action::RemoteEdit`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteEditAction {
    /// Edit the row of the folder on screen; `None`: the cursor's row (⌘E).
    Edit(Option<String>),
    /// The Editing section's Stop editing.
    Stop(EditId),
    /// The conflict sheet's buttons, for the conflict on screen.
    Resolve(EditChoice),
    /// A failed upload's Retry.
    Retry(EditId),
}

impl App {
    /// Whether Edit applies to the row `name` of the folder on screen: a regular file quick
    /// preview reads as text (`preview::plan`), while the host is connected and the folder (not
    /// search results) is on screen.
    pub fn editable(&self, name: &str) -> bool {
        let Some(c) = self.conn().filter(|c| c.interactive()) else { return false };
        if self.results().is_some() || self.on_settings() {
            return false;
        }
        c.find(name).is_some_and(|e| e.kind == EntryKind::File && matches!(plan(&c.cwd, e), Plan::Text(_)))
    }

    /// The edit actions of [`App::act`].
    pub(super) fn remote_edit_action(&mut self, action: RemoteEditAction, fx: &mut Vec<Effect>) {
        match action {
            RemoteEditAction::Edit(row) => self.start_edit(row, fx),
            RemoteEditAction::Stop(id) => self.edit_event(id, Event::Stop),
            RemoteEditAction::Resolve(choice) => {
                if let Some(id) = self.edit_conflict_on_screen().map(|s| s.id) {
                    self.edit_event(id, Event::Resolve(choice));
                }
            }
            RemoteEditAction::Retry(id) => self.edit_event(id, Event::Retry),
        }
        // A fixture answers inside the send: what its news asked of the view goes with this.
        fx.append(&mut self.edits.pending);
    }

    /// Edit on `row` (or the cursor's row): a new session, or the copy of one there is already.
    fn start_edit(&mut self, row: Option<String>, fx: &mut Vec<Effect>) {
        let Some(name) = row.or_else(|| self.cursor_row()).filter(|n| self.editable(n)) else { return };
        let Some(c) = self.conn() else { return };
        let (host, path) = (c.host.clone(), c.cwd.join(&name));
        if let Some(s) = self.edits.of(&host, &path) {
            fx.extend(s.local.clone().map(Effect::OpenFile));
            return;
        }
        self.edits.last += 1;
        let id = EditId((u64::from(self.window.0) << 32) + self.edits.last);
        let session = Session { id, host: host.clone(), path: path.clone(), local: None, phase: Phase::Downloading };
        self.edits.sessions.push(session);
        self.send(Command::RemoteEdit(RemoteEditCommand::Start { id, host, path }));
    }

    /// The service's news about an edit. An edit this window didn't start joins it once its copy
    /// is open; other news of an edit it doesn't know is stale.
    pub(super) fn remote_edit_event(&mut self, ev: RemoteEditEvent) {
        let (id, event) = match ev {
            RemoteEditEvent::Opened { id, host, path, local } => {
                if self.edits.get(id).is_none() {
                    let phase = Phase::Open;
                    self.edits.sessions.push(Session { id, host, path, local: Some(local), phase });
                    return;
                }
                (id, Event::Opened { local })
            }
            RemoteEditEvent::Uploading { id, .. } => (id, Event::Uploading),
            RemoteEditEvent::Saved { id, to, .. } => (id, Event::Saved { to }),
            RemoteEditEvent::Conflict { id, server, local, .. } => (id, Event::Conflict { server, local }),
            RemoteEditEvent::Reloaded { id, .. } => (id, Event::Reloaded),
            RemoteEditEvent::Failed { id, stage, error, .. } => (id, Event::Failed { stage, error }),
            RemoteEditEvent::Closed { id, .. } => (id, Event::Closed),
        };
        self.edit_event(id, event);
    }

    /// Steps the session `id` and carries out what it asks. A session that closed goes, and its
    /// toast with it (a failed download's note stays); commands go last, as a fixture answers
    /// them at once.
    fn edit_event(&mut self, id: EditId, ev: Event) {
        let Some(at) = self.edits.sessions.iter().position(|s| s.id == id) else { return };
        let ended = matches!(ev, Event::Stop | Event::Closed);
        let s = &mut self.edits.sessions[at];
        let out = step(s, ev);
        let (name, host) = (s.name().to_owned(), s.host.clone());
        if s.phase == Phase::Closed {
            self.edits.sessions.remove(at);
            if ended {
                self.files.toasts.forget_edit(id);
            }
        }
        let mut sends = Vec::new();
        for fx in out {
            match fx {
                Fx::Send(cmd) => sends.push(cmd),
                Fx::OpenEditor(local) => self.edits.pending.push(Effect::OpenFile(local)),
                Fx::Note(note) => self.edit_note(id, &name, &host, note),
            }
        }
        if matches!(self.edits.get(id).map(|s| &s.phase), Some(Phase::Uploading)) {
            // Saving again (a retry): the failure's toast has had its say.
            self.files.toasts.forget_edit(id);
        }
        for cmd in sends {
            self.send(Command::RemoteEdit(cmd));
        }
    }

    /// The toast for `note` about `name` on `host`.
    fn edit_note(&mut self, id: EditId, name: &str, host: &HostId, note: Note) {
        let host = self.hosts.get(host).map_or(host.as_str(), |h| h.display_name()).to_owned();
        let toasts = &mut self.files.toasts;
        match note {
            Note::Saved { to } => {
                let saved = to.file_name().unwrap_or(name);
                let n = toasts.put_edit(id, format!("Saved {saved} to {host}"), Tone::Note, false);
                self.edits.hide.push((n, Hide::Fresh));
            }
            Note::SaveFailed(e) => {
                toasts.put_edit(id, format!("Couldn't save {name} to {host}: {e}"), Tone::Error, true);
            }
            Note::OpenFailed(e) => {
                toasts.push(format!("Couldn't open {name} for editing: {e}"));
            }
            Note::Reloaded => {
                let n = toasts.put_edit(id, format!("Discarded your changes to {name}"), Tone::Note, false);
                self.edits.hide.push((n, Hide::Fresh));
            }
            Note::ReloadFailed(e) => {
                let text = format!("Couldn't download {name} from {host} again: {e}");
                toasts.put_edit(id, text, Tone::Error, false);
            }
        }
    }

    /// What the service's edit news asked of the view since it last looked (`RuneSink` carries
    /// it out after each event): opening a local copy.
    pub fn take_edit_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.edits.pending)
    }

    /// The edit conflict the sheet asks about: the oldest, while no prompt sheet, no transfer
    /// conflict, the transfers popover (whose row offers to answer it) or the palette is up.
    pub fn edit_conflict_on_screen(&self) -> Option<&Session> {
        if self.ui.sheets.is_open()
            || self.transfers.open
            || self.ui.palette.is_open()
            || self.conflict_on_screen().is_some()
        {
            return None;
        }
        self.edits.sessions.iter().find(|s| matches!(s.phase, Phase::Conflict { .. }))
    }

    /// The view's poll, every frame at `clock`: save toasts that have been up [`SAVED_FOR`]
    /// go. Says whether one went, and when the next is due.
    pub fn remote_edit_tick(&mut self, clock: f64) -> Stepped {
        let mut out = Stepped::default();
        let mut gone = Vec::new();
        for (n, hide) in &mut self.edits.hide {
            match *hide {
                Hide::Fresh => *hide = Hide::At(clock + SAVED_FOR),
                Hide::At(at) if clock >= at => gone.push(*n),
                Hide::At(_) => {}
            }
        }
        // A toast the edit's next one replaced is gone already.
        let toasts = &self.files.toasts;
        self.edits.hide.retain(|(n, _)| !gone.contains(n) && toasts.shows(*n));
        for n in gone {
            out.changed |= self.files.toasts.shows(n);
            self.files.toasts.dismiss(n);
        }
        out.wake = self
            .edits
            .hide
            .iter()
            .map(|(_, h)| match h {
                Hide::At(at) => *at,
                Hide::Fresh => clock,
            })
            .reduce(f64::min);
        out
    }
}
