//! `App`-level updates against the demo fixture: actions in, state and effects out.

use dromon_core::{ConnStatus, Entry, EntryKind, HostId, RemotePath, SortDir, SortKey};

use super::*;
use crate::fixture;

fn app() -> App {
    fixture::app(fixture::demo())
}

fn prod() -> HostId {
    HostId("prod".into())
}

fn names(s: &App) -> Vec<String> {
    s.conn().unwrap().row_names().into_iter().map(str::to_owned).collect()
}

fn selected(s: &App) -> Vec<String> {
    let c = s.conn().unwrap();
    let rows = c.row_names();
    rows.iter().filter(|n| c.selection.contains(n)).map(|n| (*n).to_owned()).collect()
}

fn cwd(s: &App) -> &str {
    s.conn().unwrap().cwd.as_str()
}

#[test]
fn selecting_a_host_connects_and_lists_its_start() {
    let mut s = app();
    assert!(s.conn().is_none());
    s.act(Action::SelectHost(prod()));
    let c = s.conn().unwrap();
    assert_eq!(c.status, ConnStatus::Ready);
    assert_eq!(c.cwd.as_str(), "/var/www/site");
    assert!(matches!(c.listing, Listing::Ready(_)));
    assert!(!c.history.can_back(), "the starting folder isn't a visit");
    let sections = s.hosts.sections(&s.conns);
    assert_eq!(sections[0].0, Section::Connected);
    assert_eq!(sections[0].1[0].alias, "prod");
}

#[test]
fn unknown_hosts_are_ignored() {
    let mut s = app();
    s.act(Action::SelectHost(HostId("nope".into())));
    assert!(s.active.is_none());
}

#[test]
fn a_silent_backend_leaves_the_connection_connecting_until_events_arrive() {
    let mut s = app();
    s.backend = Box::new(Silent::default());
    s.act(Action::SelectHost(prod()));
    assert_eq!(s.conn().unwrap().status, ConnStatus::Connecting);
    assert_eq!(s.conn().unwrap().listing, Listing::Loading);
    // The service thread's answers, applied by hand.
    s.apply(ServiceEvent::Opened { host: prod(), start: RemotePath::new("/srv") });
    assert_eq!(s.conn().unwrap().status, ConnStatus::Ready);
    let entries = vec![Entry {
        name: "media".into(),
        kind: EntryKind::Dir,
        size: 0,
        mtime: None,
        mode: None,
        owner: None,
        group: None,
    }];
    s.apply(ServiceEvent::Listing { host: prod(), path: RemotePath::new("/srv"), entries });
    assert_eq!(names(&s), ["media"]);
}

#[test]
fn open_back_forward_and_up() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    let fx = s.act(Action::Open("assets".into()));
    assert_eq!(cwd(&s), "/var/www/site/assets");
    assert!(fx.contains(&Effect::ScrollTop));
    // Files don't navigate.
    s.act(Action::Back);
    s.act(Action::Open("index.html".into()));
    assert_eq!(cwd(&s), "/var/www/site");
    s.act(Action::Forward);
    assert_eq!(cwd(&s), "/var/www/site/assets");
    // Up selects the folder you came out of.
    let fx = s.act(Action::GoUp);
    assert_eq!(cwd(&s), "/var/www/site");
    assert_eq!(selected(&s), ["assets"]);
    assert!(fx.contains(&Effect::Reveal("assets".into())));
}

#[test]
fn symlinked_folders_open_through_the_link() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    s.act(Action::Open("current".into()));
    assert_eq!(cwd(&s), "/var/www/site/current");
    assert!(!names(&s).is_empty());
}

#[test]
fn breadcrumbs_and_errors() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    s.act(Action::GoTo(RemotePath::new("/root")));
    assert_eq!(s.conn().unwrap().listing, Listing::Failed(ListError::PermissionDenied));
    s.act(Action::GoUp);
    assert_eq!(cwd(&s), "/");
    assert_eq!(selected(&s), ["root"]);
}

#[test]
fn return_opens_the_selected_folder() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    s.act(Action::Click { name: "releases".into(), extend: false, toggle: false });
    s.shortcut(Shortcut::Open, 0.0);
    assert_eq!(cwd(&s), "/var/www/site/releases");
}

#[test]
fn clicks_build_the_selection() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    s.act(Action::Click { name: "app.min.js".into(), extend: false, toggle: false });
    s.act(Action::Click { name: "index.html".into(), extend: true, toggle: false });
    assert_eq!(selected(&s), ["app.min.js", "backup-2026-11-01.tar.gz", "favicon.ico", "index.html"]);
    s.act(Action::Click { name: "favicon.ico".into(), extend: false, toggle: true });
    assert_eq!(selected(&s), ["app.min.js", "backup-2026-11-01.tar.gz", "index.html"]);
}

#[test]
fn arrows_move_and_reveal() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    let fx = s.shortcut(Shortcut::Move { delta: 1, extend: false }, 0.0);
    assert_eq!(selected(&s), [".well-known"]);
    assert_eq!(fx, [Effect::Reveal(".well-known".into())]);
    s.shortcut(Shortcut::Move { delta: 1, extend: true }, 0.0);
    assert_eq!(selected(&s), [".well-known", "assets"]);
    s.shortcut(Shortcut::End { last: true, extend: false }, 0.0);
    assert_eq!(selected(&s), ["server.log"]);
    s.shortcut(Shortcut::SelectAll, 0.0);
    assert_eq!(selected(&s).len(), 17);
}

#[test]
fn type_to_select() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    s.shortcut(Shortcut::Type("r".into()), 1.0);
    assert_eq!(selected(&s), ["releases"]);
    s.shortcut(Shortcut::Type("o".into()), 1.2);
    assert_eq!(selected(&s), ["robots.txt"]);
    // After a pause a new prefix starts.
    s.shortcut(Shortcut::Type("s".into()), 5.0);
    assert_eq!(selected(&s), ["server.log"]);
}

#[test]
fn sorting_is_global_and_keeps_folders_first() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    s.act(Action::Sort(SortKey::Size));
    assert_eq!(s.sort, Sort { key: SortKey::Size, dir: SortDir::Descending });
    let n = names(&s);
    let first_file = n.iter().position(|n| n == "backup-2026-11-01.tar.gz").unwrap();
    assert!(n[..first_file].iter().all(|n| s.conn().unwrap().find(n).unwrap().kind.is_dir_like()));
    s.act(Action::Sort(SortKey::Size));
    assert_eq!(s.sort.dir, SortDir::Ascending);
    assert_eq!(names(&s).last().map(String::as_str), Some("backup-2026-11-01.tar.gz"));
    // A new connection picks up the remembered sort.
    s.act(Action::SelectHost(HostId("nas".into())));
    assert_eq!(s.conn().unwrap().sort, s.sort);
}

#[test]
fn hidden_files_toggle_everywhere() {
    let mut s = app();
    s.act(Action::SelectHost(HostId("nas".into())));
    s.act(Action::SelectHost(prod()));
    assert!(names(&s).contains(&".env".to_owned()));
    s.act(Action::ToggleHidden);
    assert!(!names(&s).iter().any(|n| n.starts_with('.')));
    assert!(!s.conns[&HostId("nas".into())].show_hidden);
    s.act(Action::ToggleHidden);
    assert!(names(&s).contains(&".env".to_owned()));
}

#[test]
fn filter_opens_narrows_and_closes() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    s.act(Action::OpenFilter);
    assert!(s.conn().unwrap().filter.is_on());
    assert!(s.ui.filter_edit.focused);
    s.act(Action::FilterChanged("json".into()));
    assert_eq!(names(&s), ["package-lock.json", "package.json"]);
    let fx = s.shortcut(Shortcut::Escape, 0.0);
    assert!(fx.contains(&Effect::Blur));
    assert_eq!(s.conn().unwrap().filter, Filter::Off);
    assert_eq!(names(&s).len(), 17);
}

#[test]
fn nothing_navigates_while_reconnecting() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    s.act(Action::Open("assets".into()));
    s.act(Action::GoUp);
    assert_eq!(cwd(&s), "/var/www/site");
    // Reconnected: the last listing is still there, and it works again.
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Ready });
    s.act(Action::Open("assets".into()));
    assert_eq!(cwd(&s), "/var/www/site/assets");
}

#[test]
fn discovered_hosts_replace_the_ssh_config_section_only() {
    let mut s = app();
    let mut spec = dromon_core::HostSpec::from_config_alias("lab");
    spec.hostname = Some("10.1.1.1".into());
    s.apply(ServiceEvent::Hosts(vec![spec]));
    let config: Vec<&str> = s
        .hosts
        .hosts
        .iter()
        .filter(|h| h.source == dromon_core::HostSource::SshConfig)
        .map(|h| h.alias.as_str())
        .collect();
    assert_eq!(config, ["lab"]);
    assert!(s.hosts.get(&HostId("box".into())).is_some(), "saved hosts stay");
    assert!(s.hosts.get(&prod()).is_none());
}

#[test]
fn the_log_is_stamped_and_outlives_a_failed_connection() {
    let mut s = app();
    s.backend = Box::new(Silent::default());
    s.wall_ms = || 1_000;
    s.act(Action::SelectHost(prod()));
    s.apply(ServiceEvent::Log { host: prod(), line: "Permission denied (password).".into() });
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Failed("Permission denied (password).".into()) });
    assert_eq!(s.conn().unwrap().log.to_text(), "00:00:01.000  Permission denied (password).");
    // Trying again keeps the log.
    s.act(Action::SelectHost(prod()));
    assert_eq!(s.conn().unwrap().status, ConnStatus::Connecting);
    assert_eq!(s.conn().unwrap().log.len(), 1);
}

#[test]
fn answering_and_disconnecting_send_commands() {
    let rec = Recorder::default();
    let mut s = app();
    s.backend = Box::new(rec.clone());
    s.act(Action::SelectHost(prod()));
    let prompt = dromon_core::Prompt {
        id: dromon_core::PromptId(9),
        host: prod(),
        kind: dromon_core::PromptKind::Password,
        text: "pw: ".into(),
        error: None,
    };
    s.apply(ServiceEvent::Prompt(prompt));
    s.ui.sheets.edit_mut().replace_text("hunter2");
    s.act(Action::SubmitPrompt);
    s.act(Action::Disconnect(prod()));
    assert!(!s.ui.sheets.is_open(), "disconnecting closes the host's sheet");
    let answer = dromon_core::PromptAnswer::Text("hunter2".into());
    assert_eq!(
        rec.sent()[1..],
        [Command::AnswerPrompt { id: dromon_core::PromptId(9), answer }, Command::Disconnect { host: prod() }]
    );
}

#[test]
fn every_demo_host_opens() {
    for h in fixture::demo().hosts.clone() {
        let mut s = app();
        s.act(Action::SelectHost(h.id.clone()));
        let c = s.conn().unwrap();
        assert!(matches!(c.listing, Listing::Ready(_)), "{} at {}: {:?}", h.alias, c.cwd, c.listing);
    }
}

#[test]
fn every_scene_builds() {
    for (name, scene) in crate::scenes::SCENES {
        let mut s = app();
        scene(&mut s);
        // Quick connect, the saved hosts' and the first Settings page's scenes are over the
        // welcome state.
        let welcome = [
            "welcome",
            "quick-connect",
            "quick-connect-more",
            "quick-connect-error",
            "add-to-config",
            "host-menu",
            "settings",
            "settings-general",
        ];
        if !welcome.contains(name) {
            assert!(s.conn().is_some(), "{name}");
        }
    }
}

fn prompt_of(id: u64, host: &str) -> dromon_core::Prompt {
    dromon_core::Prompt {
        id: dromon_core::PromptId(id),
        host: HostId(host.into()),
        kind: dromon_core::PromptKind::Password,
        text: "pw: ".into(),
        error: None,
    }
}

fn open_prompt(s: &App) -> Option<u64> {
    s.ui.sheets.open().and_then(Sheet::prompt).map(|p| p.id.0)
}

#[test]
fn sign_in_brings_a_queued_prompt_forward() {
    let rec = Recorder::default();
    let mut s = app();
    s.backend = Box::new(rec.clone());
    s.act(Action::SelectHost(prod()));
    let sent = rec.sent().len();
    s.apply(ServiceEvent::Prompt(prompt_of(1, "other")));
    s.apply(ServiceEvent::Prompt(prompt_of(2, "prod")));
    assert_eq!(open_prompt(&s), Some(1));
    s.act(Action::SignIn(prod()));
    assert_eq!(open_prompt(&s), Some(2), "prod's prompt is the open sheet");
    assert_eq!(
        s.ui.sheets.queued().map(|p| p.id.0).collect::<Vec<_>>(),
        [1],
        "the other host's goes back to the front of the queue"
    );
    assert!(s.ui.sheets.edit().is_some_and(|e| e.focused), "with its field focused");
    // Again: it's open already, nothing moves, nothing is asked of the service.
    s.act(Action::SignIn(prod()));
    assert_eq!(open_prompt(&s), Some(2));
    assert_eq!(rec.sent().len(), sent, "no command");
}

#[test]
fn sign_in_without_a_prompt_here_asks_the_service_for_it() {
    let rec = Recorder::default();
    let mut s = app();
    s.backend = Box::new(rec.clone());
    s.act(Action::SelectHost(prod()));
    s.act(Action::SignIn(prod()));
    assert_eq!(rec.sent().last(), Some(&Command::ShowPrompts { host: prod() }));
    // A host this window has no connection to: nothing to ask about.
    let sent = rec.sent().len();
    s.act(Action::SignIn(HostId("nowhere".into())));
    assert_eq!(rec.sent().len(), sent);
}

#[test]
fn the_context_menu_key_opens_the_cursor_rows_menu_else_the_empty_one() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    // No cursor row yet.
    s.act(Action::Escape);
    assert_eq!(s.shortcut(Shortcut::ContextMenu, 0.0), [Effect::ContextMenu(MenuKind::Empty)]);
    let name = names(&s)[0].clone();
    s.act(Action::Click { name: name.clone(), extend: false, toggle: false });
    assert_eq!(s.shortcut(Shortcut::ContextMenu, 0.0), [Effect::ContextMenu(MenuKind::Row(name.clone()))]);
    // Not while a text field has the keyboard.
    s.act(Action::OpenFilter);
    assert!(s.text_focused());
    assert!(s.shortcut(Shortcut::ContextMenu, 0.0).is_empty());
    s.act(Action::CloseFilter);
    // Not while a sheet or a menu is open.
    s.act(Action::File(FileAction::OpenMenu { kind: MenuKind::Empty, at: Some((1.0, 1.0)) }));
    assert!(s.shortcut(Shortcut::ContextMenu, 0.0).is_empty(), "a menu is open");
    s.act(Action::File(FileAction::CloseMenu));
    s.apply(ServiceEvent::Prompt(prompt_of(5, "other")));
    assert!(s.shortcut(Shortcut::ContextMenu, 0.0).is_empty(), "a sheet is open");
}
