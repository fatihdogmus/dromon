//! Classifies what ssh asks its askpass helper, from the prompt text (`argv[1]`) and the
//! `SSH_ASKPASS_PROMPT` hint.
//!
//! The texts come from OpenSSH 8.4–10.x (`sshconnect.c`, `sshconnect2.c`, `readpass.c`):
//!
//! - Host key: `The authenticity of host 'h (ip)' can't be established.\n<TYPE> key fingerprint
//!   is SHA256:….` (10.x: `… fingerprint is: SHA256:…`, no period), optional extra lines, then
//!   `Are you sure you want to continue connecting (yes/no/[fingerprint])? `. A bad answer is
//!   re-asked as `Please type 'yes', 'no' or the fingerprint: `. The IP-mismatch warning asks
//!   `… continue connecting (yes/no)? ` and re-asks `Please type 'yes' or 'no': `. No hint is set.
//! - Password: `user@host's password: `. Keyboard-interactive prompts come from the server
//!   (PAM usually says `Password: `); 8.5+ prefixes them with `(user@host) `.
//! - Passphrase: `Enter passphrase for key '/path': `; ssh-add style `Enter passphrase for /path: `.
//!   ssh cuts the path at 100 bytes (`%.100s`), so `key_path` is only a prefix for longer
//!   paths; it is still the same for every prompt about that key.
//! - Hints (`readpass.c`): `confirm` is set only by `ask_permission()` (`RP_ASK_PERMISSION`),
//!   for yes/no permission questions such as `Add key /path (comment) to agent?`
//!   (`AddKeysToAgent ask`) or ssh-agent's key-use confirmation: [`PromptKind::Confirm`].
//!   `none` is set by `notify_start()` for a notice such as `Confirm user presence for key …`,
//!   whose answer ssh never reads: [`PromptKind::Notice`]. Nothing else sets a hint; in
//!   particular the host-key question goes through `confirm()` → `read_passphrase(msg, RP_ECHO)`
//!   with no hint (checked in OpenSSH 10.2p1 `sshconnect.c`). Should a future ssh send it with
//!   `confirm`, the host-key text still wins, so the fingerprint sheet is shown.
//! - Everything else (`(u@h) Verification code: `, OTP, Duo menus, `Enter PIN for … key …: `,
//!   password-change prompts) is [`PromptKind::Other`], which is never remembered.

use dromon_core::PromptKind;

/// What kind of question `text` is. See the module docs for the texts recognised.
pub fn classify_prompt(text: &str, hint: Option<&str>) -> PromptKind {
    let hint = hint.map(str::trim);
    if hint.is_some_and(|h| h.eq_ignore_ascii_case("none")) {
        return PromptKind::Notice;
    }
    if is_host_key_question(text) {
        let (key_type, fingerprint) = parse_fingerprint(text);
        return PromptKind::HostKey { key_type, fingerprint };
    }
    if hint.is_some_and(|h| h.eq_ignore_ascii_case("confirm")) {
        return PromptKind::Confirm;
    }
    // Password and passphrase prompts are a single line. Never treat a multi-line server message
    // as one, so a remembered password is not sent to an unexpected question.
    let line = text.trim();
    if line.contains('\n') {
        return PromptKind::Other;
    }
    let line = strip_user_host_prefix(line);
    if let Some(kind) = passphrase(line) {
        return kind;
    }
    if is_password(line) {
        return PromptKind::Password;
    }
    PromptKind::Other
}

fn is_host_key_question(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("continue connecting (yes/no")
        || lower.trim_start().starts_with("please type 'yes', 'no' or the fingerprint")
        || lower.trim_start().starts_with("please type 'yes' or 'no'")
}

/// `ED25519 key fingerprint is SHA256:abc.` → (`ED25519`, `SHA256:abc`).
fn parse_fingerprint(text: &str) -> (Option<String>, Option<String>) {
    const MARKER: &str = " key fingerprint is";
    for line in text.lines() {
        let Some(at) = line.find(MARKER) else { continue };
        let key_type = line[..at].split_whitespace().last().map(str::to_owned);
        let rest = line[at + MARKER.len()..].trim_start_matches(':').trim();
        let fingerprint = rest.split_whitespace().next().map(|fp| fp.trim_end_matches('.')).filter(|fp| !fp.is_empty());
        return (key_type, fingerprint.map(str::to_owned));
    }
    (None, None)
}

/// Who a password prompt is for: `deploy@10.0.0.4's password: ` and
/// `(deploy@10.0.0.4) Password: ` both give `deploy@10.0.0.4`; any other text gives itself,
/// trimmed.
///
/// One ssh process can ask for several passwords: with ProxyJump the jump host's ssh inherits
/// our askpass environment, so its prompts arrive with the same token and host. Counting and
/// remembering passwords per identity keeps the jump host's prompt from looking like a retry.
pub fn password_identity(text: &str) -> String {
    let line = text.trim();
    if let Some((who, _)) = split_user_host_prefix(line) {
        return who.to_owned();
    }
    // ASCII lowercasing keeps byte offsets, so slicing the original is safe.
    if let Some(head) = line.to_ascii_lowercase().strip_suffix("'s password:") {
        let who = &line[..head.len()];
        if !who.is_empty() && !who.contains(char::is_whitespace) {
            return who.to_owned();
        }
    }
    line.to_owned()
}

/// Removes the `(user@host) ` that OpenSSH 8.5+ puts before keyboard-interactive prompts.
fn strip_user_host_prefix(line: &str) -> &str {
    split_user_host_prefix(line).map_or(line, |(_, rest)| rest)
}

/// `(user@host) rest` → (`user@host`, `rest`).
fn split_user_host_prefix(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('(')?;
    let end = rest.find(") ")?;
    let inner = &rest[..end];
    (inner.contains('@') && !inner.contains(char::is_whitespace)).then(|| (inner, rest[end + 2..].trim_start()))
}

fn passphrase(line: &str) -> Option<PromptKind> {
    let lower = line.to_ascii_lowercase();
    if lower == "enter passphrase:" {
        return Some(PromptKind::Passphrase { key_path: None });
    }
    let prefixes = ["enter passphrase for key ", "enter passphrase for ", "bad passphrase, try again for "];
    let prefix = prefixes.iter().find(|p| lower.starts_with(*p))?;
    // ASCII lowercasing keeps byte offsets, so slicing the original is safe.
    let rest = line[prefix.len()..].trim_end();
    let rest = rest.strip_suffix(':')?.trim_end();
    let rest = rest.strip_suffix("(will confirm each use)").unwrap_or(rest).trim_end();
    let path = unquote(rest);
    Some(PromptKind::Passphrase { key_path: (!path.is_empty()).then(|| path.to_owned()) })
}

fn unquote(s: &str) -> &str {
    for q in ['\'', '"'] {
        if let Some(inner) = s.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return inner;
        }
    }
    s
}

/// `Password:`, `user@host's password:`, `Password for user@REALM:`, `Enter password:`.
fn is_password(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let Some(head) = lower.strip_suffix(':').map(str::trim_end) else { return false };
    if let Some(who) = head.strip_suffix("password").map(str::trim_end) {
        if matches!(who, "" | "enter" | "enter your") {
            return true;
        }
        // `deploy@10.0.0.4's password:`, but not `Enter u@h's old password:`.
        if let Some(name) = who.strip_suffix("'s") {
            return !name.is_empty() && !name.contains(char::is_whitespace);
        }
        return false;
    }
    // Kerberos/PAM: `Password for user@REALM:`.
    head.strip_prefix("password for ").is_some_and(|who| !who.is_empty() && !who.contains(char::is_whitespace))
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
