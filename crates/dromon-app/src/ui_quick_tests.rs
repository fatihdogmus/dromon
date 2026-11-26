//! Quick connect, the host menu and Add to SSH config driven headless through the real view, on
//! the demo fixture: clicks, typing, pastes, Return, Escape and the file picker's answer.

use std::path::PathBuf;

use dromon_core::{ConnStatus, HostId, HostSource};
use rune::{Env, FileDialogKind, Key, KeyEvent, Mods, Runtime};

use crate::state::{App, Sheet};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

fn runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.use_clipboard(&rune::clipboard::memory());
    rt.update(|s, _| *s = scenes::base());
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn right_click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    settle(rt);
}

fn key(rt: &mut Runtime<App>, k: Key) {
    rt.key(KeyEvent { key: k, mods: Mods::default() });
    settle(rt);
}

fn type_text(rt: &mut Runtime<App>, s: &str) {
    key(rt, Key::Text(s.into()));
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

fn field(rt: &Runtime<App>, f: dromon_ssh::QuickField) -> String {
    rt.state.ui.quick.edit(f).text.clone()
}

#[test]
fn the_plus_opens_quick_connect_and_a_pasted_address_connects() {
    let mut rt = runtime();
    click(&mut rt, "sb-quick-connect");
    assert!(shown(&rt, "quick"));
    assert_eq!(rt.presented_focus_key(), Some("quick-host-input"), "Host has the keyboard");
    rt.paste("deploy@prod.example.com:22");
    settle(&mut rt);
    use dromon_ssh::QuickField::{Host, Port, User};
    assert_eq!(
        (field(&rt, Host), field(&rt, User), field(&rt, Port)),
        ("prod.example.com".into(), "deploy".into(), "22".into())
    );
    key(&mut rt, Key::Enter);
    assert!(!shown(&rt, "quick"), "Return connects and closes it");
    let new = HostId("deploy@prod.example.com:22".into());
    assert_eq!(rt.state.active, Some(new.clone()));
    assert_eq!(rt.state.conn().unwrap().status, ConnStatus::Ready);
    assert!(rt.find("rows/index.html").is_some(), "prod's site lists");
    assert!(shown(&rt, "host-deploy@prod.example.com:22"));
    assert_eq!(rt.state.hosts.get(&new).map(|h| h.source), Some(HostSource::Recent));
}

#[test]
fn typing_a_bad_port_shows_its_error_and_escape_closes() {
    let mut rt = runtime();
    click(&mut rt, "welcome-quick-connect");
    assert!(shown(&rt, "quick"), "the welcome button opens it too");
    type_text(&mut rt, "10.0.0.9");
    click(&mut rt, "quick-port-input");
    type_text(&mut rt, "0");
    click(&mut rt, "quick-connect");
    assert!(shown(&rt, "quick"));
    assert_eq!(text_of(&rt, "quick-port-error"), "Use a number from 1 to 65535");
    assert_eq!(rt.presented_focus_key(), Some("quick-port-input"), "the field with the problem has the keyboard");
    assert_eq!(rt.state.active, None);
    type_text(&mut rt, "22");
    assert!(!shown(&rt, "quick-port-error"), "typing clears it");
    key(&mut rt, Key::Escape);
    assert!(!shown(&rt, "quick"));
}

#[test]
fn more_holds_the_name_and_the_identity_files_picker() {
    let mut rt = runtime();
    click(&mut rt, "sb-quick-connect");
    type_text(&mut rt, "10.0.0.7");
    assert!(!shown(&rt, "quick-alias-input"));
    click(&mut rt, "quick-more");
    assert!(shown(&rt, "quick-alias-input") && shown(&rt, "quick-identity-input") && shown(&rt, "quick-jump-input"));
    click(&mut rt, "quick-pick-identity");
    let (kind, _) = rt.file_dialog().expect("the picker is open");
    assert_eq!(kind, FileDialogKind::Open);
    rt.resolve_file_dialog(Some(PathBuf::from("/keys/id_lab"))).unwrap();
    settle(&mut rt);
    assert_eq!(field(&rt, dromon_ssh::QuickField::IdentityFile), "/keys/id_lab");
    click(&mut rt, "quick-alias-input");
    type_text(&mut rt, "lab");
    click(&mut rt, "quick-save");
    assert!(!shown(&rt, "quick"));
    assert!(shown(&rt, "host-lab"), "saved: it's in the sidebar");
    let lab = rt.state.hosts.get(&HostId("lab".into())).unwrap();
    assert_eq!((lab.source, lab.identity_file.as_deref()), (HostSource::Saved, Some("/keys/id_lab")));
    assert_eq!(rt.state.active, None, "Save doesn't connect");
}

#[test]
fn a_saved_hosts_menu_adds_it_to_the_ssh_config() {
    let mut rt = runtime();
    right_click(&mut rt, "host-box");
    assert!(shown(&rt, "ctx-menu"));
    click(&mut rt, "ctx-host-add-to-config");
    assert!(matches!(rt.state.ui.sheets.open(), Some(Sheet::AddToConfig(_))));
    assert_eq!(text_of(&rt, "sheet-title"), "Add box to your SSH config");
    assert_eq!(text_of(&rt, "add-config-block-text"), "Host box\n    HostName 10.0.0.4\n    User fatih");
    assert_eq!(text_of(&rt, "add-config-target"), "~/.ssh/config");
    assert_eq!(text_of(&rt, "add-config-backup"), "~/.ssh/config.dromon-backup-20261125-1400");
    key(&mut rt, Key::Enter);
    assert!(!shown(&rt, "add-config"), "Return adds it");
    assert_eq!(rt.state.hosts.get(&HostId("box".into())).map(|h| h.source), Some(HostSource::SshConfig));
    // Under SSH Config now, after build-box.
    let (_, y_box) = rt.find("host-box").unwrap();
    let (_, y_build) = rt.find("host-build-box").unwrap();
    let (_, y_media) = rt.find("host-media").unwrap();
    assert!(y_build < y_box && y_box < y_media, "{y_build} < {y_box} < {y_media}");
}

#[test]
fn edit_opens_on_the_saved_settings_and_return_saves() {
    let mut rt = runtime();
    right_click(&mut rt, "host-box");
    click(&mut rt, "ctx-host-edit");
    assert_eq!(text_of(&rt, "quick-title"), "Edit box");
    assert_eq!(field(&rt, dromon_ssh::QuickField::Host), "10.0.0.4");
    click(&mut rt, "quick-port-input");
    type_text(&mut rt, "2200");
    key(&mut rt, Key::Enter);
    assert!(!shown(&rt, "quick"));
    assert_eq!(rt.state.hosts.get(&HostId("box".into())).unwrap().port, Some(2200));
    assert_eq!(rt.state.active, None, "editing saves; it doesn't connect");
}

#[test]
fn a_recent_hosts_menu_saves_or_removes_it() {
    let mut rt = runtime();
    right_click(&mut rt, "host-10.0.0.12");
    let items: Vec<String> =
        rt.elements().into_iter().filter(|e| !e.exiting && e.role == "menuitem").filter_map(|e| e.label).collect();
    assert_eq!(items, ["Connect", "Save", "Remove"]);
    click(&mut rt, "ctx-host-remove");
    assert!(!shown(&rt, "host-10.0.0.12"));
    right_click(&mut rt, "host-203.0.113.7");
    click(&mut rt, "ctx-host-save");
    assert_eq!(rt.state.hosts.get(&HostId("203.0.113.7".into())).map(|h| h.source), Some(HostSource::Saved));
}
