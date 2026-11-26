//! The fixture backend serving two main windows as the service would.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use dromon_core::{ConnStatus, HostId, PromptAnswer, RemotePath};

use super::*;
use crate::fixture::{Fixture, parse};
use crate::state::{Backend, Command, ServiceEvent, WindowId};

const B: WindowId = WindowId(1);

fn prod() -> HostId {
    HostId("prod".into())
}

/// prod asks for a password ("pw") and opens at /home/deploy, with two folders.
fn fixture() -> Arc<Fixture> {
    let src = r#"{"version": 1, "now": "2026-11-25T14:00:00Z",
        "hosts": [{"alias": "prod", "source": "ssh-config", "user": "deploy", "hostname": "prod.example.com"}],
        "servers": {"prod": {"home": "/home/deploy", "auth": [{"password": {"accept": "pw"}}], "tree": {
            "/home/deploy": [{"name": "www", "kind": "dir"}, {"name": "x", "kind": "file"}],
            "/home/deploy/www": [{"name": "index.html", "kind": "file"}]}}}}"#;
    Arc::new(parse(src).unwrap_or_else(|e| panic!("{e}")))
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

fn prompt_of(evs: &[ServiceEvent]) -> Option<dromon_core::Prompt> {
    evs.iter().find_map(|e| if let ServiceEvent::Prompt(p) = e { Some(p.clone()) } else { None })
}

/// Window A signs in to prod; B is open beside it.
fn signed_in() -> (FixtureBackend, Box<dyn Backend>) {
    let mut a = FixtureBackend::new(fixture());
    let mut b = a.another(B).expect("a second window's end");
    let asked = prompt_of(&a.send(Command::Connect { host: prod() })).expect("a password prompt, in A");
    let evs = a.send(Command::AnswerPrompt { id: asked.id, answer: PromptAnswer::Text("pw".into()) });
    assert!(evs.iter().any(|e| matches!(e, ServiceEvent::Opened { .. })));
    // B heard the connection come up too.
    let heard = b.pending();
    assert!(heard.iter().any(|e| matches!(e, ServiceEvent::Status { status: ConnStatus::Ready, .. })), "{heard:?}");
    assert!(prompt_of(&heard).is_none(), "the prompt was A's alone");
    (a, b)
}

#[test]
fn a_host_up_for_one_window_opens_in_another_alone_and_listings_reach_both() {
    let (mut a, mut b) = signed_in();
    // B asks for prod: up already, it opens in B at once, A hears nothing of it.
    let evs = b.send(Command::Connect { host: prod() });
    assert_eq!(evs, [ServiceEvent::Opened { host: prod(), start: p("/home/deploy") }]);
    assert!(a.pending().is_empty());
    // B lists www; A (on /home/deploy) hears the pages too and ignores them.
    let evs = b.send(Command::List { host: prod(), path: p("/home/deploy/www") });
    assert!(evs.iter().any(|e| matches!(e, ServiceEvent::ListingPage { done: true, .. })));
    assert_eq!(a.pending(), evs);
}

#[test]
fn each_window_polls_its_own_folder() {
    let (mut a, mut b) = signed_in();
    a.send(Command::List { host: prod(), path: p("/home/deploy") });
    b.send(Command::Connect { host: prod() });
    b.send(Command::List { host: prod(), path: p("/home/deploy/www") });
    a.send(Command::Window { visible: true, focused: true });
    b.send(Command::Window { visible: false, focused: false });
    // Nothing changed elsewhere: a poll from either says nothing; B's hidden window's folder
    // isn't polled at all.
    assert!(a.send(Command::Poll { host: prod() }).is_empty());
    assert!(b.send(Command::Poll { host: prod() }).is_empty());
    // A closes; B's folder is still watched (a ⌘R in B is a refresh of it).
    a.send(Command::WindowClosed);
    let evs = b.send(Command::List { host: prod(), path: p("/home/deploy/www") });
    assert!(evs.iter().any(|e| matches!(e, ServiceEvent::ListingPage { path, .. } if *path == p("/home/deploy/www"))));
}

#[test]
fn prompts_go_to_the_focused_window_and_move_when_it_closes() {
    let mut a = FixtureBackend::new(fixture());
    let mut b = a.another(B).expect("a second window's end");
    let woken = Rc::new(RefCell::new(Vec::new()));
    let log = woken.clone();
    a.set_wake(Box::new(move |w| log.borrow_mut().push(w)));
    b.send(Command::Window { visible: true, focused: true });
    // A connects while B has the focus: the sheet is B's.
    let evs = a.send(Command::Connect { host: prod() });
    assert!(prompt_of(&evs).is_none());
    let asked = prompt_of(&b.pending()).expect("in B");
    assert!(woken.borrow().contains(&B), "B was woken for it");
    // B closes before answering: the prompt moves to A.
    b.send(Command::WindowClosed);
    let moved = prompt_of(&a.pending()).expect("moved to A");
    assert_eq!(moved.id, asked.id);
    let evs = a.send(Command::AnswerPrompt { id: asked.id, answer: PromptAnswer::Text("pw".into()) });
    assert!(evs.iter().any(|e| matches!(e, ServiceEvent::Opened { .. })));
}

#[test]
fn show_prompts_sends_the_open_prompt_to_the_asking_window_alone() {
    let mut a = FixtureBackend::new(fixture());
    let mut b = a.another(B).expect("a second window's end");
    a.send(Command::Window { visible: true, focused: true });
    // A connects while focused: the sheet is A's.
    let asked = prompt_of(&a.send(Command::Connect { host: prod() })).expect("in A");
    assert!(prompt_of(&b.pending()).is_none());
    // B asks to sign in: the same prompt comes to B, A hears nothing.
    let evs = b.send(Command::ShowPrompts { host: prod() });
    assert_eq!(prompt_of(&evs).map(|p| p.id), Some(asked.id));
    assert!(a.pending().is_empty());
    // Answering in B closes it in A too (B's own sheet waits for ssh).
    let evs = b.send(Command::AnswerPrompt { id: asked.id, answer: PromptAnswer::Text("wrong".into()) });
    assert!(a.pending().contains(&ServiceEvent::PromptClosed(asked.id)));
    // The host's next prompt (the wrong password asks again) goes to B, though A is focused.
    let again = prompt_of(&evs).expect("asked again, in B");
    assert_ne!(again.id, asked.id);
    assert!(prompt_of(&a.pending()).is_none());
    // A host with no open prompt: nothing.
    let evs = b.send(Command::AnswerPrompt { id: again.id, answer: PromptAnswer::Text("pw".into()) });
    assert!(evs.iter().any(|e| matches!(e, ServiceEvent::Opened { .. })));
    assert!(prompt_of(&b.send(Command::ShowPrompts { host: prod() })).is_none());
}
