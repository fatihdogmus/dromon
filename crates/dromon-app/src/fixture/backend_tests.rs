//! The fixture backend: connecting, each auth step answered right, wrong and cancelled,
//! remembered secrets, listing pages and errors, and the app on top of it.

use std::sync::Arc;

use dromon_core::{ConnStatus, Error, HostId, Prompt, PromptAnswer, PromptId, PromptKind, RemotePath};
use dromon_ssh::{WRONG_PASSPHRASE, WRONG_PASSWORD, WRONG_SIGN_IN};

use super::backend::PAGE;
use super::*;
use crate::state::{Action, Backend, Command, Listing, ServiceEvent};

const FP: &str = "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU";

fn prod() -> HostId {
    HostId("prod".into())
}

/// One host, prod, whose server asks `auth` (a JSON list) and then `fail`s or opens at
/// /home/deploy.
fn fixture(auth: &str, fail: Option<&str>) -> Arc<Fixture> {
    let fail = fail.map_or(String::new(), |f| format!(r#", "fail": "{f}""#));
    let src = format!(
        r#"{{"version": 1, "now": "2026-11-25T14:00:00Z",
            "hosts": [{{"alias": "prod", "source": "ssh-config", "user": "deploy", "hostname": "prod.example.com"}},
                      {{"alias": "lost", "source": "saved"}}],
            "servers": {{"prod": {{"home": "/home/deploy", "auth": {auth}{fail}, "tree": {{
                "/home/deploy": [{{"name": "big", "kind": "dir"}}, {{"name": "empty", "kind": "dir"}}, {{"name": "x", "kind": "file"}}],
                "/home/deploy/big": [{{"generate": {{"pattern": "f{{04}}", "count": 1200}}}}],
                "/home/deploy/locked": {{"error": "permission-denied"}},
                "/home/deploy/flaky": {{"error": "connection-lost"}}}}}}}}}}"#
    );
    Arc::new(parse(&src).unwrap_or_else(|e| panic!("{e}\n{src}")))
}

fn backend(auth: &str) -> FixtureBackend {
    FixtureBackend::new(fixture(auth, None))
}

fn connect(b: &mut FixtureBackend) -> Vec<ServiceEvent> {
    b.send(Command::Connect { host: prod() })
}

fn reply(b: &mut FixtureBackend, id: PromptId, text: &str) -> Vec<ServiceEvent> {
    b.send(Command::AnswerPrompt { id, answer: PromptAnswer::Text(text.into()) })
}

fn cancel(b: &mut FixtureBackend, id: PromptId) -> Vec<ServiceEvent> {
    b.send(Command::AnswerPrompt { id, answer: PromptAnswer::Cancel })
}

fn prompts(evs: &[ServiceEvent]) -> Vec<Prompt> {
    evs.iter().filter_map(|e| if let ServiceEvent::Prompt(p) = e { Some(p.clone()) } else { None }).collect()
}

/// The one prompt in `evs`.
fn asked(evs: &[ServiceEvent]) -> Prompt {
    let ps = prompts(evs);
    assert_eq!(ps.len(), 1, "{evs:#?}");
    ps[0].clone()
}

fn statuses(evs: &[ServiceEvent]) -> Vec<ConnStatus> {
    evs.iter().filter_map(|e| if let ServiceEvent::Status { status, .. } = e { Some(status.clone()) } else { None }).collect()
}

fn logs(evs: &[ServiceEvent]) -> Vec<String> {
    evs.iter().filter_map(|e| if let ServiceEvent::Log { line, .. } = e { Some(line.clone()) } else { None }).collect()
}

fn opened(evs: &[ServiceEvent]) -> bool {
    evs.iter().any(|e| matches!(e, ServiceEvent::Opened { start, .. } if start.as_str() == "/home/deploy"))
}

fn failed(evs: &[ServiceEvent]) -> Option<String> {
    statuses(evs).into_iter().find_map(|s| if let ConnStatus::Failed(m) = s { Some(m) } else { None })
}

#[test]
fn without_auth_it_connects_at_once_with_the_services_events() {
    let mut b = backend("[]");
    let evs = connect(&mut b);
    let ssh = "Running ssh -T -x -e none -o BatchMode=no -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o ConnectTimeout=15 -o ControlMaster=no -s prod sftp";
    let host = prod();
    let log = |l: &str| ServiceEvent::Log { host: host.clone(), line: l.into() };
    assert_eq!(
        evs,
        [
            ServiceEvent::Status { host: prod(), status: ConnStatus::Connecting },
            log(ssh),
            log("SFTP session started."),
            ServiceEvent::Status { host: prod(), status: ConnStatus::Ready },
            ServiceEvent::Opened { host: prod(), start: RemotePath::new("/home/deploy") },
            log("Connected."),
        ]
    );
    // Connecting while up (another window asking) opens it again for the asker, and nothing more.
    assert_eq!(connect(&mut b), [ServiceEvent::Opened { host: prod(), start: RemotePath::new("/home/deploy") }]);
}

#[test]
fn a_host_key_is_trusted_once_and_remembered() {
    let mut b = backend(r#"[{"host-key": {"key_type": "ED25519", "fingerprint": "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU"}}]"#);
    let evs = connect(&mut b);
    let p = asked(&evs);
    assert_eq!(p.kind, PromptKind::HostKey { key_type: Some("ED25519".into()), fingerprint: Some(FP.into()) });
    assert!(p.text.starts_with("The authenticity of host 'prod.example.com' can't be established.\nED25519 key fingerprint is SHA256:"));
    assert_eq!(statuses(&evs), [ConnStatus::Connecting, ConnStatus::NeedsInput]);
    assert!(logs(&evs).contains(&"ssh asks “The authenticity of host 'prod.example.com' can't be established.”".to_owned()));
    let evs = reply(&mut b, p.id, "yes");
    assert_eq!(statuses(&evs), [ConnStatus::Connecting, ConnStatus::Ready]);
    assert_eq!(logs(&evs)[..2], ["Answered ssh's prompt.", "Warning: Permanently added 'prod.example.com' (ED25519) to the list of known hosts."]);
    assert!(opened(&evs));
    // Known now: the next connection doesn't ask.
    b.send(Command::Disconnect { host: prod() });
    assert!(prompts(&connect(&mut b)).is_empty());
}

#[test]
fn a_host_key_cancelled_or_declined_fails() {
    for answer in [None, Some("no")] {
        let mut b = backend(r#"[{"host-key": {"key_type": "ED25519", "fingerprint": "SHA256:x"}}]"#);
        let p = asked(&connect(&mut b));
        let evs = match answer {
            None => cancel(&mut b, p.id),
            Some(a) => reply(&mut b, p.id, a),
        };
        assert_eq!(failed(&evs).as_deref(), Some("Host key verification failed."), "{answer:?}");
        assert!(logs(&evs).contains(&"Couldn't connect: Host key verification failed.".to_owned()));
        assert!(!opened(&evs));
    }
}

#[test]
fn a_wrong_password_asks_again_with_the_services_error() {
    let mut b = backend(r#"[{"password": {"accept": "hunter2"}}]"#);
    let p = asked(&connect(&mut b));
    assert_eq!((p.kind.clone(), p.text.as_str(), p.error.clone()), (PromptKind::Password, "deploy@prod.example.com's password: ", None));
    let evs = reply(&mut b, p.id, "wrong");
    let retry = asked(&evs);
    assert_ne!(retry.id, p.id);
    assert_eq!(retry.error.as_deref(), Some(WRONG_PASSWORD));
    assert_eq!(statuses(&evs), [ConnStatus::Connecting, ConnStatus::NeedsInput]);
    assert!(logs(&evs).contains(&"Permission denied, please try again.".to_owned()));
    // An answer to the old prompt is ignored.
    assert!(reply(&mut b, p.id, "hunter2").is_empty());
    let evs = reply(&mut b, retry.id, "hunter2");
    assert!(opened(&evs));
    // No answer reaches any event.
    for e in [&evs[..]].concat() {
        assert!(!format!("{e:?}").contains("hunter2"));
    }
}

#[test]
fn three_wrong_passwords_or_a_cancel_fail() {
    let mut b = backend(r#"[{"password": {"prompt": "Password: ", "accept": "right"}}]"#);
    let mut p = asked(&connect(&mut b));
    assert_eq!(p.text, "Password: ");
    for _ in 0..2 {
        p = asked(&reply(&mut b, p.id, "wrong"));
    }
    let evs = reply(&mut b, p.id, "wrong");
    assert_eq!(failed(&evs).as_deref(), Some("Permission denied (publickey,password)."));
    assert!(logs(&evs).contains(&"deploy@prod.example.com: Permission denied (publickey,password).".to_owned()));

    let mut b = backend(r#"[{"password": {"accept": "right"}}]"#);
    let p = asked(&connect(&mut b));
    assert_eq!(failed(&cancel(&mut b, p.id)).as_deref(), Some("Permission denied (publickey,password)."));
    // Trying again asks again, without an error.
    let p = asked(&connect(&mut b));
    assert_eq!(p.error, None);
}

#[test]
fn a_passphrase_right_wrong_and_cancelled() {
    let auth = r#"[{"passphrase": {"key_path": "/home/u/.ssh/id_ed25519", "accept": "open sesame"}}]"#;
    let mut b = backend(auth);
    let p = asked(&connect(&mut b));
    assert_eq!(p.kind, PromptKind::Passphrase { key_path: Some("/home/u/.ssh/id_ed25519".into()) });
    assert_eq!(p.text, "Enter passphrase for key '/home/u/.ssh/id_ed25519': ");
    let retry = asked(&reply(&mut b, p.id, "nope"));
    assert_eq!(retry.error.as_deref(), Some(WRONG_PASSPHRASE));
    assert_eq!(retry.text, "Bad passphrase, try again for /home/u/.ssh/id_ed25519: ");
    assert!(opened(&reply(&mut b, retry.id, "open sesame")));

    let mut b = backend(auth);
    let p = asked(&connect(&mut b));
    assert_eq!(failed(&cancel(&mut b, p.id)).as_deref(), Some("Permission denied (publickey)."));
}

#[test]
fn a_code_after_a_password_starts_the_round_over_when_wrong() {
    let mut b = backend(r#"[{"password": {"accept": "pw"}}, {"code": {"accept": "123456"}}]"#);
    let p = asked(&connect(&mut b));
    let code = asked(&reply(&mut b, p.id, "pw"));
    assert_eq!((code.kind.clone(), code.text.as_str()), (PromptKind::Other, "(deploy@prod.example.com) Verification code: "));
    // A wrong code: the server asks for both again, and we can't tell which was wrong.
    let again = asked(&reply(&mut b, code.id, "000000"));
    assert_eq!((again.kind.clone(), again.error.as_deref()), (PromptKind::Password, Some(WRONG_SIGN_IN)));
    let code = asked(&reply(&mut b, again.id, "pw"));
    assert_eq!(code.error, None);
    assert!(opened(&reply(&mut b, code.id, "123456")));
}

#[test]
fn a_code_alone_asks_again_and_a_cancel_fails() {
    let mut b = backend(r#"[{"code": {"prompt": "Token: ", "accept": "42"}}]"#);
    let p = asked(&connect(&mut b));
    let again = asked(&reply(&mut b, p.id, "41"));
    assert_eq!((again.text.as_str(), again.error.clone()), ("Token: ", None));
    assert_eq!(failed(&cancel(&mut b, again.id)).as_deref(), Some("Permission denied (keyboard-interactive)."));
}

#[test]
fn confirm_yes_no_and_cancel() {
    let auth = r#"[{"confirm": {"text": "Allow use of key /k?"}}]"#;
    let mut b = backend(auth);
    let p = asked(&connect(&mut b));
    assert_eq!((p.kind.clone(), p.text.as_str()), (PromptKind::Confirm, "Allow use of key /k?"));
    assert!(opened(&reply(&mut b, p.id, "yes")));
    for no in [Some("no"), None] {
        let mut b = backend(auth);
        let p = asked(&connect(&mut b));
        let evs = if let Some(t) = no { reply(&mut b, p.id, t) } else { cancel(&mut b, p.id) };
        assert_eq!(failed(&evs).as_deref(), Some("Permission denied (publickey)."));
    }
}

#[test]
fn a_notice_shows_until_the_connection_settles() {
    let mut b = backend(r#"[{"notice": {"text": "Confirm user presence for key ED25519-SK SHA256:abc"}}, {"password": {"accept": "pw"}}]"#);
    let evs = connect(&mut b);
    let ps = prompts(&evs);
    assert_eq!(ps.iter().map(|p| p.kind.clone()).collect::<Vec<_>>(), [PromptKind::Notice, PromptKind::Password]);
    assert_eq!(statuses(&evs), [ConnStatus::Connecting, ConnStatus::NeedsInput], "a notice needs no input");
    assert!(reply(&mut b, ps[0].id, "x").is_empty(), "notices take no answer");
    let evs = reply(&mut b, ps[1].id, "pw");
    assert!(evs.contains(&ServiceEvent::PromptClosed(ps[0].id)));
    assert!(opened(&evs));
}

#[test]
fn a_failing_server_keeps_the_password_for_the_next_try() {
    let mut b = FixtureBackend::new(fixture(r#"[{"password": {"accept": "pw"}}]"#, Some("Connection closed by 203.0.113.10 port 22")));
    let p = asked(&connect(&mut b));
    let evs = reply(&mut b, p.id, "pw");
    assert_eq!(failed(&evs).as_deref(), Some("Connection closed by 203.0.113.10 port 22"));
    assert!(logs(&evs).ends_with(&[
        "Connection closed by 203.0.113.10 port 22".to_owned(),
        "ssh exited (exit status: 255).".to_owned(),
        "Couldn't connect: Connection closed by 203.0.113.10 port 22".to_owned()
    ]));
    // Not an auth failure, so the password stays in memory and answers the next attempt.
    let evs = connect(&mut b);
    assert!(prompts(&evs).is_empty());
    assert!(logs(&evs).contains(&"ssh asked “deploy@prod.example.com's password:”: answered from memory.".to_owned()));
}

#[test]
fn disconnect_closes_prompts_and_forgets_secrets() {
    let mut b = backend(r#"[{"password": {"accept": "pw"}}]"#);
    let p = asked(&connect(&mut b));
    let evs = b.send(Command::Disconnect { host: prod() });
    assert!(evs.contains(&ServiceEvent::PromptClosed(p.id)));
    assert_eq!(statuses(&evs), [ConnStatus::Disconnected]);
    assert!(logs(&evs).contains(&"Disconnected.".to_owned()));
    let p = asked(&connect(&mut b));
    assert!(opened(&reply(&mut b, p.id, "pw")));
    b.send(Command::Disconnect { host: prod() });
    assert_eq!(asked(&connect(&mut b)).error, None, "asked again: the password was forgotten");
    // A host that never connected just reports it.
    assert_eq!(statuses(&b.send(Command::Disconnect { host: HostId("lost".into()) })), [ConnStatus::Disconnected]);
}

#[test]
fn hosts_without_a_server_or_unknown_fail() {
    let mut b = backend("[]");
    let evs = b.send(Command::Connect { host: HostId("lost".into()) });
    assert_eq!(failed(&evs).as_deref(), Some("Could not resolve hostname lost: Name or service not known"));
    let evs = b.send(Command::Connect { host: HostId("ghost".into()) });
    assert_eq!(failed(&evs).as_deref(), Some("Dromon doesn't know this host any more."));
}

fn list(b: &mut FixtureBackend, path: &str) -> Vec<ServiceEvent> {
    b.send(Command::List { host: prod(), path: RemotePath::new(path) })
}

#[test]
fn listings_stream_in_pages_of_512() {
    let mut b = backend("[]");
    assert!(list(&mut b, "/home/deploy").is_empty(), "nothing to list with before the session is up");
    connect(&mut b);
    let evs = list(&mut b, "/home/deploy/big");
    let shape: Vec<(usize, bool, bool)> = evs
        .iter()
        .map(|e| match e {
            ServiceEvent::ListingPage { entries, first, done, path, .. } if path.as_str() == "/home/deploy/big" => (entries.len(), *first, *done),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(shape, [(PAGE, true, false), (PAGE, false, false), (1200 - 2 * PAGE, false, false), (0, false, true)]);
    // A small folder is one page, then the last; an empty one is just the last.
    assert_eq!(list(&mut b, "/home/deploy").len(), 2);
    assert!(matches!(&list(&mut b, "/home/deploy/empty")[..], [ServiceEvent::ListingPage { first: true, done: true, entries, .. }] if entries.is_empty()));
    let evs = list(&mut b, "/home/deploy/locked");
    assert!(matches!(&evs[..], [ServiceEvent::ListFailed { error: Error::PermissionDenied(_), .. }]));
    let evs = list(&mut b, "/home/deploy/x");
    assert!(matches!(&evs[..], [ServiceEvent::ListFailed { error: Error::NotFound(_), .. }]));
}

#[test]
fn a_lost_connection_stays_reconnecting() {
    let mut b = backend("[]");
    connect(&mut b);
    let evs = list(&mut b, "/home/deploy/flaky");
    assert_eq!(statuses(&evs), [ConnStatus::Reconnecting { attempt: 1 }]);
    assert_eq!(logs(&evs), ["Connection lost: the SFTP channel stopped answering.", "Reconnecting in 1 s."]);
    assert!(list(&mut b, "/home/deploy").is_empty(), "no session while reconnecting");
}

/// The app on the backend: a wrong then right password through the sheet, then the listing.
#[test]
fn the_app_signs_in_through_its_sheet() {
    let mut s = super::app(fixture(r#"[{"password": {"accept": "hunter2"}}]"#, None));
    assert_eq!(s.now, DEMO_NOW);
    s.act(Action::SelectHost(prod()));
    assert_eq!(s.conn().unwrap().status, ConnStatus::NeedsInput);
    assert!(s.ui.sheets.is_open());
    s.ui.sheets.edit_mut().replace_text("wrong");
    s.act(Action::SubmitPrompt);
    let error = s.ui.sheets.open().and_then(|sh| sh.prompt()).and_then(|p| p.error.clone());
    assert_eq!(error.as_deref(), Some(WRONG_PASSWORD));
    s.ui.sheets.edit_mut().replace_text("hunter2");
    s.act(Action::SubmitPrompt);
    assert!(!s.ui.sheets.is_open());
    let c = s.conn().unwrap();
    assert_eq!((c.status.clone(), c.cwd.as_str()), (ConnStatus::Ready, "/home/deploy"));
    assert!(matches!(&c.listing, Listing::Ready(e) if e.len() == 3));
    assert!(c.log.to_text().contains("Answered ssh's prompt."));
    assert!(!c.log.to_text().contains("hunter2"));
    // Log stamps are on the fixture's clock.
    assert!(c.log.iter().all(|l| l.at_ms == DEMO_NOW * 1000));
    s.act(Action::Open("big".into()));
    assert_eq!(s.conn().unwrap().row_count(), 1200);
}

/// docs/fixtures.md's complete example parses and does what the doc says.
#[test]
fn the_docs_example() {
    let doc = include_str!("../../../../docs/fixtures.md");
    let start = doc.find("## A complete example").and_then(|at| doc[at..].find("```json\n").map(|i| at + i + 8)).unwrap();
    let end = start + doc[start..].find("\n```").unwrap();
    let fx = Arc::new(parse(&doc[start..end]).unwrap_or_else(|e| panic!("docs/fixtures.md: {e}")));
    let mut b = FixtureBackend::new(fx.clone());
    let lab = HostId("lab".into());
    // lab: a notice, then passphrase, confirm, password and code; then the server hangs up.
    let evs = b.send(Command::Connect { host: lab.clone() });
    let kinds: Vec<PromptKind> = prompts(&evs).into_iter().map(|p| p.kind).collect();
    assert_eq!(kinds, [PromptKind::Notice, PromptKind::Passphrase { key_path: Some("/home/me/.ssh/id_ed25519_sk".into()) }]);
    let mut p = prompts(&evs)[1].clone();
    for answer in ["open sesame", "yes", "pw"] {
        p = asked(&reply(&mut b, p.id, answer));
    }
    assert_eq!(p.text, "(kbd@10.0.0.7) Verification code: ");
    let evs = reply(&mut b, p.id, "123456");
    assert_eq!(failed(&evs).as_deref(), Some("Connection closed by 10.0.0.7 port 2222"));
    let old = b.send(Command::Connect { host: HostId("old-box".into()) });
    assert_eq!(failed(&old).as_deref(), Some("Could not resolve hostname 192.0.2.1: Name or service not known"));
    // prod's folders.
    let prod = fx.server("prod").unwrap();
    assert_eq!(prod.list(&RemotePath::new("/var/www/site/assets")).unwrap().len(), 600);
    assert!(matches!(prod.list(&RemotePath::new("/var/log")), Err(Error::PermissionDenied(_))));
    assert_eq!(prod.list(&RemotePath::new("/var")).unwrap().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["log", "www"]);
}

/// The backend never asks the store for anything but what `AuthAttempt` does: a backend with
/// no auth steps sends no prompt to any of the demo's hosts.
#[test]
fn the_demo_connects_everywhere_without_a_prompt() {
    let fx = demo();
    let mut b = FixtureBackend::new(fx.clone());
    for h in &fx.hosts {
        let evs = b.send(Command::Connect { host: h.id.clone() });
        assert!(prompts(&evs).is_empty() && statuses(&evs).contains(&ConnStatus::Ready), "{}", h.alias);
    }
    let _ = b.send(Command::AnswerPrompt { id: PromptId(99), answer: PromptAnswer::Text("stale".into()) });
}

/// Verbose logging as the service does it: a live connection comes back at once with `-v`,
/// signing in from memory and listing the folder on screen again; off, back without it.
#[test]
fn verbose_logging_reconnects_with_dash_v_and_without() {
    let mut b = backend(r#"[{"password": {"accept": "hunter2"}}]"#);
    let p = asked(&connect(&mut b));
    assert!(opened(&reply(&mut b, p.id, "hunter2")));
    b.send(Command::List { host: prod(), path: RemotePath::new("/home/deploy") });

    let evs = b.send(Command::Verbose { host: prod(), on: true });
    assert_eq!(evs[0], ServiceEvent::Verbose { host: prod(), on: true });
    assert_eq!(statuses(&evs), [ConnStatus::Reconnecting { attempt: 1 }, ConnStatus::Ready]);
    assert!(prompts(&evs).is_empty() && !opened(&evs), "silent, the window keeps its folder");
    let lines = logs(&evs);
    let run = lines.iter().find(|l| l.starts_with("Running ssh")).unwrap();
    assert!(run.ends_with(" -v -s prod sftp"), "{run}");
    assert!(lines.contains(&"debug1: Connecting to prod.example.com port 22.".to_owned()), "{lines:#?}");
    assert!(lines.contains(&"ssh asked “deploy@prod.example.com's password:”: answered from memory.".to_owned()));
    assert!(evs.iter().any(|e| matches!(e, ServiceEvent::ListingPage { path, done: true, .. } if path.as_str() == "/home/deploy")));
    // Connecting again from another window says it's on.
    assert!(connect(&mut b).contains(&ServiceEvent::Verbose { host: prod(), on: true }));

    let evs = b.send(Command::Verbose { host: prod(), on: false });
    let lines = logs(&evs);
    assert!(lines.iter().any(|l| l.starts_with("Running ssh") && l.ends_with(" -s prod sftp") && !l.contains(" -v ")), "{lines:#?}");
    assert!(!lines.iter().any(|l| l.starts_with("debug1:")));
    assert_eq!(statuses(&evs), [ConnStatus::Reconnecting { attempt: 1 }, ConnStatus::Ready]);
    // The same again: said, nothing else.
    assert_eq!(b.send(Command::Verbose { host: prod(), on: false }), [ServiceEvent::Verbose { host: prod(), on: false }]);
    // A host that isn't connected takes it at its next connect.
    let mut b = backend("[]");
    let evs = b.send(Command::Verbose { host: prod(), on: true });
    assert_eq!(evs[0], ServiceEvent::Verbose { host: prod(), on: true });
    assert!(statuses(&evs).is_empty());
    assert!(logs(&connect(&mut b)).iter().any(|l| l.ends_with(" -v -s prod sftp")));
}
