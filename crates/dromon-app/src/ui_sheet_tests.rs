//! Sheets driven headless: prompts from the service in, keys and clicks, commands out. The app
//! runs its real view and handlers on a [`Recorder`] backend, so every command it sends can be
//! read back.

use dromon_core::{ConnStatus, HostId, Prompt, PromptAnswer, PromptId, PromptKind};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::state::{Action, App, Command, Recorder, ServiceEvent};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);
const PW: &str = "deploy@prod.example.com's password: ";

fn prod() -> HostId {
    HostId("prod".into())
}

/// The app connecting to prod, with nothing answering but the test.
fn runtime() -> (Runtime<App>, Recorder) {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    let rec = Recorder::default();
    let backend = rec.clone();
    rt.update(move |s, _| {
        *s = scenes::base();
        s.backend = Box::new(backend);
        s.act(Action::SelectHost(prod()));
    });
    rt.frame(0.0);
    (rt, rec)
}

fn prompt(id: u64, host: &str, kind: PromptKind, text: &str, error: Option<&str>) -> Prompt {
    Prompt { id: PromptId(id), host: HostId(host.into()), kind, text: text.into(), error: error.map(str::to_owned) }
}

fn apply(rt: &mut Runtime<App>, ev: ServiceEvent) {
    rt.update(move |s, _| s.apply(ev));
    settle(rt);
}

/// Lets springs and exits finish.
fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn key(rt: &mut Runtime<App>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    settle(rt);
}

fn click(rt: &mut Runtime<App>, k: &str) {
    let (x, y) = rt.find(k).unwrap_or_else(|| panic!("no element {k:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// Whether `k` is on screen (not on its way out).
fn shown(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

fn text_of(rt: &Runtime<App>, k: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).and_then(|e| e.text).unwrap_or_default()
}

/// The answers the app sent, in order.
fn answers(rec: &Recorder) -> Vec<(u64, PromptAnswer)> {
    rec.sent()
        .into_iter()
        .filter_map(|c| match c {
            Command::AnswerPrompt { id, answer } => Some((id.0, answer)),
            _ => None,
        })
        .collect()
}

fn text(s: &str) -> PromptAnswer {
    PromptAnswer::Text(s.into())
}

#[test]
fn a_password_prompt_shows_a_focused_sheet_and_return_sends_what_was_typed() {
    let (mut rt, rec) = runtime();
    assert!(!shown(&rt, "sheet"));
    apply(&mut rt, ServiceEvent::Status { host: prod(), status: ConnStatus::NeedsInput });
    apply(&mut rt, ServiceEvent::Prompt(prompt(1, "prod", PromptKind::Password, PW, None)));
    assert!(shown(&rt, "sheet"));
    assert_eq!(text_of(&rt, "sheet-title"), "Sign in to prod");
    assert!(shown(&rt, "sheet-note"), "passwords say they're kept in memory only");
    assert_eq!(rt.presented_focus_key(), Some("sheet-input"), "the field has the keyboard");

    key(&mut rt, Key::Text("hunter2".into()));
    key(&mut rt, Key::Enter);
    assert_eq!(answers(&rec), [(1, text("hunter2"))]);
    // Answered: the sheet waits for ssh, its field emptied.
    assert!(shown(&rt, "sheet-busy"));
    assert_eq!(rt.state.ui.sheets.edit_mut().text, "");
    // It connects: the sheet goes.
    apply(&mut rt, ServiceEvent::Status { host: prod(), status: ConnStatus::Ready });
    assert!(!shown(&rt, "sheet"));
}

#[test]
fn escape_cancels_the_prompt() {
    let (mut rt, rec) = runtime();
    apply(&mut rt, ServiceEvent::Prompt(prompt(1, "prod", PromptKind::Password, PW, None)));
    key(&mut rt, Key::Text("half".into()));
    key(&mut rt, Key::Escape);
    assert_eq!(answers(&rec), [(1, PromptAnswer::Cancel)]);
    assert!(!shown(&rt, "sheet"));
}

#[test]
fn a_wrong_password_asks_again_in_place_with_an_error() {
    let (mut rt, rec) = runtime();
    apply(&mut rt, ServiceEvent::Prompt(prompt(1, "prod", PromptKind::Password, PW, None)));
    key(&mut rt, Key::Text("wrong".into()));
    key(&mut rt, Key::Enter);
    assert!(!shown(&rt, "sheet-error"));
    apply(&mut rt, ServiceEvent::Prompt(prompt(2, "prod", PromptKind::Password, PW, Some(dromon_ssh::WRONG_PASSWORD))));
    assert!(shown(&rt, "sheet"));
    assert_eq!(text_of(&rt, "sheet-error"), dromon_ssh::WRONG_PASSWORD);
    assert!(shown(&rt, "sheet-field-2"), "the field is the retry's, so it shakes once");
    assert_eq!(rt.presented_focus_key(), Some("sheet-input"));
    key(&mut rt, Key::Text("right".into()));
    click(&mut rt, "sheet-submit");
    assert_eq!(answers(&rec), [(1, text("wrong")), (2, text("right"))]);
}

#[test]
fn the_host_key_sheet_trusts_with_yes() {
    let (mut rt, rec) = runtime();
    let fp = "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU";
    let kind = PromptKind::HostKey { key_type: Some("ED25519".into()), fingerprint: Some(fp.into()) };
    apply(&mut rt, ServiceEvent::Prompt(prompt(7, "prod", kind, "The authenticity of host…", None)));
    assert_eq!(text_of(&rt, "sheet-title"), "First connection to prod");
    assert_eq!(text_of(&rt, "sheet-fingerprint"), fp);
    assert!(!shown(&rt, "sheet-input"));
    // Return presses the default button.
    key(&mut rt, Key::Enter);
    assert_eq!(answers(&rec), [(7, text("yes"))]);
}

#[test]
fn confirm_answers_no() {
    let (mut rt, rec) = runtime();
    apply(&mut rt, ServiceEvent::Prompt(prompt(3, "prod", PromptKind::Confirm, "Allow use of key /k?", None)));
    click(&mut rt, "sheet-no");
    assert_eq!(answers(&rec), [(3, text("no"))]);
    assert!(!shown(&rt, "sheet"));
}

#[test]
fn a_closed_prompt_takes_its_sheet_away() {
    let (mut rt, rec) = runtime();
    apply(&mut rt, ServiceEvent::Prompt(prompt(1, "prod", PromptKind::Other, "Verification code: ", None)));
    assert!(shown(&rt, "sheet"));
    assert!(!shown(&rt, "sheet-note"), "2FA codes are never remembered");
    apply(&mut rt, ServiceEvent::PromptClosed(PromptId(1)));
    assert!(!shown(&rt, "sheet"));
    assert!(answers(&rec).is_empty());
}

#[test]
fn prompts_queue_one_sheet_at_a_time() {
    let (mut rt, rec) = runtime();
    apply(&mut rt, ServiceEvent::Prompt(prompt(1, "prod", PromptKind::Password, PW, None)));
    apply(&mut rt, ServiceEvent::Prompt(prompt(2, "nas", PromptKind::Password, "admin@nas.local's password: ", None)));
    assert_eq!(text_of(&rt, "sheet-title"), "Sign in to prod");
    key(&mut rt, Key::Escape);
    assert_eq!(text_of(&rt, "sheet-title"), "Sign in to nas");
    key(&mut rt, Key::Text("pw".into()));
    key(&mut rt, Key::Enter);
    assert_eq!(answers(&rec), [(1, PromptAnswer::Cancel), (2, text("pw"))]);
}

#[test]
fn notices_are_toasts_until_closed() {
    let (mut rt, _rec) = runtime();
    let text = "Confirm user presence for key ED25519-SK SHA256:abc";
    apply(&mut rt, ServiceEvent::Prompt(prompt(4, "prod", PromptKind::Notice, text, None)));
    assert!(shown(&rt, "notice-4"));
    assert!(!shown(&rt, "sheet"), "a notice blocks nothing");
    apply(&mut rt, ServiceEvent::PromptClosed(PromptId(4)));
    assert!(!shown(&rt, "notice-4"));
}

#[test]
fn the_status_opens_the_connection_log() {
    let (mut rt, rec) = runtime();
    apply(&mut rt, ServiceEvent::Log { host: prod(), line: "debug1: Connecting to prod".into() });
    click(&mut rt, "status-conn");
    assert!(shown(&rt, "sheet"));
    assert!(text_of(&rt, "log-lines").ends_with("debug1: Connecting to prod"));
    click(&mut rt, "log-disconnect");
    assert!(rec.sent().contains(&Command::Disconnect { host: prod() }));
    key(&mut rt, Key::Escape);
    assert!(!shown(&rt, "sheet"));
}

#[test]
fn the_logs_verbose_switch_asks_for_ssh_with_dash_v_and_says_it_reconnects() {
    let (mut rt, rec) = runtime();
    apply(&mut rt, ServiceEvent::Status { host: prod(), status: ConnStatus::Ready });
    click(&mut rt, "status-conn");
    let switch = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("log-verbose")).expect("the switch");
    assert_eq!((switch.role, switch.label.as_deref()), ("switch", Some("Verbose logging (reconnects)")));
    assert_eq!(text_of(&rt, "log-verbose-note"), "reconnects");
    let track = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("log-verbose-track")).unwrap().fill;
    let off = track(&rt);

    click(&mut rt, "log-verbose");
    assert_eq!(rec.sent().last(), Some(&Command::Verbose { host: prod(), on: true }));
    assert!(rt.state.conn().unwrap().log.verbose);
    assert_ne!(track(&rt), off, "on: the accent track");
    assert!(shown(&rt, "sheet"), "the log stays open to watch the reconnect");
    // ssh's debug lines arrive in the log like any other.
    apply(&mut rt, ServiceEvent::Log { host: prod(), line: "debug1: Connecting to prod port 22.".into() });
    assert!(text_of(&rt, "log-lines").ends_with("debug1: Connecting to prod port 22."));

    // Another window turned it off: this one's switch follows.
    apply(&mut rt, ServiceEvent::Verbose { host: prod(), on: false });
    assert!(!rt.state.conn().unwrap().log.verbose);
    assert_eq!(track(&rt), off);
    click(&mut rt, "log-verbose");
    assert_eq!(rec.sent().last(), Some(&Command::Verbose { host: prod(), on: true }));
}
