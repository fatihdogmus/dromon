//! The window's sheets (§6.8): what ssh asks (sign in, a new host key, a yes/no question), the
//! connection log, the delete confirmation and Add to SSH config. One sheet shows at a time; prompts that arrive meanwhile wait in order.
//! Notices (ssh's "touch your security key") don't block anything: they show as toasts until
//! the service closes them.
//!
//! What the user types for a prompt lives only in the open sheet's [`TextEdit`]. Submitting moves
//! the text out into the answer (no copy stays behind) and the edit is wiped and dropped with the
//! sheet; cancelling wipes it.

use std::collections::VecDeque;

use dromon_core::{HostId, Prompt, PromptAnswer, PromptId, PromptKind, RemotePath};
use rune::TextEdit;

use super::hostbook::AddToConfigView;

/// The sheet on screen.
pub enum Sheet {
    /// Waiting for the user. `edit` holds what they type, for the kinds that take text.
    Ask { prompt: Prompt, edit: Box<TextEdit> },
    /// Answered, and ssh hasn't moved on yet. The sheet stays up (its button busy) until this
    /// host's next prompt replaces it, so a wrong password re-asks in place with its error
    /// instead of the sheet flashing away and back, or until the connection settles.
    Sent { prompt: Prompt },
    /// One host's connection log.
    Log { host: HostId },
    /// "Delete 3 items permanently?": the rows `names` of `folder`. Return doesn't delete;
    /// only its Delete button does.
    Delete { host: HostId, folder: RemotePath, names: Vec<String> },
    /// "Add to SSH config": a saved host's block, where it goes and the backup (`hostbook.rs`).
    AddToConfig(Box<AddToConfigView>),
}

impl Sheet {
    pub fn host(&self) -> &HostId {
        match self {
            Sheet::Ask { prompt, .. } | Sheet::Sent { prompt } => &prompt.host,
            Sheet::Log { host } | Sheet::Delete { host, .. } => host,
            Sheet::AddToConfig(v) => &v.host,
        }
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        match self {
            Sheet::Ask { prompt, .. } | Sheet::Sent { prompt } => Some(prompt),
            Sheet::Log { .. } | Sheet::Delete { .. } | Sheet::AddToConfig(_) => None,
        }
    }
}

/// Whether a prompt of this kind is answered by typing.
pub fn takes_text(kind: &PromptKind) -> bool {
    matches!(kind, PromptKind::Password | PromptKind::Passphrase { .. } | PromptKind::Other)
}

/// The open sheet, the prompts waiting behind it, and the notices on screen.
#[derive(Default)]
pub struct Sheets {
    open: Option<Sheet>,
    queue: VecDeque<Prompt>,
    notices: Vec<Prompt>,
    /// What [`Sheets::edit_mut`] hands out when no sheet takes text (a view's accessor must
    /// return some edit); always empty.
    spare: TextEdit,
}

impl Sheets {
    pub fn open(&self) -> Option<&Sheet> {
        self.open.as_ref()
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Prompts waiting behind the open sheet.
    #[cfg(test)]
    pub fn queued(&self) -> impl ExactSizeIterator<Item = &Prompt> + '_ {
        self.queue.iter()
    }

    pub fn notices(&self) -> &[Prompt] {
        &self.notices
    }

    /// The open sheet's field, for `text_input(..).on_edit`.
    pub fn edit_mut(&mut self) -> &mut TextEdit {
        match &mut self.open {
            Some(Sheet::Ask { edit, .. }) => edit.as_mut(),
            _ => {
                self.spare.clear();
                &mut self.spare
            }
        }
    }

    /// What the open sheet's field shows: its edit while asking, else an empty one.
    pub fn field(&self) -> &TextEdit {
        match &self.open {
            Some(Sheet::Ask { edit, .. }) => edit,
            _ => &self.spare,
        }
    }

    /// The open sheet's field text, if it has one.
    #[cfg(test)]
    pub fn edit(&self) -> Option<&TextEdit> {
        match &self.open {
            Some(Sheet::Ask { edit, prompt }) if takes_text(&prompt.kind) => Some(edit.as_ref()),
            _ => None,
        }
    }

    /// ssh asked something.
    pub fn on_prompt(&mut self, p: Prompt) {
        if p.kind == PromptKind::Notice {
            if !self.notices.iter().any(|n| n.id == p.id) {
                self.notices.push(p);
            }
            return;
        }
        // A question from ssh comes before the log or a delete still being asked about (whose
        // host can't take it while it signs in anyway).
        let replaces = match &self.open {
            None | Some(Sheet::Log { .. } | Sheet::Delete { .. } | Sheet::AddToConfig(_)) => true,
            Some(Sheet::Sent { prompt }) => prompt.host == p.host,
            Some(Sheet::Ask { .. }) => false,
        };
        if replaces {
            self.open = Some(ask(p));
        } else {
            self.queue.push_back(p);
        }
    }

    /// The prompt went away without an answer (ssh hung up, the notice ended).
    pub fn on_closed(&mut self, id: PromptId) {
        self.notices.retain(|n| n.id != id);
        self.queue.retain(|p| p.id != id);
        if matches!(&self.open, Some(Sheet::Ask { prompt, .. }) if prompt.id == id) {
            self.close_and_advance();
        }
    }

    /// `host`'s connection settled (up, failed or disconnected): an answered sheet for it closes.
    pub fn on_settled(&mut self, host: &HostId) {
        if matches!(&self.open, Some(Sheet::Sent { prompt }) if &prompt.host == host) {
            self.close_and_advance();
        }
    }

    /// The primary button (Connect, Trust and connect, Yes): the answer to send, and the sheet
    /// waits for ssh's next move.
    pub fn submit(&mut self) -> Option<(PromptId, PromptAnswer)> {
        let Some(Sheet::Ask { prompt, edit }) = &mut self.open else { return None };
        let answer = match prompt.kind {
            PromptKind::HostKey { .. } | PromptKind::Confirm => PromptAnswer::Text("yes".into()),
            PromptKind::Password | PromptKind::Passphrase { .. } | PromptKind::Other => {
                // Move the buffer out rather than copy it, then wipe whatever the edit holds.
                let text = std::mem::take(&mut edit.text);
                edit.clear();
                PromptAnswer::Text(text)
            }
            PromptKind::Notice => return None,
        };
        let id = prompt.id;
        if let Some(Sheet::Ask { prompt, .. }) = self.open.take() {
            self.open = Some(Sheet::Sent { prompt });
        }
        Some((id, answer))
    }

    /// A yes/no question's No: answers `no` and closes the sheet. Other sheets cancel.
    pub fn decline(&mut self) -> Option<(PromptId, PromptAnswer)> {
        match &self.open {
            Some(Sheet::Ask { prompt, .. }) if prompt.kind == PromptKind::Confirm => {
                let id = prompt.id;
                self.close_and_advance();
                Some((id, PromptAnswer::Text("no".into())))
            }
            _ => self.cancel(),
        }
    }

    /// Cancel or Escape: an open question is cancelled (the answer to send); an answered one or
    /// the log just closes.
    pub fn cancel(&mut self) -> Option<(PromptId, PromptAnswer)> {
        let id = match &self.open {
            Some(Sheet::Ask { prompt, .. }) => Some(prompt.id),
            Some(Sheet::Sent { .. } | Sheet::Log { .. } | Sheet::Delete { .. } | Sheet::AddToConfig(_)) => None,
            None => return None,
        };
        self.close_and_advance();
        id.map(|id| (id, PromptAnswer::Cancel))
    }

    /// Opens `host`'s log, unless a prompt is showing.
    pub fn open_log(&mut self, host: HostId) -> bool {
        match &self.open {
            None | Some(Sheet::Log { .. }) => {
                self.open = Some(Sheet::Log { host });
                true
            }
            Some(_) => false,
        }
    }

    /// Asks whether to delete `names` from `folder`, unless another sheet is up.
    pub fn open_delete(&mut self, host: HostId, folder: RemotePath, names: Vec<String>) -> bool {
        if self.open.is_some() || names.is_empty() {
            return false;
        }
        self.open = Some(Sheet::Delete { host, folder, names });
        true
    }

    /// The delete sheet's Delete: what to delete, and the sheet closes.
    pub fn take_delete(&mut self) -> Option<(HostId, RemotePath, Vec<String>)> {
        if !matches!(self.open, Some(Sheet::Delete { .. })) {
            return None;
        }
        let Some(Sheet::Delete { host, folder, names }) = self.open.take() else { return None };
        self.open = self.queue.pop_front().map(ask);
        Some((host, folder, names))
    }

    /// Opens the Add to SSH config sheet, unless another sheet is up.
    pub fn open_add_to_config(&mut self, view: AddToConfigView) -> bool {
        if self.open.is_some() {
            return false;
        }
        self.open = Some(Sheet::AddToConfig(Box::new(view)));
        true
    }

    /// The Add to SSH config sheet, while it's open.
    pub fn add_to_config_mut(&mut self) -> Option<&mut AddToConfigView> {
        match &mut self.open {
            Some(Sheet::AddToConfig(v)) => Some(v),
            _ => None,
        }
    }

    /// `host`'s block was added: its sheet closes.
    pub fn close_add_to_config(&mut self, host: &HostId) {
        if matches!(&self.open, Some(Sheet::AddToConfig(v)) if &v.host == host) {
            self.close_and_advance();
        }
    }

    /// The user disconnected `host`: nothing of it stays on screen.
    pub fn forget_host(&mut self, host: &HostId) {
        self.queue.retain(|p| &p.host != host);
        self.notices.retain(|p| &p.host != host);
        let delete = matches!(&self.open, Some(Sheet::Delete { host: h, .. }) if h == host);
        if delete || self.open.as_ref().and_then(Sheet::prompt).is_some_and(|p| &p.host == host) {
            self.close_and_advance();
        }
    }

    /// Closes the open sheet (wiping its field) and shows the next waiting prompt.
    fn close_and_advance(&mut self) {
        if let Some(Sheet::Ask { mut edit, .. }) = self.open.take() {
            edit.clear();
        }
        self.open = self.queue.pop_front().map(ask);
    }
}

/// A fresh sheet for `p`: an empty field, focused, masked for secrets.
fn ask(p: Prompt) -> Sheet {
    let mut edit = TextEdit::new("");
    if p.kind.secret() {
        edit = edit.secure();
    }
    edit.focused = takes_text(&p.kind);
    Sheet::Ask { prompt: p, edit: Box::new(edit) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(s: &str) -> HostId {
        HostId(s.into())
    }

    fn prompt(id: u64, h: &str, kind: PromptKind) -> Prompt {
        Prompt { id: PromptId(id), host: host(h), kind, text: format!("prompt {id}"), error: None }
    }

    fn open_id(s: &Sheets) -> Option<u64> {
        s.open().and_then(Sheet::prompt).map(|p| p.id.0)
    }

    fn sent(s: &Sheets) -> bool {
        matches!(s.open(), Some(Sheet::Sent { .. }))
    }

    fn type_text(s: &mut Sheets, text: &str) {
        s.edit_mut().replace_text(text);
    }

    #[test]
    fn prompts_queue_in_order_one_sheet_at_a_time() {
        let mut s = Sheets::default();
        s.on_prompt(prompt(1, "a", PromptKind::Password));
        s.on_prompt(prompt(2, "b", PromptKind::Password));
        s.on_prompt(prompt(3, "c", PromptKind::Confirm));
        assert_eq!(open_id(&s), Some(1));
        assert_eq!(s.queued().len(), 2);
        assert_eq!(s.cancel(), Some((PromptId(1), PromptAnswer::Cancel)));
        assert_eq!(open_id(&s), Some(2));
        s.on_closed(PromptId(2));
        assert_eq!(open_id(&s), Some(3));
        s.on_closed(PromptId(3));
        assert!(!s.is_open());
    }

    #[test]
    fn submitting_moves_the_text_out_and_waits_for_ssh() {
        let mut s = Sheets::default();
        s.on_prompt(prompt(1, "a", PromptKind::Password));
        assert!(s.edit().is_some_and(|e| e.focused));
        type_text(&mut s, "hunter2");
        assert_eq!(s.submit(), Some((PromptId(1), PromptAnswer::Text("hunter2".into()))));
        assert!(sent(&s));
        assert!(s.edit().is_none());
        assert_eq!(s.submit(), None, "an answered sheet can't answer twice");
        // The same host asking again (wrong password) replaces the sheet in place.
        let mut retry = prompt(2, "a", PromptKind::Password);
        retry.error = Some("That password didn't work. Try again.".into());
        s.on_prompt(retry);
        assert_eq!(open_id(&s), Some(2));
        assert!(!sent(&s));
        assert_eq!(s.edit().map(|e| e.text.as_str()), Some(""));
        // Then it connects.
        type_text(&mut s, "right");
        s.submit();
        s.on_settled(&host("a"));
        assert!(!s.is_open());
    }

    #[test]
    fn an_answered_sheet_holds_other_hosts_prompts_until_it_settles() {
        let mut s = Sheets::default();
        s.on_prompt(prompt(1, "a", PromptKind::HostKey { key_type: None, fingerprint: None }));
        assert_eq!(s.submit(), Some((PromptId(1), PromptAnswer::Text("yes".into()))));
        s.on_prompt(prompt(2, "b", PromptKind::Password));
        assert!(sent(&s));
        s.on_settled(&host("b"));
        assert!(sent(&s), "another host settling changes nothing");
        s.on_settled(&host("a"));
        assert_eq!(open_id(&s), Some(2));
    }

    #[test]
    fn confirm_answers_yes_or_no() {
        let mut s = Sheets::default();
        s.on_prompt(prompt(1, "a", PromptKind::Confirm));
        s.on_prompt(prompt(2, "a", PromptKind::Confirm));
        assert_eq!(s.decline(), Some((PromptId(1), PromptAnswer::Text("no".into()))));
        assert_eq!(open_id(&s), Some(2));
        assert_eq!(s.submit(), Some((PromptId(2), PromptAnswer::Text("yes".into()))));
    }

    #[test]
    fn notices_are_toasts_until_closed() {
        let mut s = Sheets::default();
        s.on_prompt(prompt(1, "a", PromptKind::Notice));
        s.on_prompt(prompt(1, "a", PromptKind::Notice));
        assert!(!s.is_open());
        assert_eq!(s.notices().len(), 1);
        s.on_closed(PromptId(1));
        assert!(s.notices().is_empty());
    }

    #[test]
    fn closing_a_queued_prompt_drops_it() {
        let mut s = Sheets::default();
        s.on_prompt(prompt(1, "a", PromptKind::Password));
        s.on_prompt(prompt(2, "b", PromptKind::Password));
        s.on_closed(PromptId(2));
        s.cancel();
        assert!(!s.is_open());
    }

    #[test]
    fn the_log_gives_way_to_prompts() {
        let mut s = Sheets::default();
        assert!(s.open_log(host("a")));
        s.on_prompt(prompt(1, "a", PromptKind::Password));
        assert_eq!(open_id(&s), Some(1));
        assert!(!s.open_log(host("a")), "no log over a question");
        s.cancel();
        assert!(s.open_log(host("a")));
        assert_eq!(s.cancel(), None);
        assert!(!s.is_open());
    }

    #[test]
    fn disconnecting_forgets_the_hosts_prompts() {
        let mut s = Sheets::default();
        s.on_prompt(prompt(1, "a", PromptKind::Password));
        s.on_prompt(prompt(2, "a", PromptKind::Other));
        s.on_prompt(prompt(3, "b", PromptKind::Password));
        s.on_prompt(prompt(4, "a", PromptKind::Notice));
        s.forget_host(&host("a"));
        assert_eq!(open_id(&s), Some(3));
        assert_eq!(s.queued().len(), 0);
        assert!(s.notices().is_empty());
    }

    #[test]
    fn the_delete_sheet_asks_once_cancels_quietly_and_gives_way_to_prompts() {
        let mut s = Sheets::default();
        let folder = RemotePath::new("/w");
        assert!(!s.open_delete(host("a"), folder.clone(), Vec::new()), "nothing to delete");
        assert!(s.open_delete(host("a"), folder.clone(), vec!["x".into()]));
        assert!(!s.open_delete(host("a"), folder.clone(), vec!["y".into()]), "one sheet at a time");
        assert_eq!(s.cancel(), None, "nothing to answer");
        assert!(!s.is_open());
        assert_eq!(s.take_delete(), None);

        s.open_delete(host("a"), folder.clone(), vec!["x".into(), "y".into()]);
        assert_eq!(s.take_delete(), Some((host("a"), folder.clone(), vec!["x".into(), "y".into()])));
        assert!(!s.is_open());

        s.open_delete(host("a"), folder.clone(), vec!["x".into()]);
        s.on_prompt(prompt(1, "a", PromptKind::Password));
        assert_eq!(open_id(&s), Some(1), "ssh's question comes first");
        assert_eq!(s.take_delete(), None);
        s.cancel();
        s.open_delete(host("b"), folder, vec!["x".into()]);
        s.forget_host(&host("b"));
        assert!(!s.is_open());
    }

    #[test]
    fn host_key_and_confirm_sheets_have_no_field() {
        let mut s = Sheets::default();
        s.on_prompt(prompt(1, "a", PromptKind::HostKey { key_type: None, fingerprint: None }));
        assert!(s.edit().is_none());
        assert_eq!(s.edit_mut().text, "");
    }
}
