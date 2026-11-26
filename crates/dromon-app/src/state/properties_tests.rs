//! The Properties sheet: its size machine, its texts, and in the app opening it, Calculate's
//! walk (progress, the total, failures), cancelling a walk whose sheet closed and ignoring
//! answers that aren't the sheet's. On the demo fixture (which walks at once) or a [`Recorder`]
//! to read what's sent and drive the answers by hand.

use dromon_core::{ConnStatus, Entry, EntryKind, Error, HostId, LinkTarget, Prompt, PromptId, PromptKind, RemotePath};

use super::*;
use crate::fixture;
use crate::state::menu::MenuKind;
use crate::state::{Action, App, FileAction, Recorder, Sheet};

fn prod() -> HostId {
    HostId("prod".into())
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

fn click(s: &mut App, name: &str) {
    s.act(Action::Click { name: name.into(), extend: false, toggle: false });
}

fn file(s: &mut App, a: FileAction) {
    s.act(Action::File(a));
}

/// The sheet on `name`, opened from its row's menu.
fn opened(s: &mut App, name: &str) {
    let kind = MenuKind::Row(name.into());
    file(s, FileAction::OpenMenu { kind: kind.clone(), at: Some((10.0, 10.0)) });
    file(s, FileAction::Pick { kind, id: "properties".into() });
}

fn view(s: &App) -> &PropertiesView {
    s.ui.sheets.properties().expect("the Properties sheet")
}

fn sizes_sent(rec: &Recorder) -> Vec<Command> {
    rec.sent().into_iter().filter(|c| matches!(c, Command::FolderSize { .. } | Command::CancelFolderSize { .. })).collect()
}

fn total(bytes: u64, files: u64, folders: u64, unreadable: u64) -> SizeTotal {
    SizeTotal { bytes, files, folders, unreadable }
}

#[test]
fn the_size_machine_runs_one_walk_at_a_time_and_ignores_stale_answers() {
    let mut phase = SizePhase::Idle;
    assert!(!step(&mut phase, SizeEvent::Progress(SizeId(1), total(1, 1, 0, 0))), "nothing is running");
    assert!(step(&mut phase, SizeEvent::Start(SizeId(1))));
    assert_eq!(phase, SizePhase::Calculating { id: SizeId(1), found: SizeTotal::default() });
    assert!(!step(&mut phase, SizeEvent::Start(SizeId(2))), "one walk at a time");
    assert!(step(&mut phase, SizeEvent::Progress(SizeId(1), total(10, 2, 1, 0))));
    assert!(!step(&mut phase, SizeEvent::Done(SizeId(9), total(0, 0, 0, 0))), "another walk's answer");
    assert_eq!(phase, SizePhase::Calculating { id: SizeId(1), found: total(10, 2, 1, 0) });
    assert!(step(&mut phase, SizeEvent::Done(SizeId(1), total(30, 3, 1, 1))));
    assert_eq!(phase, SizePhase::Done { total: total(30, 3, 1, 1) });
    assert!(!step(&mut phase, SizeEvent::Progress(SizeId(1), total(1, 1, 1, 1))), "late progress after the end");
    // Recalculate, and a failure.
    assert!(step(&mut phase, SizeEvent::Start(SizeId(2))));
    assert!(step(&mut phase, SizeEvent::Failed(SizeId(2), total(5, 1, 0, 0), Error::ConnectionLost)));
    assert_eq!(phase, SizePhase::Failed { total: total(5, 1, 0, 0), error: Error::ConnectionLost });
    assert!(step(&mut phase, SizeEvent::Start(SizeId(3))), "and again after a failure");
}

fn entry(kind: EntryKind, mode: Option<u32>) -> Entry {
    Entry { name: "x".into(), kind, size: 0, mtime: None, mode, owner: None, group: None }
}

#[test]
fn kinds_read_as_words() {
    assert_eq!(kind_text(&entry(EntryKind::Dir, None)), "Folder");
    assert_eq!(kind_text(&entry(EntryKind::File, None)), "File");
    assert_eq!(kind_text(&entry(EntryKind::Symlink { target: Some(LinkTarget::Dir) }, None)), "Symbolic link to a folder");
    assert_eq!(kind_text(&entry(EntryKind::Symlink { target: None }, None)), "Symbolic link (target missing)");
    assert_eq!(kind_text(&entry(EntryKind::Other, Some(0o140_755))), "Socket");
    assert_eq!(kind_text(&entry(EntryKind::Other, Some(0o010_644))), "Named pipe");
    assert_eq!(kind_text(&entry(EntryKind::Other, None)), "Special file");
}

#[test]
fn permissions_read_as_letters_and_octal() {
    assert_eq!(permissions_text(Some(0o040_755)), "rwxr-xr-x (755)");
    assert_eq!(permissions_text(Some(0o100_644)), "rw-r--r-- (644)");
    assert_eq!(permissions_text(Some(0o100_600)), "rw------- (600)");
    assert_eq!(permissions_text(Some(0o104_755)), "rwsr-xr-x (4755)");
    assert_eq!(permissions_text(Some(0o041_777)), "rwxrwxrwt (1777)");
    assert_eq!(permissions_text(Some(0o102_640)), "rw-r-S--- (2640)");
    assert_eq!(permissions_text(None), "—");
}

#[test]
fn owners_read_as_owner_and_group() {
    let mut e = entry(EntryKind::File, None);
    assert_eq!(owner_text(&e), "—");
    e.owner = Some("alice".into());
    assert_eq!(owner_text(&e), "alice");
    e.group = Some("staff".into());
    assert_eq!(owner_text(&e), "alice:staff");
    e.owner = None;
    assert_eq!(owner_text(&e), ":staff");
    let ids = Entry { owner: Some("1001".into()), group: Some("1002".into()), ..e };
    assert_eq!(owner_text(&ids), "1001:1002", "numeric ids when the server gave no names");
}

#[test]
fn counts_read_as_words() {
    assert_eq!(counts_text(&total(0, 1, 1, 0)), "1 file, 1 folder");
    assert_eq!(counts_text(&total(0, 12_000, 3, 0)), "12,000 files, 3 folders");
    assert_eq!(unreadable_text(&total(0, 0, 0, 0)), None);
    assert_eq!(unreadable_text(&total(0, 0, 0, 1)).as_deref(), Some("1 folder couldn't be read"));
    assert_eq!(unreadable_text(&total(0, 0, 0, 2)).as_deref(), Some("2 folders couldn't be read"));
    assert_eq!(calculating_text(&total(0, 1, 0, 0)), "Calculating… 1 item");
    assert_eq!(calculating_text(&total(0, 1_200, 4, 0)), "Calculating… 1,204 items");
}

#[test]
fn it_opens_for_exactly_one_selected_row() {
    let mut s = demo();
    file(&mut s, FileAction::Properties);
    assert!(!s.ui.sheets.is_open(), "nothing selected");
    click(&mut s, "index.html");
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: true });
    file(&mut s, FileAction::Properties);
    assert!(!s.ui.sheets.is_open(), "two selected");
    click(&mut s, "index.html");
    file(&mut s, FileAction::Properties);
    let v = view(&s);
    assert_eq!((v.host.as_str(), v.folder.as_str(), v.entry.name.as_str()), ("prod", "/var/www/site", "index.html"));
    assert!(!v.is_folder());
    assert!(!s.can_calculate(), "a file's size is the listing's");
    // Escape (DismissSheet) closes it.
    s.act(Action::DismissSheet);
    assert!(!s.ui.sheets.is_open());
    // From the row menu: right-click selects the row, Properties shows it.
    opened(&mut s, "releases");
    assert!(view(&s).is_folder());
    assert_eq!(view(&s).size, SizePhase::Idle);
    // One sheet at a time.
    file(&mut s, FileAction::Properties);
    assert_eq!(view(&s).entry.name, "releases");
}

#[test]
fn calculate_shows_progress_then_the_total() {
    let (mut s, rec) = recorded();
    opened(&mut s, "releases");
    assert!(s.can_calculate());
    file(&mut s, FileAction::Calculate);
    let id = view(&s).walking().expect("walking");
    assert_eq!(id.0 >> 32, u64::from(s.window.0), "numbered in this window's own range");
    assert_eq!(sizes_sent(&rec), [Command::FolderSize { id, host: prod(), path: RemotePath::new("/var/www/site/releases") }]);
    assert!(!s.can_calculate(), "not twice at once");
    file(&mut s, FileAction::Calculate);
    assert_eq!(sizes_sent(&rec).len(), 1);

    s.apply(ServiceEvent::FolderSizeProgress { id, host: prod(), total: total(14_812, 1, 2, 0) });
    assert_eq!(view(&s).size, SizePhase::Calculating { id, found: total(14_812, 1, 2, 0) });
    s.apply(ServiceEvent::FolderSizeDone { id, host: prod(), total: total(303_365, 3, 4, 0) });
    assert_eq!(view(&s).size, SizePhase::Done { total: total(303_365, 3, 4, 0) });

    // Recalculate: a new walk with a new id; closing after it ended cancels nothing.
    assert!(s.can_calculate());
    file(&mut s, FileAction::Calculate);
    let again = view(&s).walking().expect("walking again");
    assert_ne!(again, id);
    s.apply(ServiceEvent::FolderSizeFailed { id: again, host: prod(), total: total(9, 1, 0, 0), error: Error::ConnectionLost });
    assert_eq!(view(&s).size, SizePhase::Failed { total: total(9, 1, 0, 0), error: Error::ConnectionLost });
    s.act(Action::DismissSheet);
    assert_eq!(sizes_sent(&rec).len(), 2, "nothing to cancel: {:?}", sizes_sent(&rec));
}

#[test]
fn closing_the_sheet_cancels_its_walk_once() {
    let (mut s, rec) = recorded();
    opened(&mut s, "releases");
    file(&mut s, FileAction::Calculate);
    let id = view(&s).walking().unwrap();
    s.act(Action::DismissSheet);
    assert_eq!(sizes_sent(&rec).last(), Some(&Command::CancelFolderSize { id, host: prod() }));
    // Its late answers are ignored, and nothing is cancelled twice.
    s.apply(ServiceEvent::FolderSizeProgress { id, host: prod(), total: total(1, 1, 0, 0) });
    s.apply(ServiceEvent::FolderSizeDone { id, host: prod(), total: total(1, 1, 0, 0) });
    assert!(!s.ui.sheets.is_open());
    click(&mut s, "index.html");
    assert_eq!(sizes_sent(&rec).len(), 2);
    // A stale answer doesn't touch the next sheet either.
    opened(&mut s, "releases");
    s.apply(ServiceEvent::FolderSizeDone { id, host: prod(), total: total(1, 1, 0, 0) });
    assert_eq!(view(&s).size, SizePhase::Idle);
}

#[test]
fn a_prompt_or_a_disconnect_taking_the_sheet_away_cancels_its_walk() {
    let (mut s, rec) = recorded();
    opened(&mut s, "releases");
    file(&mut s, FileAction::Calculate);
    let id = view(&s).walking().unwrap();
    let prompt = Prompt { id: PromptId(7), host: prod(), kind: PromptKind::Password, text: "pw: ".into(), error: None };
    s.apply(ServiceEvent::Prompt(prompt));
    assert!(matches!(s.ui.sheets.open(), Some(Sheet::Ask { .. })), "ssh's question comes first");
    assert_eq!(sizes_sent(&rec).last(), Some(&Command::CancelFolderSize { id, host: prod() }));

    let (mut s, rec) = recorded();
    opened(&mut s, "releases");
    file(&mut s, FileAction::Calculate);
    let id = view(&s).walking().unwrap();
    s.act(Action::Disconnect(prod()));
    assert!(!s.ui.sheets.is_open());
    assert!(sizes_sent(&rec).contains(&Command::CancelFolderSize { id, host: prod() }));
}

#[test]
fn answers_for_another_host_or_walk_change_nothing() {
    let (mut s, _rec) = recorded();
    opened(&mut s, "releases");
    file(&mut s, FileAction::Calculate);
    let id = view(&s).walking().unwrap();
    let nas = HostId("nas".into());
    s.apply(ServiceEvent::FolderSizeDone { id, host: nas, total: total(1, 1, 0, 0) });
    s.apply(ServiceEvent::FolderSizeDone { id: SizeId(id.0 + 1), host: prod(), total: total(1, 1, 0, 0) });
    assert_eq!(view(&s).walking(), Some(id), "still waiting for its own");
}

#[test]
fn calculate_waits_for_the_connection() {
    let (mut s, rec) = recorded();
    opened(&mut s, "releases");
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    assert!(s.ui.sheets.is_open(), "the sheet shows what the listing knew");
    assert!(!s.can_calculate());
    file(&mut s, FileAction::Calculate);
    assert!(sizes_sent(&rec).is_empty());
}

#[test]
fn the_fixture_walks_the_folder_at_once() {
    let mut s = demo();
    opened(&mut s, "releases");
    file(&mut s, FileAction::Calculate);
    // Three releases, one of them with an (empty) assets folder and three files.
    assert_eq!(view(&s).size, SizePhase::Done { total: total(14_812 + 286_410 + 2_143, 3, 4, 0) });
    // A folder with one that can't be read: the rest is counted, and how many weren't.
    s.act(Action::DismissSheet);
    s.act(Action::GoTo(RemotePath::new("/var")));
    opened(&mut s, "lib");
    file(&mut s, FileAction::Calculate);
    assert!(matches!(view(&s).size, SizePhase::Done { total: SizeTotal { unreadable: 1, folders: 1, .. } }), "{:?}", view(&s).size);
}
