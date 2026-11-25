//! How to answer each prompt in one connection attempt (one ssh process), per plan §4.3:
//!
//! | Prompt | Decision |
//! |---|---|
//! | Password, first for this identity in this attempt | Remembered password if any, else ask |
//! | Password, again for the same identity | The last answer was rejected: forget it, ask with an error |
//! | Passphrase, first for this key in this attempt | Remembered passphrase if any, else ask |
//! | Passphrase, again for the same key | Rejected: forget it, ask with an error |
//! | Host key, confirm, notice, anything else | Always ask, never remembered |
//!
//! A password prompt's identity is the `user@host` it names ([`password_identity`]). One ssh
//! process can ask for several: with ProxyJump the jump host's ssh inherits our askpass
//! environment, so its `jump@bastion's password: ` arrives with the same token as the target's
//! `deploy@10.0.0.4's password: `, and must not count as a retry of it.
//!
//! The state machine clears rejected secrets itself through the `&mut PasswordStore` it is given,
//! so the caller only forwards decisions and answers. Create a fresh [`AuthAttempt`] for every
//! ssh process, reconnects included.

use std::collections::HashMap;

use dromon_core::{HostId, PromptAnswer, PromptKind};
use secrecy::SecretString;

use crate::passwords::PasswordStore;
use crate::prompt::password_identity;

/// The inline error when a password is asked again.
pub const WRONG_PASSWORD: &str = "That password didn't work. Try again.";
/// The inline error when a key passphrase is asked again.
pub const WRONG_PASSPHRASE: &str = "That passphrase didn't work. Try again.";

/// What to do with one prompt.
#[derive(Debug)]
pub enum AuthDecision {
    /// Reply with this remembered secret without showing anything.
    AnswerFromMemory(SecretString),
    /// Show a sheet, with `error` under the field when this is a retry.
    AskUser { error: Option<String> },
}

/// One connection attempt's view of the prompts so far.
#[derive(Debug)]
pub struct AuthAttempt {
    host: HostId,
    /// Per password identity (`user@host`, see [`password_identity`]).
    password_prompts: HashMap<String, u32>,
    /// The identity of the latest password prompt: the one being tried when ssh gives up.
    last_password: Option<String>,
    /// Per key path (`None` when ssh didn't name the key).
    passphrase_prompts: HashMap<Option<String>, u32>,
}

impl AuthAttempt {
    pub fn new(host: HostId) -> Self {
        AuthAttempt { host, password_prompts: HashMap::new(), last_password: None, passphrase_prompts: HashMap::new() }
    }

    pub fn host(&self) -> &HostId {
        &self.host
    }

    /// Decides how to answer the next prompt, of kind `kind` (from
    /// [`classify_prompt`](crate::classify_prompt)) with ssh's text `prompt`. A repeated
    /// password (same identity) or passphrase (same key) prompt clears the rejected secret from
    /// `store` before asking.
    pub fn on_prompt(&mut self, kind: &PromptKind, prompt: &str, store: &mut PasswordStore) -> AuthDecision {
        match kind {
            PromptKind::Password => {
                let identity = password_identity(prompt);
                let count = self.password_prompts.entry(identity.clone()).or_insert(0);
                *count += 1;
                let repeated = *count > 1;
                self.last_password = Some(identity.clone());
                if repeated {
                    store.clear_password(&self.host, &identity);
                    return AuthDecision::AskUser { error: Some(WRONG_PASSWORD.to_owned()) };
                }
                match store.get(&self.host, &identity) {
                    Some(secret) => AuthDecision::AnswerFromMemory(secret.clone()),
                    None => AuthDecision::AskUser { error: None },
                }
            }
            PromptKind::Passphrase { key_path } => {
                let count = self.passphrase_prompts.entry(key_path.clone()).or_insert(0);
                *count += 1;
                if *count > 1 {
                    store.clear_passphrase(&self.host, key_path.as_deref());
                    return AuthDecision::AskUser { error: Some(WRONG_PASSPHRASE.to_owned()) };
                }
                match store.passphrase(&self.host, key_path.as_deref()) {
                    Some(secret) => AuthDecision::AnswerFromMemory(secret.clone()),
                    None => AuthDecision::AskUser { error: None },
                }
            }
            PromptKind::HostKey { .. } | PromptKind::Confirm | PromptKind::Notice | PromptKind::Other => {
                AuthDecision::AskUser { error: None }
            }
        }
    }

    /// Records what the user typed for the prompt `prompt` of kind `kind`. Passwords (by
    /// identity) and passphrases (by key) are remembered right away (a rejection shows up as a
    /// repeated prompt and clears them); host-key and other answers, cancels and empty answers
    /// are not. ssh treats an empty passphrase as "skip this key", so remembering it would skip
    /// the key on every reconnect.
    pub fn on_user_answer(&mut self, kind: &PromptKind, prompt: &str, answer: &PromptAnswer, store: &mut PasswordStore) {
        let PromptAnswer::Text(text) = answer else { return };
        if text.is_empty() {
            return;
        }
        match kind {
            PromptKind::Password => {
                store.set(self.host.clone(), password_identity(prompt), SecretString::from(text.as_str()));
            }
            PromptKind::Passphrase { key_path } => {
                store.set_passphrase(self.host.clone(), key_path.clone(), SecretString::from(text.as_str()));
            }
            PromptKind::HostKey { .. } | PromptKind::Confirm | PromptKind::Notice | PromptKind::Other => {}
        }
    }

    /// Call when ssh gave up on authentication ("Permission denied") after this attempt saw a
    /// password prompt: the last password tried (the latest prompt's identity; a jump host
    /// finishes before its target starts) was rejected without a further prompt, so it is
    /// forgotten. Other identities' passwords are kept, and so are passphrases: ssh checks those
    /// locally and re-asks when they are wrong, so a failure later on doesn't mean the passphrase
    /// was.
    pub fn on_auth_failed(&mut self, store: &mut PasswordStore) {
        if let Some(identity) = &self.last_password {
            store.clear_password(&self.host, identity);
        }
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
