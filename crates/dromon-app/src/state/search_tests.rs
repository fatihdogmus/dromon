//! Search in subfolders: the matcher, the machine's steps, and in the app starting a search
//! (debounced typing, the scope switch, ⌘⇧F), streaming results, the cap, every way it ends
//! (each cancelling its walk), stale answers, hidden files, and acting on results. On the demo
//! fixture (which searches at once) or a [`Recorder`] to read what's sent and answer by hand.

use dromon_core::{ConnStatus, Entry, EntryKind, HostId, RemotePath};

use super::*;
use crate::fixture;
use crate::state::{Action, App, FileAction, Recorder, ServiceEvent, Shortcut};

fn prod() -> HostId {
    HostId("prod".into())
}

fn site() -> RemotePath {
    RemotePath::new("/var/www/site")
}

fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::SelectHost(prod()));
    s
}

fn recorded() -> (App, Recorder) {
    let mut s = demo();
    let rec = Recorder::default();
    s.backend = Box::new(rec.clone());
    (s, rec)
}

fn searches_sent(rec: &Recorder) -> Vec<Command> {
    rec.sent().into_iter().filter(|c| matches!(c, Command::Search(_))).collect()
}

fn act(s: &mut App, a: SearchAction) -> Vec<Effect> {
    s.act(Action::Search(a))
}

/// ⌘⇧F, then `text` typed at `now`.
fn typed(s: &mut App, text: &str, now: f64) {
    if !s.ui.search.is_on() && s.ui.search.scope != Scope::Subfolders {
        act(s, SearchAction::Open);
    }
    s.ui.filter_edit = rune::TextEdit::new(text);
    act(s, SearchAction::Typed { text: text.into(), now });
}

/// `text` searched for: typed, and the pause after it.
fn searched(s: &mut App, text: &str) {
    typed(s, text, 10.0);
    act(s, SearchAction::Tick(10.0 + DEBOUNCE));
}

fn hits(s: &App) -> Vec<String> {
    s.ui.search.found.iter().map(|f| format!("{}/{}", f.folder, f.entry.name)).collect()
}

fn file(name: &str) -> Entry {
    Entry { name: name.into(), kind: EntryKind::File, size: 1, mtime: None, mode: None, owner: None, group: None }
}

fn found(folder: &str, name: &str) -> Found {
    Found { folder: folder.into(), entry: file(name) }
}

fn sent_id(rec: &Recorder) -> SearchId {
    rec.sent()
        .iter()
        .rev()
        .find_map(|c| if let Command::Search(SearchCommand::Start { id, .. }) = c { Some(*id) } else { None })
        .expect("a search was sent")
}

#[test]
fn a_query_matches_part_of_a_name_ignoring_case_or_a_glob_of_the_whole_name() {
    let sub = Matcher::new("LOG");
    assert!(sub.matches("server.log") && sub.matches("Logs") && sub.matches("catalog.txt"));
    assert!(!sub.matches("lo.g"));
    let glob = Matcher::new("*.LOG");
    assert!(glob.matches("server.log") && glob.matches(".log"));
    assert!(!glob.matches("server.log.1"), "a glob matches the whole name");
    assert!(!glob.matches("catalog.txt"));
    let dated = Matcher::new("backup-202?-*");
    assert!(dated.matches("backup-2026-11-01.tar.gz"));
    assert!(!dated.matches("backup-26-11.tar") && !dated.matches("old-backup-2026-1"));
    assert!(Matcher::new("a*b*c").matches("aXXbYYbZc") && !Matcher::new("a*b*c").matches("aXXbYY"));
    assert!(Matcher::new("?").matches("ä") && !Matcher::new("?").matches("ab"));
    assert!(Matcher::new("ÄRG").matches("ärger.txt"), "case folds beyond ASCII");
}

#[test]
fn the_machine_waits_for_typing_to_pause_then_searches_and_streams() {
    let mut s = Search::default();
    assert_eq!(step(&mut s, Event::Typed { query: "lo".into(), now: 1.0 }), []);
    assert_eq!(s.due(), Some(1.0 + DEBOUNCE));
    assert_eq!(step(&mut s, Event::Typed { query: "log".into(), now: 1.1 }), [], "typing on waits on");
    assert_eq!(step(&mut s, Event::Tick { now: 1.2, next: SearchId(1) }), [], "not yet");
    let out = step(&mut s, Event::Tick { now: 1.1 + DEBOUNCE, next: SearchId(1) });
    assert_eq!(out, [Out::Send { id: SearchId(1), query: "log".into() }], "one search, for what was typed last");
    assert_eq!(s.phase, Phase::Searching { id: SearchId(1), folders: 0 });
    step(&mut s, Event::Found { id: SearchId(1), entries: vec![found(".", "a.log")], folders: 3 });
    step(&mut s, Event::Found { id: SearchId(1), entries: vec![found("x/y", "b.log")], folders: 9 });
    assert_eq!(s.phase, Phase::Searching { id: SearchId(1), folders: 9 });
    assert_eq!(s.found, [found(".", "a.log"), found("x/y", "b.log")], "results join in the order found");
    step(&mut s, Event::Done { id: SearchId(1), folders: 12, unreadable: 2, capped: false });
    assert_eq!(s.phase, Phase::Done { id: SearchId(1), folders: 12, unreadable: 2, capped: false });
    assert_eq!(status_text(&s), "2 results in 12 folders");
    assert_eq!(unreadable_text(&s).as_deref(), Some("2 folders couldn't be read"));
}

#[test]
fn typing_again_cancels_the_walk_and_keeps_the_last_results_until_the_next_starts() {
    let mut s = Search::default();
    step(&mut s, Event::Start { id: SearchId(1), query: "a".into() });
    step(&mut s, Event::Found { id: SearchId(1), entries: vec![found(".", "a")], folders: 1 });
    assert_eq!(step(&mut s, Event::Typed { query: "ab".into(), now: 5.0 }), [Out::Cancel(SearchId(1))]);
    assert_eq!(s.found.len(), 1, "what it found stays while the next waits");
    step(&mut s, Event::Tick { now: 6.0, next: SearchId(2) });
    assert!(s.found.is_empty(), "a new search starts empty");
    assert_eq!(step(&mut s, Event::Typed { query: String::new(), now: 7.0 }), [Out::Cancel(SearchId(2))]);
    assert_eq!(s.phase, Phase::Idle, "clearing the field ends it");
}

#[test]
fn answers_for_a_search_that_isnt_running_change_nothing() {
    let mut s = Search::default();
    assert_eq!(step(&mut s, Event::Found { id: SearchId(1), entries: vec![found(".", "a")], folders: 1 }), []);
    assert_eq!(s.phase, Phase::Idle);
    step(&mut s, Event::Start { id: SearchId(2), query: "a".into() });
    step(&mut s, Event::Found { id: SearchId(1), entries: vec![found(".", "old")], folders: 1 });
    step(&mut s, Event::Done { id: SearchId(1), folders: 1, unreadable: 0, capped: false });
    assert_eq!(s.phase, Phase::Searching { id: SearchId(2), folders: 0 }, "an older search's answers are stale");
    assert!(s.found.is_empty());
    step(&mut s, Event::Failed { id: SearchId(2), error: dromon_core::Error::ConnectionLost });
    assert!(matches!(s.phase, Phase::Failed { .. }));
    assert_eq!(status_text(&s), "Search stopped: The connection was lost");
    step(&mut s, Event::Found { id: SearchId(2), entries: vec![found(".", "late")], folders: 1 });
    assert!(s.found.is_empty(), "nothing after the end");
}

#[test]
fn the_status_line_counts_folders_then_results_and_says_when_it_stopped_at_the_cap() {
    let mut s = Search::default();
    step(&mut s, Event::Start { id: SearchId(1), query: "f".into() });
    step(&mut s, Event::Found { id: SearchId(1), entries: Vec::new(), folders: 1_204 });
    assert_eq!(status_text(&s), "Searching… 1,204 folders");
    let many: Vec<Found> = (0..CAP).map(|i| found(".", &format!("f{i}"))).collect();
    step(&mut s, Event::Found { id: SearchId(1), entries: many, folders: 1_300 });
    step(&mut s, Event::Done { id: SearchId(1), folders: 1_300, unreadable: 0, capped: true });
    assert_eq!(status_text(&s), "First 5,000 results in 1,300 folders; the search stopped there");
    assert_eq!(unreadable_text(&s), None);
    let one = Search {
        phase: Phase::Done { id: SearchId(2), folders: 1, unreadable: 1, capped: false },
        ..Search::default()
    };
    assert_eq!(status_text(&one), "0 results in 1 folder");
    assert_eq!(finished(&one), Some((SearchId(2), "0 results in 1 folder. 1 folder couldn't be read".into())));
}

#[test]
fn typing_in_subfolders_searches_the_folder_on_screen_once_typing_pauses() {
    let (mut s, rec) = recorded();
    act(&mut s, SearchAction::Open);
    assert_eq!(s.ui.search.scope, Scope::Subfolders);
    assert!(s.ui.filter_edit.focused, "the field has the keyboard");
    typed(&mut s, "lo", 1.0);
    typed(&mut s, "log", 1.1);
    act(&mut s, SearchAction::Tick(1.2));
    assert!(searches_sent(&rec).is_empty(), "nothing until typing pauses");
    act(&mut s, SearchAction::Tick(1.1 + DEBOUNCE));
    let id = sent_id(&rec);
    assert_eq!(
        searches_sent(&rec),
        [Command::Search(SearchCommand::Start {
            id,
            host: prod(),
            root: site(),
            query: "log".into(),
            show_hidden: true
        })]
    );
    assert_eq!(id.0 >> 32, u64::from(s.window.0), "ids are the window's");
    assert!(s.results().is_some());
    assert_eq!(s.conn().unwrap().filter, Filter::On(String::new()), "the folder view isn't filtered by it");
}

#[test]
fn the_demo_streams_results_with_their_folders() {
    let mut s = demo();
    searched(&mut s, "*.json");
    let mut got = hits(&s);
    got.sort();
    assert_eq!(got, ["./package-lock.json", "./package.json", "releases/20261125-1348/package.json"]);
    assert!(matches!(s.ui.search.phase, Phase::Done { unreadable: 0, capped: false, .. }), "{:?}", s.ui.search.phase);
}

#[test]
fn a_finished_search_is_announced_once() {
    let mut s = demo();
    let mut announcer = crate::state::focus::Announcer::default();
    assert_eq!(announcer.news(&s), None);
    searched(&mut s, "*.json");
    assert_eq!(announcer.news(&s).as_deref(), Some("3 results in 10 folders"));
    assert_eq!(announcer.news(&s), None, "said once");
}

#[test]
fn hidden_entries_follow_the_hidden_files_setting_and_a_toggle_searches_again() {
    let mut s = demo();
    searched(&mut s, "env");
    assert_eq!(hits(&s), ["./.env"]);
    s.act(Action::ToggleHidden);
    assert!(s.results().is_some(), "still searching");
    assert_eq!(hits(&s), Vec::<String>::new(), "the search ran again without hidden files");
    assert!(matches!(s.ui.search.phase, Phase::Done { .. }));
}

#[test]
fn switching_the_scope_searches_at_once_and_switching_back_filters_the_folder_again() {
    let (mut s, rec) = recorded();
    s.act(Action::OpenFilter);
    s.ui.filter_edit = rune::TextEdit::new("index");
    s.act(Action::FilterChanged("index".into()));
    assert_eq!(s.conn().unwrap().filter, Filter::On("index".into()));
    act(&mut s, SearchAction::Scope(Scope::Subfolders));
    let id = sent_id(&rec);
    assert_eq!(searches_sent(&rec).len(), 1, "no wait: the text was there");
    assert_eq!(s.conn().unwrap().filter, Filter::On(String::new()));
    act(&mut s, SearchAction::Scope(Scope::Folder));
    assert_eq!(searches_sent(&rec).last(), Some(&Command::Search(SearchCommand::Cancel { id, host: prod() })));
    assert!(s.results().is_none());
    assert_eq!(s.conn().unwrap().filter, Filter::On("index".into()), "This folder filters by the text again");
}

/// A search running on the recorder, and the id it was sent as.
fn running() -> (App, Recorder, SearchId) {
    let (mut s, rec) = recorded();
    searched(&mut s, "log");
    let id = sent_id(&rec);
    let entries = vec![found(".", "server.log")];
    s.apply(ServiceEvent::Search(SearchEvent::Found { id, host: prod(), entries, folders: 1 }));
    (s, rec, id)
}

fn cancelled(rec: &Recorder, id: SearchId) -> bool {
    searches_sent(rec).last() == Some(&Command::Search(SearchCommand::Cancel { id, host: prod() }))
}

#[test]
fn escape_ends_the_search_cancels_its_walk_and_the_folder_view_is_as_it_was() {
    let (mut s, rec) = recorded();
    s.act(Action::Click { name: "README.md".into(), extend: false, toggle: false });
    searched(&mut s, "log");
    let id = sent_id(&rec);
    s.act(Action::Escape);
    assert!(cancelled(&rec, id));
    assert!(s.results().is_none());
    let c = s.conn().unwrap();
    assert_eq!((c.cwd.clone(), c.filter.clone()), (site(), Filter::Off));
    assert!(c.selection.contains("README.md"), "the folder's selection is as it was");
    assert_eq!(s.ui.search.scope, Scope::Folder, "the next ⌘F filters this folder");
}

#[test]
fn clearing_the_field_ends_the_search() {
    let (mut s, rec, id) = running();
    typed(&mut s, "", 20.0);
    assert!(cancelled(&rec, id));
    assert!(s.results().is_none());
    assert!(s.conn().unwrap().filter.is_on(), "the field stays open");
}

#[test]
fn navigating_ends_the_search() {
    let (mut s, rec, id) = running();
    s.act(Action::GoUp);
    assert!(cancelled(&rec, id));
    assert!(s.results().is_none());
}

#[test]
fn choosing_another_host_ends_the_search() {
    let (mut s, rec, id) = running();
    s.act(Action::SelectHost(HostId("staging".into())));
    assert!(cancelled(&rec, id));
    assert!(s.results().is_none());
}

#[test]
fn a_disconnect_ends_the_search() {
    let (mut s, rec, id) = running();
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Disconnected });
    assert!(cancelled(&rec, id));
    assert!(s.results().is_none());
}

#[test]
fn a_search_that_already_ended_isnt_cancelled() {
    let (mut s, rec, id) = running();
    s.apply(ServiceEvent::Search(SearchEvent::Done { id, host: prod(), folders: 3, unreadable: 0, capped: false }));
    s.act(Action::Escape);
    assert!(!searches_sent(&rec).iter().any(|c| matches!(c, Command::Search(SearchCommand::Cancel { .. }))));
}

#[test]
fn opening_a_file_result_opens_its_folder_with_it_selected() {
    let mut s = demo();
    searched(&mut s, "app.min");
    let key = |s: &App, folder: &str| {
        let i = s.ui.search.found.iter().position(|f| f.folder == folder).expect("the result");
        hit_key(i)
    };
    let deep = key(&s, "releases/20261125-1348");
    s.act(Action::Click { name: deep, extend: false, toggle: false });
    s.act(Action::OpenSelection);
    assert!(s.results().is_none(), "the search ended");
    let c = s.conn().unwrap();
    assert_eq!(c.cwd, RemotePath::new("/var/www/site/releases/20261125-1348"));
    assert_eq!(c.selection.cursor(), Some("app.min.js"));
    // One in the folder searched: the folder view, the file selected.
    s.act(Action::GoTo(site()));
    searched(&mut s, "app.min");
    let here = key(&s, ".");
    s.act(Action::Open(here));
    let c = s.conn().unwrap();
    assert_eq!((c.cwd.clone(), c.filter.clone(), c.selection.cursor()), (site(), Filter::Off, Some("app.min.js")));
    assert!(s.results().is_none());
}

#[test]
fn opening_a_folder_result_opens_it() {
    let mut s = demo();
    searched(&mut s, "2026112");
    let i = s.ui.search.found.iter().position(|f| f.entry.name == "20261125-1348").expect("the folder");
    s.act(Action::Open(hit_key(i)));
    assert_eq!(s.conn().unwrap().cwd, RemotePath::new("/var/www/site/releases/20261125-1348"));
    assert!(s.results().is_none());
}

#[test]
fn results_take_the_lists_keys_menus_and_downloads_but_not_its_changes() {
    let (mut s, rec) = recorded();
    searched(&mut s, "log");
    let id = sent_id(&rec);
    let entries = vec![found(".", "server.log"), found("logs/2026", "jan.log"), found("logs", "old.log")];
    s.apply(ServiceEvent::Search(SearchEvent::Found { id, host: prod(), entries, folders: 3 }));
    s.act(Action::Move { delta: 1, extend: false });
    s.act(Action::Move { delta: 1, extend: true });
    assert_eq!(s.ui.search.selection.len(), 2);
    assert!(s.conn().unwrap().selection.is_empty(), "the folder view's selection is left alone");
    let copied = s.act(Action::File(FileAction::CopyPath));
    assert_eq!(copied, [Effect::Copy("/var/www/site/server.log\n/var/www/site/logs/2026/jan.log".into())]);
    s.act(Action::Download);
    let started = rec.sent().into_iter().find_map(|c| match c {
        Command::Transfer(super::super::TransferCommand::Start(dromon_transfer::JobRequest::Download {
            items,
            ..
        })) => Some(items.into_iter().map(|i| i.path.to_string()).collect::<Vec<_>>()),
        _ => None,
    });
    assert_eq!(started, Some(vec!["/var/www/site/server.log".into(), "/var/www/site/logs/2026/jan.log".into()]));
    // Delete and rename belong to the folder view, out of sight.
    s.act(Action::File(FileAction::Delete));
    assert!(!s.ui.sheets.is_open());
    // The menu: one result, its Properties.
    s.act(Action::Click { name: hit_key(2), extend: false, toggle: false });
    let menu = s.menu(&MenuKind::Hit(hit_key(2)));
    let ids: Vec<&str> = menu.iter().filter_map(|e| e.item()).map(|i| i.id).collect();
    assert_eq!(ids, ["download", "copy-path", "properties"]);
    s.act(Action::File(FileAction::Pick { kind: MenuKind::Hit(hit_key(2)), id: "properties".into() }));
    let v = s.ui.sheets.properties().expect("the Properties sheet");
    assert_eq!((v.folder.clone(), v.entry.name.as_str()), (RemotePath::new("/var/www/site/logs"), "old.log"));
}

#[test]
fn cmd_shift_f_opens_the_filter_in_subfolders_and_cmd_f_back_in_this_folder() {
    let mut s = demo();
    s.shortcut(Shortcut::Search, 0.0);
    assert!(s.conn().unwrap().filter.is_on());
    assert_eq!(s.ui.search.scope, Scope::Subfolders);
    searched(&mut s, "log");
    assert!(s.results().is_some());
    s.shortcut(Shortcut::Filter, 0.0);
    assert_eq!(s.ui.search.scope, Scope::Folder);
    assert!(s.results().is_none());
}
