use serde::{Deserialize, Serialize};

use crate::HostId;

/// A connection's lifecycle as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    Ready,
    /// Lost and retrying silently; `attempt` counts from 1.
    Reconnecting {
        attempt: u32,
    },
    /// Waiting for the user to answer a prompt.
    NeedsInput,
    Failed(String),
    Disconnected,
}

/// What ssh is asking for, decided by `dromon_ssh::classify_prompt`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptKind {
    /// "Are you sure you want to continue connecting (yes/no/[fingerprint])?"
    HostKey {
        key_type: Option<String>,
        fingerprint: Option<String>,
    },
    Password,
    Passphrase {
        key_path: Option<String>,
    },
    /// A yes/no question (`SSH_ASKPASS_PROMPT=confirm`), e.g. confirming a key use. Not secret.
    Confirm,
    /// A notice with nothing to answer (`SSH_ASKPASS_PROMPT=none`, "touch your security key").
    /// ssh dismisses it itself, so the UI shows it until the request closes.
    Notice,
    /// Keyboard-interactive, 2FA, OTP and anything else: never remembered.
    Other,
}

impl PromptKind {
    /// Whether an answer may be kept in memory for silent reconnects.
    pub fn rememberable(&self) -> bool {
        matches!(self, PromptKind::Password | PromptKind::Passphrase { .. })
    }

    /// Whether the answer should be masked while typed.
    pub fn secret(&self) -> bool {
        matches!(self, PromptKind::Password | PromptKind::Passphrase { .. } | PromptKind::Other)
    }
}

/// Identifies one outstanding prompt, so a late answer to an old prompt can be ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PromptId(pub u64);

/// A question from ssh that the UI must answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub id: PromptId,
    pub host: HostId,
    pub kind: PromptKind,
    /// The server's own text, shown in the sheet.
    pub text: String,
    /// Set when this is a retry after a rejected answer ("Wrong password, try again").
    pub error: Option<String>,
}

/// The UI's reply. The secret is wrapped by the receiver; this type is only moved, never logged.
#[derive(Clone, PartialEq, Eq)]
pub enum PromptAnswer {
    Text(String),
    Cancel,
}

impl std::fmt::Debug for PromptAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PromptAnswer::Text(_) => f.write_str("PromptAnswer::Text(<redacted>)"),
            PromptAnswer::Cancel => f.write_str("PromptAnswer::Cancel"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_never_print_their_secret() {
        let a = PromptAnswer::Text("hunter2".into());
        assert!(!format!("{a:?}").contains("hunter2"));
    }

    #[test]
    fn only_passwords_and_passphrases_are_remembered() {
        assert!(PromptKind::Password.rememberable());
        assert!(PromptKind::Passphrase { key_path: None }.rememberable());
        assert!(!PromptKind::Other.rememberable());
        assert!(!PromptKind::HostKey { key_type: None, fingerprint: None }.rememberable());
        assert!(!PromptKind::Confirm.rememberable());
    }

    #[test]
    fn only_typed_secrets_are_masked() {
        assert!(PromptKind::Password.secret());
        assert!(PromptKind::Other.secret());
        assert!(!PromptKind::Confirm.secret());
        assert!(!PromptKind::Notice.secret());
        assert!(!PromptKind::HostKey { key_type: None, fingerprint: None }.secret());
    }
}
