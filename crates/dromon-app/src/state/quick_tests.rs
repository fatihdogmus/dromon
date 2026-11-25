use dromon_core::HostSource;
use dromon_ssh::quick_host;

use super::*;

fn text(q: &QuickConnect, f: QuickField) -> &str {
    &q.edit(f).text
}

fn focused(q: &QuickConnect) -> Vec<QuickField> {
    FIELDS.into_iter().filter(|f| q.edit(*f).focused).collect()
}

#[test]
fn opening_starts_empty_with_host_focused_and_keeps_what_is_typed() {
    let mut q = QuickConnect::default();
    assert!(!q.focused());
    q.open_empty();
    assert!(q.open && !q.more && q.editing.is_none());
    assert_eq!(focused(&q), [QuickField::Host]);
    q.set_text(QuickField::Host, "prod.example.com");
    q.open_empty();
    assert_eq!(text(&q, QuickField::Host), "prod.example.com", "open again: nothing is lost");
    q.close();
    assert!(!q.open);
    q.open_empty();
    assert_eq!(text(&q, QuickField::Host), "", "closed: what was typed goes");
}

#[test]
fn an_address_pasted_into_host_splits_across_three_fields() {
    let mut q = QuickConnect::default();
    q.open_empty();
    q.set_text(QuickField::User, "old");
    assert!(q.paste_host("deploy@10.0.0.4:2222"));
    assert_eq!((text(&q, QuickField::Host), text(&q, QuickField::User), text(&q, QuickField::Port)), ("10.0.0.4", "deploy", "2222"));
    assert!(q.paste_host("[fe80::1]:22"));
    assert_eq!((text(&q, QuickField::Host), text(&q, QuickField::User), text(&q, QuickField::Port)), ("fe80::1", "deploy", "22"));
    assert!(!q.paste_host("just-a-host"), "a plain host is the field's own paste");
    assert_eq!(text(&q, QuickField::Host), "fe80::1");
    assert_eq!(focused(&q), [QuickField::Host], "the keyboard stays in Host");
}

#[test]
fn problems_show_under_their_fields_and_the_first_takes_the_keyboard() {
    let mut q = QuickConnect::default();
    q.open_empty();
    q.set_text(QuickField::Host, "h");
    q.set_text(QuickField::Port, "0");
    q.set_text(QuickField::ProxyJump, "-J x");
    let errors = quick_host(&q.input(), HostSource::Recent).unwrap_err();
    q.fail(errors);
    assert_eq!(q.errors.keys().copied().collect::<Vec<_>>(), [QuickField::Port, QuickField::ProxyJump]);
    assert!(q.more, "a problem under More opens it");
    assert_eq!(focused(&q), [QuickField::Port]);
    assert_eq!(q.shakes, 1);
    q.edited(QuickField::Port);
    assert!(!q.errors.contains_key(&QuickField::Port), "typing clears the field's error");
    // Closing More with the keyboard in it hands the keyboard back to Host.
    q.focus(QuickField::ProxyJump);
    q.toggle_more();
    assert!(!q.more);
    assert_eq!(focused(&q), [QuickField::Host]);
}

#[test]
fn editing_a_saved_host_fills_the_fields_and_shows_more_when_needed() {
    let mut q = QuickConnect::default();
    let input = QuickInput { host: "10.0.0.4".into(), user: "fatih".into(), port: "2222".into(), alias: "box".into(), ..QuickInput::default() };
    let spec = quick_host(&input, HostSource::Saved).unwrap();
    q.open_with(&spec, Some(spec.id.clone()));
    assert_eq!(q.input(), input);
    assert!(q.more, "its name is under More");
    assert_eq!(q.editing, Some(spec.id.clone()));
    let plain = quick_host(&QuickInput { host: "10.0.0.9".into(), ..QuickInput::default() }, HostSource::Saved).unwrap();
    q.open_with(&plain, None);
    assert!(!q.more && q.editing.is_none());
}

#[test]
fn a_picked_identity_file_fills_its_field() {
    let mut q = QuickConnect::default();
    q.open_empty();
    q.fail(vec![(QuickField::IdentityFile, "x".into())]);
    q.picked_identity("/home/me/.ssh/id_lab");
    assert!(q.more);
    assert_eq!(text(&q, QuickField::IdentityFile), "/home/me/.ssh/id_lab");
    assert!(q.errors.is_empty());
}
