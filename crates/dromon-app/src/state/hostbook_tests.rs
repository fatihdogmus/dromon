//! Quick connect, saved and recent hosts, the host menu and Add to SSH config, on the demo
//! fixture (whose backend keeps saved hosts and the "ssh config" in memory).

use std::cell::RefCell;
use std::rc::Rc;

use dromon_core::{ConnStatus, HostId, HostSource, HostSpec};
use dromon_ssh::QuickField;

use super::*;
use crate::fixture;
use crate::state::prefs::{Saved, Store};
use crate::state::{Action, App, Command, Listing, MenuKind, Recorder, Section, ServiceEvent, Sheet, Shortcut};

fn id(s: &str) -> HostId {
    HostId(s.into())
}

fn demo() -> App {
    fixture::app(fixture::demo())
}

fn host(s: &mut App, a: HostAction) {
    s.act(Action::HostBook(a));
}

/// Opens the popover and fills `fields`.
fn fill(s: &mut App, fields: &[(QuickField, &str)]) {
    s.act(Action::QuickConnect);
    for (f, text) in fields {
        s.ui.quick.set_text(*f, text);
    }
}

fn section(s: &App, which: Section) -> Vec<String> {
    s.hosts
        .sections(&s.conns)
        .into_iter()
        .filter(|(sec, _)| *sec == which)
        .flat_map(|(_, h)| h.into_iter().map(|h| h.id.0.clone()))
        .collect()
}

fn source(s: &App, h: &str) -> Option<HostSource> {
    s.hosts.get(&id(h)).map(|h| h.source)
}

#[derive(Clone, Default)]
struct Recorded(Rc<RefCell<Vec<Saved>>>);

impl Store for Recorded {
    fn save(&mut self, saved: &Saved) {
        self.0.borrow_mut().push(saved.clone());
    }
}

#[test]
fn quick_connect_reaches_the_server_and_becomes_the_newest_recent_host() {
    let mut s = demo();
    fill(&mut s, &[(QuickField::Host, "deploy@prod.example.com")]);
    host(&mut s, HostAction::Connect);
    let new = id("deploy@prod.example.com:22");
    assert!(!s.ui.quick.open, "the popover closes");
    assert_eq!(s.active, Some(new.clone()));
    let c = s.conn().unwrap();
    assert_eq!(c.status, ConnStatus::Ready);
    assert!(matches!(&c.listing, Listing::Ready(_)) && c.find("index.html").is_some(), "prod's server lists");
    let h = s.hosts.get(&new).unwrap();
    assert_eq!(
        (h.source, h.alias.as_str(), h.user.as_deref()),
        (HostSource::Recent, "prod.example.com", Some("deploy"))
    );
    assert_eq!(s.recent_hosts()[0].id, new);
    assert_eq!(section(&s, Section::Connected), [new.as_str()]);
    // The same server typed another way is the same recent host, moved to the front.
    s.act(Action::Disconnect(new.clone()));
    fill(&mut s, &[(QuickField::Host, "prod.example.com"), (QuickField::User, "deploy"), (QuickField::Port, "22")]);
    host(&mut s, HostAction::Connect);
    let recent: Vec<HostId> = s.recent_hosts().into_iter().map(|h| h.id).collect();
    assert_eq!(recent.iter().filter(|r| **r == new).count(), 1, "{recent:?}");
    assert_eq!(recent[0], new);
}

#[test]
fn recent_hosts_are_capped_remembered_and_checked_again_when_read_back() {
    let mut s = demo();
    let store = Recorded::default();
    s.remember(Saved::default(), Box::new(store.clone()));
    for i in 0..12 {
        fill(&mut s, &[(QuickField::Host, &format!("10.9.0.{i}"))]);
        host(&mut s, HostAction::Connect);
    }
    let recent = s.recent_hosts();
    assert_eq!(recent.len(), MAX_RECENT);
    assert_eq!(recent[0].id, id("10.9.0.11:22"));
    let saved = store.0.borrow().last().cloned().unwrap();
    assert_eq!(saved.recent.len(), MAX_RECENT);
    assert_eq!(saved.recent[0].hostname, "10.9.0.11");
    assert!(!format!("{saved:?}").contains("password"), "addresses only");

    // The next launch shows them again; one that no longer passes the checks is dropped.
    let mut saved = saved;
    saved.recent[1].hostname = "-oProxyCommand=sh".into();
    let sent = Recorder::default();
    let mut next = demo();
    next.backend = Box::new(sent.clone());
    next.remember(saved, Box::new(crate::state::NoStore));
    let ids: Vec<HostId> = next.recent_hosts().into_iter().map(|h| h.id).collect();
    assert_eq!(ids.len(), MAX_RECENT - 1);
    assert_eq!(ids[0], id("10.9.0.11:22"));
    assert!(!ids.contains(&id("10.9.0.10:22")));
    assert!(
        matches!(&sent.sent()[..], [Command::HostBook(HostBookCommand::Recent(list))] if list.len() == MAX_RECENT - 1)
    );
}

#[test]
fn connect_with_a_saved_hosts_settings_connects_the_saved_host() {
    let mut s = demo();
    fill(&mut s, &[(QuickField::Host, "fatih@10.0.0.4")]);
    host(&mut s, HostAction::Connect);
    assert_eq!(s.active, Some(id("box")), "box is fatih@10.0.0.4");
    assert!(s.recent_hosts().iter().all(|h| h.hostname.as_deref() != Some("10.0.0.4")));
}

#[test]
fn bad_fields_keep_the_popover_and_send_nothing() {
    let mut s = demo();
    let sent = Recorder::default();
    s.backend = Box::new(sent.clone());
    fill(&mut s, &[(QuickField::Host, "-oProxyCommand=sh"), (QuickField::Port, "99999")]);
    host(&mut s, HostAction::Connect);
    assert!(s.ui.quick.open);
    assert_eq!(s.ui.quick.errors.keys().copied().collect::<Vec<_>>(), [QuickField::Host, QuickField::Port]);
    host(&mut s, HostAction::Save);
    assert!(sent.sent().is_empty(), "nothing reaches the service: {:?}", sent.sent());
    assert_eq!(s.active, None);
}

#[test]
fn save_adds_a_saved_host_and_names_must_be_free() {
    let mut s = demo();
    fill(&mut s, &[(QuickField::Host, "kbd@10.0.0.7:2222"), (QuickField::Alias, "lab")]);
    host(&mut s, HostAction::Save);
    assert!(!s.ui.quick.open);
    assert_eq!(section(&s, Section::Saved), ["box", "media", "lab"]);
    assert_eq!(s.active, None, "Save doesn't connect");
    let lab = s.hosts.get(&id("lab")).unwrap();
    assert_eq!((lab.hostname.as_deref(), lab.user.as_deref(), lab.port), (Some("10.0.0.7"), Some("kbd"), Some(2222)));

    // "prod" is an SSH Config host's name, "media" a saved one's.
    for taken in ["prod", "media"] {
        fill(&mut s, &[(QuickField::Host, "10.0.0.8"), (QuickField::Alias, taken)]);
        host(&mut s, HostAction::Save);
        assert!(s.ui.quick.open, "{taken}");
        assert!(s.ui.quick.errors[&QuickField::Alias].contains("already a host"), "{:?}", s.ui.quick.errors);
        host(&mut s, HostAction::CloseQuick);
    }
    // No name: the hostname is its name.
    fill(&mut s, &[(QuickField::Host, "10.0.0.8")]);
    host(&mut s, HostAction::Save);
    assert_eq!(source(&s, "10.0.0.8"), Some(HostSource::Saved));
}

#[test]
fn edit_replaces_the_saved_host_and_remove_takes_it_away() {
    let mut s = demo();
    host(&mut s, HostAction::Edit(id("box")));
    assert!(s.ui.quick.open);
    assert_eq!(s.ui.quick.input().host, "10.0.0.4");
    s.ui.quick.set_text(QuickField::Alias, "box2");
    s.ui.quick.set_text(QuickField::Port, "2200");
    host(&mut s, HostAction::Save);
    assert_eq!(section(&s, Section::Saved), ["box2", "media"], "renamed in place");
    assert_eq!(s.hosts.get(&id("box2")).unwrap().port, Some(2200));
    // Editing a host keeps its own name free for it.
    host(&mut s, HostAction::Edit(id("media")));
    host(&mut s, HostAction::Save);
    assert!(!s.ui.quick.open);
    host(&mut s, HostAction::Remove(id("media")));
    assert_eq!(section(&s, Section::Saved), ["box2"]);
    host(&mut s, HostAction::Remove(id("10.0.0.12")));
    assert_eq!(section(&s, Section::Recent), ["203.0.113.7"]);
    host(&mut s, HostAction::Remove(id("prod")));
    assert_eq!(source(&s, "prod"), Some(HostSource::SshConfig), "SSH Config hosts aren't ours to remove");
}

#[test]
fn a_recent_hosts_save_moves_it_to_saved() {
    let mut s = demo();
    host(&mut s, HostAction::SaveRecent(id("203.0.113.7")));
    assert_eq!(source(&s, "203.0.113.7"), Some(HostSource::Saved));
    assert_eq!(section(&s, Section::Recent), ["10.0.0.12"]);
}

#[test]
fn the_host_menu_fits_each_section() {
    let mut s = demo();
    let titles = |s: &App, h: &str| -> Vec<&'static str> {
        s.menu(&MenuKind::Host(id(h))).iter().map(|e| e.item().map_or("—", |i| i.title)).collect()
    };
    assert_eq!(titles(&s, "prod"), ["Connect"]);
    assert_eq!(titles(&s, "box"), ["Connect", "—", "Add to SSH config…", "Edit…", "—", "Remove"]);
    assert_eq!(titles(&s, "203.0.113.7"), ["Connect", "—", "Save", "Remove"]);
    s.act(Action::SelectHost(id("box")));
    assert_eq!(titles(&s, "box")[0], "Disconnect");
    assert_eq!(s.menu_action(&MenuKind::Host(id("box")), "host-disconnect"), Some(Action::Disconnect(id("box"))));
    assert_eq!(s.menu_action(&MenuKind::Host(id("box")), "host-connect"), None, "not in the menu now");
    assert_eq!(s.menu_action(&MenuKind::Host(id("prod")), "host-edit"), None);
    // Picking from the drawn menu.
    s.act(Action::File(crate::state::FileAction::OpenMenu {
        kind: MenuKind::Host(id("media")),
        at: Some((10.0, 10.0)),
    }));
    s.act(Action::File(crate::state::FileAction::Pick { kind: MenuKind::Host(id("media")), id: "host-edit".into() }));
    assert!(s.files.menu.is_none());
    assert_eq!(s.ui.quick.editing, Some(id("media")));
}

fn sheet(s: &App) -> Option<&AddToConfigView> {
    match s.ui.sheets.open() {
        Some(Sheet::AddToConfig(v)) => Some(v),
        _ => None,
    }
}

#[test]
fn add_to_config_shows_the_block_then_moves_the_host_to_ssh_config() {
    let mut s = demo();
    host(&mut s, HostAction::AddToConfig(id("box")));
    let v = sheet(&s).expect("the sheet");
    let p = v.preview.as_ref().expect("the fixture answers at once");
    assert_eq!(p.block, "Host box\n    HostName 10.0.0.4\n    User fatih\n");
    assert_eq!(p.target, "~/.ssh/config");
    assert_eq!(p.backup.as_deref(), Some("~/.ssh/config.dromon-backup-20261125-1400"));
    host(&mut s, HostAction::ConfirmAddToConfig);
    assert!(sheet(&s).is_none(), "done: the sheet closes");
    assert_eq!(source(&s, "box"), Some(HostSource::SshConfig));
    assert_eq!(section(&s, Section::Saved), ["media"]);
    assert!(section(&s, Section::SshConfig).contains(&"box".to_owned()));
    // It connects as an SSH Config host now.
    s.act(Action::SelectHost(id("box")));
    assert_eq!(s.conn().unwrap().status, ConnStatus::Ready);
    // The next one in the same minute gets the next backup name.
    host(&mut s, HostAction::AddToConfig(id("media")));
    assert_eq!(
        sheet(&s).unwrap().preview.as_ref().unwrap().backup.as_deref(),
        Some("~/.ssh/config.dromon-backup-20261125-1400-2")
    );
    s.act(Action::DismissSheet);
    assert!(sheet(&s).is_none());
    assert_eq!(source(&s, "media"), Some(HostSource::Saved), "Cancel changes nothing");
}

#[test]
fn add_to_config_problems_show_in_the_sheet() {
    let mut s = demo();
    let sent = Recorder::default();
    s.backend = Box::new(sent.clone());
    host(&mut s, HostAction::AddToConfig(id("box")));
    assert_eq!(sent.sent(), [Command::HostBook(HostBookCommand::PreviewAddToConfig(id("box")))]);
    host(&mut s, HostAction::ConfirmAddToConfig);
    assert_eq!(sent.sent().len(), 1, "nothing to confirm before the preview");
    let preview = ConfigPreview { target: "~/.ssh/config".into(), backup: None, block: "Host box\n".into() };
    s.apply(ServiceEvent::HostBook(HostBookEvent::Preview { host: id("box"), preview: Ok(preview) }));
    host(&mut s, HostAction::ConfirmAddToConfig);
    host(&mut s, HostAction::ConfirmAddToConfig);
    assert_eq!(sent.sent().len(), 2, "one Add while busy");
    assert!(sheet(&s).unwrap().busy);
    s.apply(ServiceEvent::HostBook(HostBookEvent::AddFailed {
        host: id("box"),
        error: "Couldn't write ~/.ssh/config: denied".into(),
    }));
    let v = sheet(&s).unwrap();
    assert!(!v.busy);
    assert_eq!(v.error.as_deref(), Some("Couldn't write ~/.ssh/config: denied"));
    // Without its sheet, a failure is a toast.
    s.act(Action::DismissSheet);
    s.apply(ServiceEvent::HostBook(HostBookEvent::AddFailed { host: id("box"), error: "nope".into() }));
    s.apply(ServiceEvent::HostBook(HostBookEvent::SaveFailed("disk full".into())));
    let toasts: Vec<String> = s.toasts().into_iter().map(|t| t.text).collect();
    assert_eq!(toasts, ["Couldn't add box to your ssh config: nope", "Couldn't save your hosts: disk full"]);
}

#[test]
fn a_config_edit_keeps_the_host_in_use_and_one_host_per_id() {
    let mut s = demo();
    s.act(Action::SelectHost(id("prod")));
    // prod went from the ssh config while connected: it stays, the others follow the file.
    s.apply(ServiceEvent::Hosts(vec![HostSpec::from_config_alias("nas"), HostSpec::from_config_alias("new")]));
    assert_eq!(s.active, Some(id("prod")));
    assert_eq!(source(&s, "prod"), Some(HostSource::SshConfig));
    assert_eq!(section(&s, Section::Connected), ["prod"]);
    assert_eq!(section(&s, Section::SshConfig), ["nas", "new"]);
    assert_eq!(s.conn().unwrap().status, ConnStatus::Ready, "the connection is untouched");
    // Once it's closed and another host is on screen, the next discovery lets it go.
    s.act(Action::Disconnect(id("prod")));
    s.act(Action::SelectHost(id("nas")));
    s.apply(ServiceEvent::Hosts(vec![HostSpec::from_config_alias("nas")]));
    assert_eq!(source(&s, "prod"), None);
    // A saved host whose name the ssh config now has is the config's.
    s.apply(ServiceEvent::Hosts(vec![HostSpec::from_config_alias("nas"), HostSpec::from_config_alias("media")]));
    assert_eq!(source(&s, "media"), Some(HostSource::SshConfig));
    assert_eq!(s.hosts.hosts.iter().filter(|h| h.id == id("media")).count(), 1);
}

#[test]
fn cmd_shift_k_opens_quick_connect_and_the_list_keys_wait() {
    let mut s = demo();
    s.act(Action::SelectHost(id("prod")));
    s.shortcut(Shortcut::QuickConnect, 0.0);
    assert!(s.ui.quick.open);
    assert!(s.text_focused(), "Host has the keyboard");
    s.shortcut(Shortcut::SelectAll, 0.0);
    assert!(s.conn().unwrap().selection.is_empty(), "the list's keys don't reach behind it");
    s.shortcut(Shortcut::Escape, 0.0);
    assert!(!s.ui.quick.open);
}

#[test]
fn trace_lines_name_the_event_only() {
    let preview = ConfigPreview { target: "~/.ssh/secret".into(), backup: None, block: "Host secret".into() };
    let lines: Vec<String> = [
        HostBookEvent::Saved(vec![HostSpec::from_config_alias("a")]),
        HostBookEvent::SaveFailed("secret".into()),
        HostBookEvent::Preview { host: id("box"), preview: Ok(preview) },
        HostBookEvent::Added { host: id("box") },
        HostBookEvent::AddFailed { host: id("box"), error: "secret".into() },
    ]
    .iter()
    .map(trace_line)
    .collect();
    assert_eq!(
        lines,
        ["Saved count=1", "SaveFailed", "Preview host=box ok=true", "Added host=box", "AddFailed host=box"]
    );
}

#[test]
fn upsert_replaces_in_place() {
    let h = |a: &str| HostSpec { source: HostSource::Saved, ..HostSpec::from_config_alias(a) };
    let mut list = vec![h("a"), h("b"), h("c")];
    upsert(&mut list, h("x"), Some(&id("b")));
    assert_eq!(list.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(), ["a", "x", "c"]);
    upsert(&mut list, h("c"), None);
    assert_eq!(list.len(), 3);
    upsert(&mut list, h("d"), None);
    assert_eq!(list.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(), ["a", "x", "c", "d"]);
}
