//! Toasts: small notes stacked at the bottom centre of the window that block nothing. A delete
//! shows one while it runs ("Deleting 3 items…", then "Deleting… 1,204 of 5,005" once it knows
//! how many entries there are); it goes when the delete is done, and stays, with a dismiss
//! button, when it failed. [`Toasts`] also holds plain notes for whatever else needs one (M4's
//! "/var/www/old was removed"), and each file being edited in a local editor has at most one of
//! its own ("Saved notes.txt to prod", or a failed save that offers Retry; `remote_edit.rs`).

use dromon_core::Error;

use super::ops::OpView;
use super::remote_edit::EditId;
use super::service::{FileOpKind, OpId};

#[cfg(test)]
#[path = "toast_tests.rs"]
mod tests;

/// Which toast, for its key and its dismiss button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToastId {
    /// A file operation's progress or failure.
    Op(OpId),
    /// A note pushed with [`Toasts::push`].
    Note(u64),
}

impl ToastId {
    pub fn key(self) -> String {
        match self {
            ToastId::Op(id) => format!("toast-op-{}", id.0),
            ToastId::Note(n) => format!("toast-note-{n}"),
        }
    }
}

/// How a toast looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Something running: a spinner.
    Busy,
    /// Something that went wrong: a warning glyph and a dismiss button.
    Error,
    /// A note: a dismiss button.
    Note,
}

/// One toast as the view draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub id: ToastId,
    pub text: String,
    pub tone: Tone,
    /// A failed save of this file being edited: the toast's Retry uploads it again.
    pub retry: Option<EditId>,
}

impl Toast {
    pub fn dismissible(&self) -> bool {
        self.tone != Tone::Busy
    }
}

/// One note on screen; `edit` is the file being edited it's about, if any.
#[derive(Clone, Debug)]
struct Note {
    n: u64,
    text: String,
    tone: Tone,
    edit: Option<EditId>,
    retry: bool,
}

/// Plain notes, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Toasts {
    notes: Vec<Note>,
    last: u64,
}

impl Toasts {
    /// Shows `text` until it's dismissed.
    pub fn push(&mut self, text: impl Into<String>) -> ToastId {
        self.last += 1;
        self.notes.push(Note { n: self.last, text: text.into(), tone: Tone::Note, edit: None, retry: false });
        ToastId::Note(self.last)
    }

    /// Shows `text` about `edit` in place of what its toast said before (a new note, so it's
    /// announced again), as a note or an error (`tone`); `retry`: a failed save, which its
    /// Retry uploads again. Returns the note's number.
    pub fn put_edit(&mut self, edit: EditId, text: impl Into<String>, tone: Tone, retry: bool) -> u64 {
        self.forget_edit(edit);
        self.last += 1;
        self.notes.push(Note { n: self.last, text: text.into(), tone, edit: Some(edit), retry });
        self.last
    }

    /// `edit` ended or moved on: its toast goes.
    pub fn forget_edit(&mut self, edit: EditId) {
        self.notes.retain(|n| n.edit != Some(edit));
    }

    /// Whether note `n` is on screen.
    pub fn shows(&self, n: u64) -> bool {
        self.notes.iter().any(|note| note.n == n)
    }

    pub fn dismiss(&mut self, n: u64) {
        self.notes.retain(|note| note.n != n);
    }

    pub fn notes(&self) -> impl Iterator<Item = Toast> + '_ {
        self.notes.iter().map(|note| Toast {
            id: ToastId::Note(note.n),
            text: note.text.clone(),
            tone: note.tone,
            retry: note.edit.filter(|_| note.retry),
        })
    }
}

/// "‘name’" for one item, "3 items" for more.
fn what(names: &[String], count: usize) -> String {
    match names {
        [one] if count == 1 => format!("‘{one}’"),
        _ => format!("{} items", crate::fmt::thousands(count as u64)),
    }
}

/// Why a delete stopped, as the toast says it.
fn reason(error: &Error) -> String {
    error.to_string()
}

/// A delete's toast, from its op and the names it was asked to delete; `None` once it's done.
pub fn delete_toast(op: &OpView, names: &[String]) -> Option<Toast> {
    if op.kind != FileOpKind::Delete || op.succeeded() {
        return None;
    }
    let id = ToastId::Op(op.id);
    if let Some(error) = &op.error {
        let text = format!("Couldn't delete {}: {}", what(names, op.items), reason(error));
        return Some(Toast { id, text, tone: Tone::Error, retry: None });
    }
    let text = match op.total {
        None => format!("Deleting {}…", what(names, op.items)),
        Some(total) => {
            format!("Deleting… {} of {}", crate::fmt::thousands(op.done.min(total)), crate::fmt::thousands(total))
        }
    };
    Some(Toast { id, text, tone: Tone::Busy, retry: None })
}
