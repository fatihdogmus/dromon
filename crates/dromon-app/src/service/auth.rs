//! Prompt routing for one connection attempt (one ssh process, plan §4.3 "Askpass").
//!
//! Every askpass request for the attempt goes through [`AttemptPrompts::on_request`]: it is
//! classified, and `AuthAttempt` decides. A remembered secret is answered at once without any
//! UI; anything else becomes a [`Prompt`] with a fresh [`PromptId`] and waits for the user's
//! `AnswerPrompt` with that id. Answers for ids that are gone are ignored and wiped. A notice is
//! shown and never answered; when ssh's helper hangs up, [`AttemptPrompts::hung_up`] reports it
//! so the UI can drop the sheet or toast.
//!
//! The [`PasswordStore`] is the service's alone (behind a mutex shared by the connection
//! tasks, locked only briefly and never across an await). Answers are never logged.

use std::collections::HashMap;
use std::future::poll_fn;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use dromon_core::{HostId, Prompt, PromptAnswer, PromptId, PromptKind};
use dromon_ssh::{AskpassRequest, AuthAttempt, AuthDecision, PasswordStore, classify_prompt};
use secrecy::ExposeSecret;
use tokio::sync::oneshot;
use zeroize::Zeroize;

/// Issues prompt ids for the whole service and remembers which host each open one belongs
/// to, so an `AnswerPrompt` reaches the right connection.
#[derive(Clone, Default)]
pub struct PromptIds {
    next: Arc<AtomicU64>,
    owners: Arc<Mutex<HashMap<PromptId, HostId>>>,
}

impl PromptIds {
    fn issue(&self, host: &HostId) -> PromptId {
        let id = PromptId(self.next.fetch_add(1, Ordering::Relaxed) + 1);
        self.lock().insert(id, host.clone());
        id
    }

    fn release(&self, id: PromptId) {
        self.lock().remove(&id);
    }

    /// The host of the open prompt `id`.
    pub fn owner(&self, id: PromptId) -> Option<HostId> {
        self.lock().get(&id).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PromptId, HostId>> {
        self.owners.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Wipes an answer that won't be delivered.
pub fn wipe(answer: PromptAnswer) {
    if let PromptAnswer::Text(mut text) = answer {
        text.zeroize();
    }
}

/// Locks the store, even if a panicking task poisoned it.
pub fn lock(store: &Mutex<PasswordStore>) -> std::sync::MutexGuard<'_, PasswordStore> {
    store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One prompt waiting for the user.
struct Open {
    id: PromptId,
    kind: PromptKind,
    text: String,
    reply: oneshot::Sender<PromptAnswer>,
}

/// What became of one askpass request.
#[derive(Debug, PartialEq, Eq)]
pub enum Routed {
    /// Answered from memory; the UI sees nothing.
    FromMemory,
    /// Show this; `needs_user` unless it's a notice (which waits for nothing).
    Asked { prompt: Prompt, needs_user: bool },
    /// The helper was already gone.
    Dropped,
}

/// The prompts of one connection attempt, with its `AuthAttempt`.
pub struct AttemptPrompts {
    host: HostId,
    attempt: AuthAttempt,
    open: Vec<Open>,
    ids: PromptIds,
}

impl AttemptPrompts {
    pub fn new(host: HostId, ids: PromptIds) -> Self {
        AttemptPrompts { attempt: AuthAttempt::new(host.clone()), host, open: Vec::new(), ids }
    }

    /// Decides one askpass request (see the module docs).
    pub fn on_request(&mut self, req: AskpassRequest, store: &Mutex<PasswordStore>) -> Routed {
        if req.reply.is_closed() {
            return Routed::Dropped;
        }
        let kind = classify_prompt(&req.prompt, req.hint.as_deref());
        let decision = self.attempt.on_prompt(&kind, &req.prompt, &mut lock(store));
        match decision {
            AuthDecision::AnswerFromMemory(secret) => {
                if let Err(answer) = req.reply.send(PromptAnswer::Text(secret.expose_secret().to_owned())) {
                    wipe(answer);
                }
                Routed::FromMemory
            }
            AuthDecision::AskUser { error } => {
                let id = self.ids.issue(&self.host);
                let needs_user = kind != PromptKind::Notice;
                let prompt =
                    Prompt { id, host: self.host.clone(), kind: kind.clone(), text: req.prompt.clone(), error };
                self.open.push(Open { id, kind, text: req.prompt, reply: req.reply });
                Routed::Asked { prompt, needs_user }
            }
        }
    }

    /// The user answered `id`. Returns whether it was one of ours waiting for the user (a
    /// notice, or an id that's gone, is ignored and its answer wiped).
    pub fn on_answer(&mut self, id: PromptId, answer: PromptAnswer, store: &Mutex<PasswordStore>) -> bool {
        let Some(at) = self.open.iter().position(|p| p.id == id && p.kind != PromptKind::Notice) else {
            wipe(answer);
            return false;
        };
        let open = self.open.remove(at);
        self.ids.release(id);
        self.attempt.on_user_answer(&open.kind, &open.text, &answer, &mut lock(store));
        if let Err(answer) = open.reply.send(answer) {
            wipe(answer);
        }
        true
    }

    /// Resolves with the id of the first open prompt whose helper hung up (ssh exited, or a
    /// notice ended); never while none is open. Follow with [`AttemptPrompts::remove`].
    pub async fn hung_up(&mut self) -> PromptId {
        poll_fn(|cx| {
            for p in &mut self.open {
                if p.reply.poll_closed(cx).is_ready() {
                    return Poll::Ready(p.id);
                }
            }
            Poll::Pending
        })
        .await
    }

    /// Forgets the open prompt `id`; returns whether it was waiting for the user.
    pub fn remove(&mut self, id: PromptId) -> Option<bool> {
        let at = self.open.iter().position(|p| p.id == id)?;
        let open = self.open.remove(at);
        self.ids.release(id);
        Some(open.kind != PromptKind::Notice)
    }

    /// Closes every open prompt (their helpers get a cancel) and returns their ids.
    pub fn close_all(&mut self) -> Vec<PromptId> {
        let ids: Vec<PromptId> = self.open.drain(..).map(|p| p.id).collect();
        for id in &ids {
            self.ids.release(*id);
        }
        ids
    }

    /// ssh gave up on authentication: forget the password it was last given.
    pub fn auth_failed(&mut self, store: &Mutex<PasswordStore>) {
        self.attempt.on_auth_failed(&mut lock(store));
    }
}

impl Drop for AttemptPrompts {
    fn drop(&mut self) {
        self.close_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Mutex<PasswordStore> {
        Mutex::new(PasswordStore::new())
    }

    fn request(prompt: &str, hint: Option<&str>) -> (AskpassRequest, oneshot::Receiver<PromptAnswer>) {
        let (reply, rx) = oneshot::channel();
        (
            AskpassRequest { host: HostId("prod".into()), prompt: prompt.into(), hint: hint.map(str::to_owned), reply },
            rx,
        )
    }

    fn asked(r: Routed) -> Prompt {
        match r {
            Routed::Asked { prompt, .. } => prompt,
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    const PW: &str = "deploy@prod's password: ";

    #[test]
    fn a_password_is_asked_then_remembered_for_the_next_attempt() {
        let (ids, store) = (PromptIds::default(), store());
        let mut a = AttemptPrompts::new(HostId("prod".into()), ids.clone());
        let (req, mut rx) = request(PW, None);
        let p = asked(a.on_request(req, &store));
        assert_eq!((p.kind.clone(), p.error.clone()), (PromptKind::Password, None));
        assert_eq!(ids.owner(p.id), Some(HostId("prod".into())));
        assert!(!a.on_answer(PromptId(999), PromptAnswer::Text("stale".into()), &store), "stale ids are ignored");
        assert!(a.on_answer(p.id, PromptAnswer::Text("hunter2".into()), &store));
        assert_eq!(rx.try_recv().unwrap(), PromptAnswer::Text("hunter2".into()));
        assert_eq!(ids.owner(p.id), None);

        // A reconnect's fresh attempt answers from memory without the UI.
        let mut b = AttemptPrompts::new(HostId("prod".into()), ids.clone());
        let (req, mut rx) = request(PW, None);
        assert_eq!(b.on_request(req, &store), Routed::FromMemory);
        assert_eq!(rx.try_recv().unwrap(), PromptAnswer::Text("hunter2".into()));
        // Asked again in the same attempt: it was wrong; ask with the error.
        let (req, _rx) = request(PW, None);
        let p = asked(b.on_request(req, &store));
        assert_eq!(p.error.as_deref(), Some(dromon_ssh::WRONG_PASSWORD));
    }

    #[test]
    fn notices_are_shown_but_never_answered() {
        let (ids, store) = (PromptIds::default(), store());
        let mut a = AttemptPrompts::new(HostId("prod".into()), ids);
        let (req, rx) = request("Confirm user presence for key ED25519-SK SHA256:abc", Some("none"));
        let Routed::Asked { prompt, needs_user } = a.on_request(req, &store) else { panic!() };
        assert!(!needs_user);
        assert!(!a.on_answer(prompt.id, PromptAnswer::Text("x".into()), &store));
        drop(rx);
        assert_eq!(a.remove(prompt.id), Some(false));
        assert_eq!(a.remove(prompt.id), None);
    }

    #[tokio::test]
    async fn a_helper_hanging_up_is_reported() {
        let (ids, store) = (PromptIds::default(), store());
        let mut a = AttemptPrompts::new(HostId("prod".into()), ids);
        let (req, _keep) = request("Verification code: ", None);
        let first = asked(a.on_request(req, &store));
        let (req, rx) = request(PW, None);
        let second = asked(a.on_request(req, &store));
        drop(rx);
        assert_eq!(a.hung_up().await, second.id);
        assert_eq!(a.remove(second.id), Some(true));
        assert_eq!(a.close_all(), [first.id]);
    }

    #[test]
    fn a_request_whose_helper_is_gone_is_dropped() {
        let (ids, store) = (PromptIds::default(), store());
        let mut a = AttemptPrompts::new(HostId("prod".into()), ids);
        let (req, rx) = request(PW, None);
        drop(rx);
        assert_eq!(a.on_request(req, &store), Routed::Dropped);
    }
}
