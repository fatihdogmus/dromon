//! Properties of prompt classification, on any text and hint ssh or a server could send. Neither
//! `classify_prompt` nor `password_identity` panics. A hint counts trimmed and in any case;
//! `none` is always a notice. A question of several lines is never taken for a password or a
//! passphrase, unless it's the host-key question. A passphrase prompt names its key, and a
//! password prompt its `user@host`; any other text is its own identity, trimmed.

use dromon_core::PromptKind;
use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use super::*;

/// Pieces of what ssh and servers ask, and what's awkward around them.
const PIECES: &[&str] = &[
    "password",
    "Password",
    "PASSWORD",
    ":",
    ": ",
    "'s",
    "(",
    ")",
    ") ",
    "@",
    "deploy@10.0.0.4",
    "Enter ",
    "enter passphrase for key ",
    "Enter passphrase for ",
    "Bad passphrase, try again for ",
    "Enter passphrase:",
    "'",
    "\"",
    " (will confirm each use)",
    "continue connecting (yes/no",
    "Please type 'yes' or 'no'",
    " key fingerprint is",
    ":",
    "SHA256:abc",
    ".",
    "\n",
    "\r\n",
    " ",
    "\t",
    "\u{a0}",
    "\u{3000}",
    "\0",
    "é",
    "İ",
    "ß",
    "ﬀ",
    "K",
    "漢",
    "🚀",
];

/// Text built from [`PIECES`] and random bits, or any text.
#[hegel::composite]
fn texts(tc: &TestCase) -> String {
    let piece = hegel::one_of!(gs::sampled_from(PIECES).map(|p: &str| p.to_owned()), gs::text().max_size(4));
    tc.draw(hegel::one_of!(gs::vecs(piece).max_size(10).map(|p: Vec<String>| p.concat()), gs::text()))
}

/// A hint: none, one ssh sets, either with odd case and spaces, or any text.
#[hegel::composite]
fn hints(tc: &TestCase) -> Option<String> {
    let word = gs::sampled_from(vec!["none", "confirm", "NONE", "Confirm", " none ", "\tconfirm\n", ""]);
    tc.draw(gs::optional(hegel::one_of!(word.map(|w: &str| w.to_owned()), texts())))
}

/// `text` with one letter of it in the other ASCII case.
fn flip_case(text: &str, at: usize) -> String {
    let flip = |c: char| if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() };
    text.chars().enumerate().map(|(i, c)| if i == at { flip(c) } else { c }).collect()
}

#[hegel::test]
fn classifying_any_prompt_never_panics_and_a_hint_counts_trimmed_in_any_case(tc: TestCase) {
    let text = tc.draw(texts());
    let hint = tc.draw(hints());
    let at = tc.draw(gs::integers::<usize>().max_value(8));
    let kind = classify_prompt(&text, hint.as_deref());
    let _ = password_identity(&text);
    if let Some(hint) = &hint {
        let padded = format!(" \t{}\n", flip_case(hint, at));
        assert_eq!(classify_prompt(&text, Some(&padded)), kind, "hint {hint:?} as {padded:?}");
        if hint.trim().eq_ignore_ascii_case("none") {
            assert_eq!(kind, PromptKind::Notice);
        }
    }
}

/// A remembered password or passphrase is only sent to a one-line question.
#[hegel::test]
fn a_question_of_several_lines_is_never_a_password_or_passphrase(tc: TestCase) {
    let first = tc.draw(texts());
    let second = tc.draw(texts());
    let hint = tc.draw(hints());
    let text = format!("{}\n{}", first.trim(), second.trim());
    tc.assume(!first.trim().is_empty() && !second.trim().is_empty());
    assert!(!classify_prompt(&text, hint.as_deref()).rememberable(), "{text:?}");
}

/// `Enter passphrase for key '<path>': ` (and the other forms ssh writes) names the key.
#[hegel::test]
fn a_passphrase_prompt_names_its_key(tc: TestCase) {
    let path = tc.draw(texts());
    let form = tc.draw(gs::sampled_from(vec!["Enter passphrase for key '{}': ", "Enter passphrase for {}: "]));
    let prefix = tc.draw(gs::sampled_from(vec!["", "(u@h) ", "  "]));
    let text = format!("{prefix}{}", form.replace("{}", &path));
    tc.assume(!path.contains(['\n', '\'', '"']) && path.trim() == path && !path.is_empty());
    tc.assume(!path.ends_with("(will confirm each use)") && !path.to_ascii_lowercase().contains("continue connecting"));
    tc.assume(!path.to_ascii_lowercase().starts_with("key "));
    assert_eq!(classify_prompt(&text, None), PromptKind::Passphrase { key_path: Some(path) }, "{text:?}");
}

/// `<who>'s password: ` and `(<who>) Password: ` name `who`, a `user@host`.
#[hegel::test]
fn a_password_prompt_names_its_user_at_host(tc: TestCase) {
    let user = tc.draw(texts());
    let host = tc.draw(texts());
    let form = tc.draw(gs::sampled_from(vec!["{}'s password: ", "({}) Password: ", "  ({}) Verification code: "]));
    let who = format!("{user}@{host}");
    tc.assume(!who.contains(char::is_whitespace) && !who.starts_with('('));
    assert_eq!(password_identity(&form.replace("{}", &who)), who);
}

/// What `password_identity` gives is the text trimmed, or the `user@host` it names in one of the
/// two forms ssh writes.
#[hegel::test]
fn any_other_text_is_its_own_identity_trimmed(tc: TestCase) {
    let text = tc.draw(texts());
    let t = text.trim();
    let who = password_identity(&text);
    if who == t {
        return;
    }
    assert!(!who.is_empty() && !who.contains(char::is_whitespace), "{who:?}");
    let named = t.strip_prefix('(').and_then(|r| r.strip_prefix(who.as_str())).is_some_and(|r| r.starts_with(") "));
    let says = t.to_ascii_lowercase() == format!("{}'s password:", who.to_ascii_lowercase());
    assert!((named && who.contains('@')) || says, "{text:?} gave {who:?}");
}
