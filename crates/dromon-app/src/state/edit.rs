//! The inline name field (§5 M3): a new folder's row at the top of the list, or a row being
//! renamed. One explicit machine, one edit at a time: [`Edit::Idle`] → [`Edit::Editing`] (the
//! field is up, maybe with an error under it) → [`Edit::Sending`] (the request runs, the field
//! is read-only) → back to `Idle` on Done, or to `Editing` with the server's error.
//!
//! [`step`] is pure: it says what to send ([`EditEffect::Send`]) and what to select once it's
//! done ([`EditEffect::Finished`]); the app hands out the operation's id ([`EditEvent::Sent`])
//! before sending, since a fixture answers from inside the send.
//!
//! Names are checked here before anything is sent: empty, `.`, `..`, and names with `/` or NUL
//! get an inline error and no request.

use dromon_core::{Error, HostId, RemotePath};
use rune::TextEdit;

use super::service::{FileOp, OpId};

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;

/// What the field names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// A folder that doesn't exist yet.
    NewFolder,
    /// The row `from`, renamed within its folder.
    Rename { from: String },
}

/// The field and what it's for.
#[derive(Clone, Debug)]
pub struct Field {
    pub host: HostId,
    /// The folder on screen when it started (the new folder's parent, the renamed row's folder).
    pub folder: RemotePath,
    pub kind: EditKind,
    pub edit: TextEdit,
    /// What's wrong with the name, shown under the field.
    pub error: Option<String>,
    /// How many errors it has shown: each one shakes the field once more.
    pub shakes: u32,
}

impl Field {
    /// A new folder's field, prefilled with `name` (a free one) and all of it selected.
    pub fn new_folder(host: HostId, folder: RemotePath, name: &str) -> Field {
        Field::with(host, folder, EditKind::NewFolder, name, name.len())
    }

    /// A rename's field: the name, with the stem selected for a file (`report.final.pdf` →
    /// `report.final`) and all of it for a folder.
    pub fn rename(host: HostId, folder: RemotePath, name: &str, is_dir: bool) -> Field {
        let end = if is_dir { name.len() } else { stem_len(name) };
        Field::with(host, folder, EditKind::Rename { from: name.to_owned() }, name, end)
    }

    fn with(host: HostId, folder: RemotePath, kind: EditKind, name: &str, end: usize) -> Field {
        let mut edit = TextEdit::new(name);
        edit.set_selection(0, end);
        edit.focused = true;
        Field { host, folder, kind, edit, error: None, shakes: 0 }
    }

    /// The row this field stands in for: the renamed row (`None` for a new folder).
    pub fn renaming(&self) -> Option<&str> {
        match &self.kind {
            EditKind::Rename { from } => Some(from),
            EditKind::NewFolder => None,
        }
    }

    fn fail(&mut self, message: String) {
        self.error = Some(message);
        self.shakes += 1;
        self.edit.focused = true;
    }
}

/// The byte length of a file name's stem: up to its last dot, unless that dot starts the name
/// (`.env`) or ends it (`file.`), which have no extension.
pub fn stem_len(name: &str) -> usize {
    match name.rfind('.') {
        Some(0) | None => name.len(),
        Some(i) if i + 1 == name.len() => name.len(),
        Some(i) => i,
    }
}

/// Why `name` can't name an item, before asking the server.
pub fn check_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        Err("Type a name.")
    } else if name == "." || name == ".." {
        Err("“.” and “..” can't be used as names.")
    } else if name.contains('/') {
        Err("A name can't contain “/”.")
    } else if name.contains('\0') {
        Err("A name can't contain a NUL character.")
    } else {
        Ok(())
    }
}

/// What the server's refusal says under the field.
pub fn error_text(name: &str, error: &Error) -> String {
    match error {
        Error::AlreadyExists(_) => format!("‘{name}’ already exists here"),
        other => other.to_string(),
    }
}

/// Where the field is.
#[derive(Clone, Debug, Default)]
pub enum Edit {
    #[default]
    Idle,
    /// Up and taking text.
    Editing(Box<Field>),
    /// Submitted: the request for it runs (its id once the app has it), the field read-only.
    Sending(Box<Field>, Option<OpId>),
}

/// What happens to the field.
#[derive(Clone, Debug)]
pub enum EditEvent {
    /// A new folder or rename starts (replacing a field still being edited; never one that's
    /// sending).
    Start(Box<Field>),
    /// Return.
    Submit,
    /// The text changed: an error from before goes.
    Changed,
    /// Escape.
    Cancel,
    /// The field lost focus: as Escape when nothing was typed, else as Return.
    Blur,
    /// The request went out with this id.
    Sent(OpId),
    Done(OpId),
    Failed(OpId, Error),
    /// Its folder or row went away (navigated off, the connection dropped, a relisting
    /// removed the row): it closes.
    Gone,
}

/// What the app does for a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditEffect {
    /// Send this operation (then report its id with [`EditEvent::Sent`]).
    Send { host: HostId, op: FileOp },
    /// It's done: select `name` in `folder` once it lists.
    Finished { host: HostId, folder: RemotePath, name: String },
}

impl Edit {
    pub fn field(&self) -> Option<&Field> {
        match self {
            Edit::Idle => None,
            Edit::Editing(f) | Edit::Sending(f, _) => Some(f),
        }
    }

    pub fn field_mut(&mut self) -> Option<&mut Field> {
        match self {
            Edit::Idle => None,
            Edit::Editing(f) | Edit::Sending(f, _) => Some(f),
        }
    }

    pub fn is_sending(&self) -> bool {
        matches!(self, Edit::Sending(..))
    }
}

/// The name a field asks for, as typed.
fn request(f: &Field) -> Result<FileOp, &'static str> {
    let name = f.edit.text.as_str();
    check_name(name)?;
    Ok(match &f.kind {
        EditKind::NewFolder => FileOp::Mkdir { path: f.folder.join(name) },
        EditKind::Rename { from } => FileOp::Rename { from: f.folder.join(from), to: f.folder.join(name) },
    })
}

/// Moves the field along. Pure: everything it does is in `edit` and the returned effect.
pub fn step(edit: &mut Edit, ev: EditEvent) -> Option<EditEffect> {
    match (std::mem::take(edit), ev) {
        (Edit::Sending(f, id), EditEvent::Start(_)) => {
            *edit = Edit::Sending(f, id);
            None
        }
        (_, EditEvent::Start(f)) => {
            *edit = Edit::Editing(f);
            None
        }
        (Edit::Editing(f), EditEvent::Blur) if f.edit.is_pristine() => None,
        (Edit::Editing(mut f), EditEvent::Submit | EditEvent::Blur) => {
            if matches!(&f.kind, EditKind::Rename { from } if *from == f.edit.text) {
                return None;
            }
            match request(&f) {
                Ok(op) => {
                    f.error = None;
                    let host = f.host.clone();
                    *edit = Edit::Sending(f, None);
                    Some(EditEffect::Send { host, op })
                }
                Err(message) => {
                    f.fail(message.to_owned());
                    *edit = Edit::Editing(f);
                    None
                }
            }
        }
        (Edit::Editing(mut f), EditEvent::Changed) => {
            f.error = None;
            *edit = Edit::Editing(f);
            None
        }
        (Edit::Sending(f, None), EditEvent::Sent(id)) => {
            *edit = Edit::Sending(f, Some(id));
            None
        }
        (Edit::Sending(f, Some(id)), EditEvent::Done(done)) if id == done => {
            let name = f.edit.text.clone();
            Some(EditEffect::Finished { host: f.host, folder: f.folder, name })
        }
        (Edit::Sending(mut f, Some(id)), EditEvent::Failed(failed, error)) if id == failed => {
            let message = error_text(&f.edit.text, &error);
            f.fail(message);
            *edit = Edit::Editing(f);
            None
        }
        (Edit::Idle | Edit::Editing(_) | Edit::Sending(..), EditEvent::Cancel | EditEvent::Gone) => None,
        // Anything else changes nothing: put it back.
        (before, _) => {
            *edit = before;
            None
        }
    }
}
