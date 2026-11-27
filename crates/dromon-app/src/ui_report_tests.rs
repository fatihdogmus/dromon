//! The bug report sheet driven headless: an error's Report opens it with that error, its keys
//! and Tab order, what a screen reader gets, Copy report, Open GitHub, Escape.

use dromon_core::{ConnStatus, HostId};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::state::update::{Method, Offer, Package, UpdateAction, UpdateEvent, Version};
use crate::state::{Action, App, ServiceEvent, Silent};
use crate::ui_a11y_tests::{scene, tree};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);
const SHIFT: Mods = Mods { shift: true, cmd: false, alt: false, ctrl: false, logo: false };

fn runtime(start: fn(&mut App)) -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(move |s, _| start(s));
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn key(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

fn click(rt: &mut Runtime<App>, k: &str) {
    let (x, y) = rt.find(k).unwrap_or_else(|| panic!("no element {k:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn shown(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

fn text_of(rt: &Runtime<App>, k: &str) -> String {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(k) && !e.exiting)
        .and_then(|e| e.text)
        .unwrap_or_default()
}

fn focused_key(rt: &Runtime<App>) -> String {
    rt.elements().into_iter().find(|e| e.focused && !e.exiting).and_then(|e| e.key).unwrap_or_default()
}

/// The key of the element whose key starts with `prefix` and ends with `suffix`.
fn key_like(rt: &Runtime<App>, prefix: &str, suffix: &str) -> String {
    rt.elements()
        .into_iter()
        .filter_map(|e| e.key)
        .find(|k| k.starts_with(prefix) && k.ends_with(suffix))
        .unwrap_or_else(|| panic!("no {prefix}…{suffix}"))
}

#[test]
fn a_failed_deletes_report_opens_the_sheet_with_its_error() {
    let mut rt = runtime(scenes::delete_failed);
    assert!(!shown(&rt, "report"));
    let report = key_like(&rt, "toast-op-", "-report");
    click(&mut rt, &report);
    assert!(shown(&rt, "report"));
    assert_eq!(text_of(&rt, "sheet-title"), "Report a Bug");
    assert!(text_of(&rt, "report-explain").contains("new issue on GitHub"));
    let text = text_of(&rt, "report-text");
    assert_eq!(text, rt.state.report.draft.as_ref().unwrap().text, "the preview is the report");
    assert!(text.contains("Kind: FileOp Delete, PermissionDenied"), "{text}");
    assert!(text.contains("Message: You don't have access to <path>"), "{text}");
    assert!(!text.contains("uploads") && !text.contains("/var/www"), "{text}");
}

#[test]
fn the_sheets_buttons_take_tab_in_order_and_escape_cancels() {
    let mut rt = runtime(scene("report"));
    let mut order = Vec::new();
    for _ in 0..8 {
        key(&mut rt, Key::Tab, Mods::default());
        let k = focused_key(&rt);
        if order.first() == Some(&k) {
            break;
        }
        order.push(k);
    }
    assert!(order.iter().all(|k| k.starts_with("report-")), "Tab stays in the sheet: {order:?}");
    let at = order.iter().position(|k| k == "report-copy").unwrap_or_else(|| panic!("{order:?}"));
    order.rotate_left(at);
    assert_eq!(order, ["report-copy", "report-cancel", "report-open"], "in reading order, once each");
    let here = order.iter().position(|k| *k == focused_key(&rt)).expect("focus in the sheet");
    key(&mut rt, Key::Tab, SHIFT);
    assert_eq!(focused_key(&rt), order[(here + order.len() - 1) % order.len()], "Shift+Tab goes back");
    key(&mut rt, Key::Escape, Mods::default());
    assert!(!shown(&rt, "report"));
    assert!(rt.take_url_requests().is_empty(), "nothing opened");
}

#[test]
fn a_screen_reader_gets_a_named_dialog_with_its_buttons() {
    let rt = runtime(scene("report"));
    let nodes = tree(&rt);
    let dialog = nodes.iter().find(|n| n.key == "report").expect("the sheet");
    assert_eq!((dialog.role.as_str(), dialog.name.as_str()), ("dialog", "Report a Bug"));
    let role = |k: &str| nodes.iter().find(|n| n.key == k).map(|n| (n.role.clone(), n.name.clone()));
    assert_eq!(role("report-copy"), Some(("button".into(), "Copy report".into())));
    assert_eq!(role("report-cancel"), Some(("button".into(), "Cancel".into())));
    assert_eq!(role("report-open").map(|r| r.1), Some("Open GitHub".into()));
}

#[test]
fn return_opens_github_with_the_report_and_closes_the_sheet() {
    let mut rt = runtime(scene("report"));
    let url = rt.state.report.draft.as_ref().unwrap().url.clone();
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.take_url_requests(), [url]);
    assert!(!shown(&rt, "report"));
}

#[test]
fn copy_report_puts_the_text_on_the_clipboard_and_says_copied() {
    let mut rt = runtime(scene("report"));
    let board = rune::clipboard::memory();
    rt.use_clipboard(&board);
    let text = rt.state.report.draft.as_ref().unwrap().text.clone();
    click(&mut rt, "report-copy");
    assert_eq!(board.text().map(|t| t.to_string()), Some(text));
    assert!(shown(&rt, "report"), "the sheet stays");
    let copied = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("report-copy")).unwrap();
    assert_eq!(copied.label.as_deref(), Some("Copied"));
    click(&mut rt, "report-cancel");
    assert!(!shown(&rt, "report"));
}

/// prod's connection failed, ssh's reason on screen.
fn connect_failed(s: &mut App) {
    scenes::connecting(s);
    let status = ConnStatus::Failed("ssh: Could not resolve hostname prod.example.com".into());
    s.apply(ServiceEvent::Status { host: HostId("prod".into()), status });
}

#[test]
fn the_connection_failed_screens_report_says_the_connection_failed_without_sshs_words() {
    let mut rt = runtime(connect_failed);
    click(&mut rt, "state-report");
    let text = text_of(&rt, "report-text");
    assert!(text.contains("Kind: Connection Failed\n  Host: host-1"), "{text}");
    assert!(!text.contains("prod"), "{text}");
}

/// An update to 9.9.0 that failed to install.
fn update_failed(s: &mut App) {
    scenes::listing(s);
    s.backend = Box::new(Silent::default());
    let version = Version::parse("9.9.0").unwrap();
    let offer = Offer {
        page: "https://github.com/fatihdogmus/dromon/releases/tag/9.9.0".into(),
        method: Method::Install(Package::Dmg),
        version,
    };
    s.apply(ServiceEvent::Update(UpdateEvent::Available(offer)));
    s.act(Action::Update(UpdateAction::Update));
    s.apply(ServiceEvent::Update(UpdateEvent::InstallFailed("couldn't write /Users/zelda/Library/Caches".into())));
}

#[test]
fn a_failed_updates_report_names_the_version_and_not_the_folder() {
    let mut rt = runtime(update_failed);
    click(&mut rt, "toast-update-report");
    let text = text_of(&rt, "report-text");
    assert!(text.contains("What: installing the update to 9.9.0 failed\n  Kind: Update InstallFailed"), "{text}");
    assert!(!text.contains("zelda"), "{text}");
}
