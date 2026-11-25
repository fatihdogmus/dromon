use secrecy::ExposeSecret;

use super::*;

fn host() -> HostId {
    HostId("prod".into())
}

/// The target's password prompt, as OpenSSH words it, and the identity it names.
const PW: &str = "deploy@10.0.0.4's password: ";
const ME: &str = "deploy@10.0.0.4";
/// A ProxyJump jump host's prompt, arriving with the same token.
const JUMP_PW: &str = "jump@bastion's password: ";
const JUMP: &str = "jump@bastion";

const PASSWORD: PromptKind = PromptKind::Password;

fn key(path: &str) -> PromptKind {
    PromptKind::Passphrase { key_path: Some(path.into()) }
}

fn key_prompt(path: &str) -> String {
    format!("Enter passphrase for key '{path}': ")
}

fn text(s: &str) -> PromptAnswer {
    PromptAnswer::Text(s.into())
}

fn host_key() -> PromptKind {
    PromptKind::HostKey { key_type: Some("ED25519".into()), fingerprint: Some("SHA256:x".into()) }
}

const HOST_KEY_PROMPT: &str = "The authenticity of host 'prod (10.0.0.4)' can't be established.\n\
                               ED25519 key fingerprint is: SHA256:x\n\
                               Are you sure you want to continue connecting (yes/no/[fingerprint])? ";

#[track_caller]
fn assert_memory(d: AuthDecision, expected: &str) {
    match d {
        AuthDecision::AnswerFromMemory(s) => assert_eq!(s.expose_secret(), expected),
        other => panic!("expected an answer from memory, got {other:?}"),
    }
}

#[track_caller]
fn assert_ask(d: AuthDecision, expected_error: Option<&str>) {
    match d {
        AuthDecision::AskUser { error } => assert_eq!(error.as_deref(), expected_error),
        other => panic!("expected to ask, got {other:?}"),
    }
}

#[test]
fn first_password_without_memory_asks() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), None);
}

#[test]
fn typed_password_is_remembered_and_used_by_the_next_attempt() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), None);
    a.on_user_answer(&PASSWORD, PW, &text("hunter2"), &mut store);
    assert_eq!(store.get(&host(), ME).unwrap().expose_secret(), "hunter2");

    // Reconnect: a new attempt answers silently.
    let mut b = AuthAttempt::new(host());
    assert_memory(b.on_prompt(&PASSWORD, PW, &mut store), "hunter2");
    assert_eq!(b.host(), &host());
}

#[test]
fn second_password_prompt_after_memory_clears_and_asks_with_error() {
    let mut store = PasswordStore::new();
    store.set(host(), ME.into(), "stale".into());
    let mut a = AuthAttempt::new(host());
    assert_memory(a.on_prompt(&PASSWORD, PW, &mut store), "stale");
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), Some(WRONG_PASSWORD));
    assert!(store.get(&host(), ME).is_none());

    a.on_user_answer(&PASSWORD, PW, &text("fresh"), &mut store);
    assert_eq!(store.get(&host(), ME).unwrap().expose_secret(), "fresh");
}

#[test]
fn password_repeated_after_a_code_blames_neither_answer() {
    const KBD_PW: &str = "(deploy@10.0.0.4) Password: ";
    const CODE: &str = "(deploy@10.0.0.4) Verification code: ";
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    assert_ask(a.on_prompt(&PASSWORD, KBD_PW, &mut store), None);
    a.on_user_answer(&PASSWORD, KBD_PW, &text("right"), &mut store);
    assert_ask(a.on_prompt(&PromptKind::Other, CODE, &mut store), None);
    a.on_user_answer(&PromptKind::Other, CODE, &text("000000"), &mut store);
    assert_ask(a.on_prompt(&PASSWORD, KBD_PW, &mut store), Some(WRONG_SIGN_IN));
    // Without a code in between, a repeat blames the password again.
    a.on_user_answer(&PASSWORD, KBD_PW, &text("typo"), &mut store);
    assert_ask(a.on_prompt(&PASSWORD, KBD_PW, &mut store), Some(WRONG_PASSWORD));
}

#[test]
fn second_password_prompt_after_typing_clears_and_asks_with_error() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), None);
    a.on_user_answer(&PASSWORD, PW, &text("typo"), &mut store);
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), Some(WRONG_PASSWORD));
    assert!(store.get(&host(), ME).is_none());
    a.on_user_answer(&PASSWORD, PW, &text("typo2"), &mut store);
    // Third prompt: still an error, never memory, even though something was stored.
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), Some(WRONG_PASSWORD));
    assert!(store.get(&host(), ME).is_none());
}

#[test]
fn password_memory_is_per_host() {
    let mut store = PasswordStore::new();
    store.set(HostId("other".into()), ME.into(), "theirs".into());
    let mut a = AuthAttempt::new(host());
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), None);
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), Some(WRONG_PASSWORD));
    assert!(store.get(&HostId("other".into()), ME).is_some());
}

#[test]
fn proxy_jump_prompts_are_counted_and_remembered_per_identity() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    // The jump host's ssh asks first, then the target's: neither is a retry.
    assert_ask(a.on_prompt(&PASSWORD, JUMP_PW, &mut store), None);
    a.on_user_answer(&PASSWORD, JUMP_PW, &text("jump-pw"), &mut store);
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), None);
    a.on_user_answer(&PASSWORD, PW, &text("target-pw"), &mut store);
    assert_eq!(store.get(&host(), JUMP).unwrap().expose_secret(), "jump-pw");
    assert_eq!(store.get(&host(), ME).unwrap().expose_secret(), "target-pw");

    // Reconnect: both from memory, each its own.
    let mut b = AuthAttempt::new(host());
    assert_memory(b.on_prompt(&PASSWORD, JUMP_PW, &mut store), "jump-pw");
    assert_memory(b.on_prompt(&PASSWORD, PW, &mut store), "target-pw");
    // The target rejects its password: only the target's is forgotten.
    assert_ask(b.on_prompt(&PASSWORD, PW, &mut store), Some(WRONG_PASSWORD));
    assert!(store.get(&host(), ME).is_none());
    assert_eq!(store.get(&host(), JUMP).unwrap().expose_secret(), "jump-pw");
}

#[test]
fn keyboard_interactive_and_password_method_share_an_identity() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    let kbd = "(deploy@10.0.0.4) Password: ";
    assert_ask(a.on_prompt(&PASSWORD, kbd, &mut store), None);
    a.on_user_answer(&PASSWORD, kbd, &text("pw"), &mut store);
    // The same account asked again through the password method is a retry.
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), Some(WRONG_PASSWORD));
    assert!(store.get(&host(), ME).is_none());
}

#[test]
fn unnamed_password_prompts_use_their_text() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    assert_ask(a.on_prompt(&PASSWORD, "Password: ", &mut store), None);
    a.on_user_answer(&PASSWORD, "Password: ", &text("pw"), &mut store);
    assert_eq!(store.get(&host(), "Password:").unwrap().expose_secret(), "pw");
    assert_ask(a.on_prompt(&PASSWORD, "Password:", &mut store), Some(WRONG_PASSWORD));
}

#[test]
fn cancel_and_empty_answers_are_not_remembered() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    a.on_user_answer(&PASSWORD, PW, &PromptAnswer::Cancel, &mut store);
    a.on_user_answer(&PASSWORD, PW, &text(""), &mut store);
    a.on_user_answer(&key("/k"), &key_prompt("/k"), &text(""), &mut store);
    a.on_user_answer(&key("/k"), &key_prompt("/k"), &PromptAnswer::Cancel, &mut store);
    assert!(store.is_empty());
}

#[test]
fn passphrase_is_remembered_like_a_password() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    let p = key_prompt("/k");
    assert_ask(a.on_prompt(&key("/k"), &p, &mut store), None);
    a.on_user_answer(&key("/k"), &p, &text("pp"), &mut store);
    assert_eq!(store.passphrase(&host(), Some("/k")).unwrap().expose_secret(), "pp");
    assert!(store.get(&host(), &password_identity(&p)).is_none(), "a passphrase must not become a password");

    let mut b = AuthAttempt::new(host());
    assert_memory(b.on_prompt(&key("/k"), &p, &mut store), "pp");
}

#[test]
fn repeated_passphrase_for_the_same_key_clears_and_asks_with_error() {
    let mut store = PasswordStore::new();
    store.set_passphrase(host(), Some("/k".into()), "old".into());
    let mut a = AuthAttempt::new(host());
    let p = key_prompt("/k");
    assert_memory(a.on_prompt(&key("/k"), &p, &mut store), "old");
    assert_ask(a.on_prompt(&key("/k"), &p, &mut store), Some(WRONG_PASSPHRASE));
    assert!(store.passphrase(&host(), Some("/k")).is_none());
}

#[test]
fn passphrases_for_different_keys_are_independent() {
    let mut store = PasswordStore::new();
    store.set_passphrase(host(), Some("/k1".into()), "one".into());
    store.set_passphrase(host(), Some("/k2".into()), "two".into());
    let mut a = AuthAttempt::new(host());
    assert_memory(a.on_prompt(&key("/k1"), &key_prompt("/k1"), &mut store), "one");
    // A different key's first prompt is not a rejection of /k1.
    assert_memory(a.on_prompt(&key("/k2"), &key_prompt("/k2"), &mut store), "two");
    assert!(store.passphrase(&host(), Some("/k1")).is_some());
}

#[test]
fn unnamed_key_passphrase() {
    let mut store = PasswordStore::new();
    let unnamed = PromptKind::Passphrase { key_path: None };
    let p = "Enter passphrase: ";
    let mut a = AuthAttempt::new(host());
    assert_ask(a.on_prompt(&unnamed, p, &mut store), None);
    a.on_user_answer(&unnamed, p, &text("pp"), &mut store);
    assert_eq!(store.passphrase(&host(), None).unwrap().expose_secret(), "pp");
    assert_ask(a.on_prompt(&unnamed, p, &mut store), Some(WRONG_PASSPHRASE));
    assert!(store.passphrase(&host(), None).is_none());
}

#[test]
fn password_and_passphrase_counts_are_separate() {
    let mut store = PasswordStore::new();
    store.set(host(), ME.into(), "pw".into());
    store.set_passphrase(host(), Some("/k".into()), "pp".into());
    let mut a = AuthAttempt::new(host());
    assert_memory(a.on_prompt(&key("/k"), &key_prompt("/k"), &mut store), "pp");
    assert_memory(a.on_prompt(&PASSWORD, PW, &mut store), "pw");
    assert!(store.passphrase(&host(), Some("/k")).is_some());
}

#[test]
fn host_key_confirm_notice_and_other_always_ask_and_are_never_remembered() {
    let mut store = PasswordStore::new();
    store.set(host(), ME.into(), "pw".into());
    let mut a = AuthAttempt::new(host());
    let cases = [
        (host_key(), HOST_KEY_PROMPT),
        (PromptKind::Other, "(deploy@10.0.0.4) Verification code: "),
        (PromptKind::Confirm, "Add key /k (c) to agent?"),
        (PromptKind::Notice, "Confirm user presence for key ED25519-SK SHA256:x"),
        // The same texts again: still asked, never an error.
        (host_key(), HOST_KEY_PROMPT),
        (PromptKind::Other, "(deploy@10.0.0.4) Verification code: "),
        (PromptKind::Confirm, "Add key /k (c) to agent?"),
    ];
    for (kind, prompt) in cases {
        assert_ask(a.on_prompt(&kind, prompt, &mut store), None);
        a.on_user_answer(&kind, prompt, &text("yes"), &mut store);
    }
    // None touched the password or created passphrases or other passwords.
    assert_eq!(store.get(&host(), ME).unwrap().expose_secret(), "pw");
    assert!(store.passphrase(&host(), None).is_none());
    assert_eq!(format!("{store:?}"), "PasswordStore { passwords: 1, passphrases: 0 }");
    // And they don't count as password prompts.
    assert_memory(a.on_prompt(&PASSWORD, PW, &mut store), "pw");
}

#[test]
fn full_flow_host_key_then_password_then_2fa() {
    let mut store = PasswordStore::new();
    let mut a = AuthAttempt::new(host());
    let otp = "(deploy@10.0.0.4) Verification code: ";
    assert_ask(a.on_prompt(&host_key(), HOST_KEY_PROMPT, &mut store), None);
    a.on_user_answer(&host_key(), HOST_KEY_PROMPT, &text("yes"), &mut store);
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), None);
    a.on_user_answer(&PASSWORD, PW, &text("pw"), &mut store);
    assert_ask(a.on_prompt(&PromptKind::Other, otp, &mut store), None);
    a.on_user_answer(&PromptKind::Other, otp, &text("123456"), &mut store);

    // Reconnect: password silent, 2FA asked again.
    let mut b = AuthAttempt::new(host());
    assert_memory(b.on_prompt(&PASSWORD, PW, &mut store), "pw");
    assert_ask(b.on_prompt(&PromptKind::Other, otp, &mut store), None);
}

#[test]
fn auth_failure_forgets_the_last_password_only() {
    let mut store = PasswordStore::new();
    store.set_passphrase(host(), Some("/k".into()), "pp".into());
    store.set(host(), JUMP.into(), "jump-pw".into());
    let mut a = AuthAttempt::new(host());
    assert_memory(a.on_prompt(&PASSWORD, JUMP_PW, &mut store), "jump-pw");
    assert_ask(a.on_prompt(&PASSWORD, PW, &mut store), None);
    a.on_user_answer(&PASSWORD, PW, &text("wrong"), &mut store);
    a.on_auth_failed(&mut store);
    assert!(store.get(&host(), ME).is_none());
    assert_eq!(store.get(&host(), JUMP).unwrap().expose_secret(), "jump-pw");
    assert!(store.passphrase(&host(), Some("/k")).is_some());
}

#[test]
fn auth_failure_without_a_password_prompt_keeps_the_password() {
    let mut store = PasswordStore::new();
    store.set(host(), ME.into(), "pw".into());
    let mut a = AuthAttempt::new(host());
    assert_ask(a.on_prompt(&PromptKind::Other, "(deploy@10.0.0.4) Verification code: ", &mut store), None);
    a.on_auth_failed(&mut store);
    assert!(store.get(&host(), ME).is_some());
}

#[test]
fn debug_output_has_no_secrets() {
    let mut store = PasswordStore::new();
    store.set(host(), ME.into(), "hunter2".into());
    let mut a = AuthAttempt::new(host());
    let d = a.on_prompt(&PASSWORD, PW, &mut store);
    assert!(!format!("{d:?}").contains("hunter2"));
    assert!(!format!("{a:?}").contains("hunter2"));
}
