//! File operations in the app state, on the demo fixture (which answers at once) or a
//! [`Recorder`] to read what's sent.

use dromon_core::{ConnStatus, EntryKind, Error, HostId, RemotePath};
use dromon_transfer::{JobRequest, RemoteItem};

use super::*;
use crate::fixture;
use crate::state::drag::DragTarget;
use crate::state::edit::Edit;
use crate::state::{Action, App, Command, Effect, Recorder, ServiceEvent, Sheet, TransferCommand};

const SITE: &str = "/var/www/site";

fn host(s: &str) -> HostId {
    HostId(s.into())
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// The demo with nas opened, then prod on screen at its site.
fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::SelectHost(host("nas")));
    s.act(Action::SelectHost(host("prod")));
    s
}

fn recorded() -> (App, Recorder) {
    let mut s = demo();
    let rec = Recorder::default();
    s.backend = Box::new(rec.clone());
    (s, rec)
}

fn file(s: &mut App, a: FileAction) -> Vec<Effect> {
    s.act(Action::File(a))
}

fn click(s: &mut App, name: &str) {
    s.act(Action::Click { name: name.into(), extend: false, toggle: false });
}

fn also(s: &mut App, name: &str) {
    s.act(Action::Click { name: name.into(), extend: false, toggle: true });
}

fn names(s: &App) -> Vec<String> {
    s.conn().unwrap().row_names().into_iter().map(str::to_owned).collect()
}

fn selected(s: &App) -> Vec<String> {
    let c = s.conn().unwrap();
    c.row_names().into_iter().filter(|n| c.selection.contains(n)).map(str::to_owned).collect()
}

fn type_in(s: &mut App, text: &str) {
    let e = s.files.field_edit();
    e.replace_text(text);
    e.mark_edited();
    file(s, FileAction::EditChanged);
}

fn field_text(s: &App) -> Option<(String, Option<String>)> {
    s.files.edit.field().map(|f| (f.edit.text.clone(), f.error.clone()))
}

fn started(rec: &Recorder) -> Vec<JobRequest> {
    rec.sent()
        .into_iter()
        .filter_map(|c| if let Command::Transfer(TransferCommand::Start(r)) = c { Some(r) } else { None })
        .collect()
}

fn file_ops(rec: &Recorder) -> Vec<FileOp> {
    rec.sent().into_iter().filter_map(|c| if let Command::FileOp { op, .. } = c { Some(op) } else { None }).collect()
}

fn item(path: &str, kind: EntryKind, size: u64) -> RemoteItem {
    RemoteItem { path: p(path), kind, size }
}

#[test]
fn a_new_folder_is_made_selected_and_scrolled_to() {
    let mut s = demo();
    let fx = file(&mut s, FileAction::NewFolder);
    assert!(fx.contains(&Effect::ScrollTop));
    assert_eq!(field_text(&s), Some(("New folder".into(), None)));
    file(&mut s, FileAction::EditSubmit);
    assert!(matches!(s.files.edit, Edit::Idle));
    assert!(names(&s).contains(&"New folder".to_owned()), "listed again with it");
    assert_eq!(selected(&s), ["New folder"]);
    assert_eq!(s.files.scroll.as_ref().map(|(_, n)| n.as_str()), Some("New folder"));
    let n = s.files.scroll.as_ref().unwrap().0;
    file(&mut s, FileAction::Scrolled(n));
    assert_eq!(s.files.scroll, None);
    // The next one takes the next free name.
    file(&mut s, FileAction::NewFolder);
    assert_eq!(field_text(&s), Some(("New folder (1)".into(), None)));
}

#[test]
fn a_taken_name_comes_back_as_an_inline_error_and_nothing_is_made() {
    let mut s = demo();
    let before = names(&s);
    file(&mut s, FileAction::NewFolder);
    type_in(&mut s, "assets");
    file(&mut s, FileAction::EditSubmit);
    assert_eq!(field_text(&s), Some(("assets".into(), Some("‘assets’ already exists here".into()))));
    assert!(matches!(s.files.edit, Edit::Editing(_)), "the field stays, editable");
    assert_eq!(names(&s), before);
    // A name that can't be one isn't even asked about.
    let (mut s, rec) = recorded();
    file(&mut s, FileAction::NewFolder);
    type_in(&mut s, "../escape");
    file(&mut s, FileAction::EditSubmit);
    assert!(file_ops(&rec).is_empty());
    assert_eq!(field_text(&s).and_then(|f| f.1).as_deref(), Some("A name can't contain “/”."));
}

#[test]
fn escape_makes_nothing_and_navigating_away_closes_the_field() {
    let (mut s, rec) = recorded();
    file(&mut s, FileAction::NewFolder);
    file(&mut s, FileAction::EditCancel);
    assert!(matches!(s.files.edit, Edit::Idle));
    file(&mut s, FileAction::NewFolder);
    s.act(Action::GoUp);
    assert!(matches!(s.files.edit, Edit::Idle), "its folder left the screen");
    assert!(file_ops(&rec).is_empty());
}

#[test]
fn rename_selects_the_stem_renames_and_keeps_the_item_selected() {
    let mut s = demo();
    click(&mut s, "README.md");
    let fx = file(&mut s, FileAction::Rename);
    assert_eq!(fx, [Effect::Reveal("README.md".into())]);
    let f = s.files.edit.field().unwrap();
    assert_eq!((f.edit.text.as_str(), f.edit.range()), ("README.md", (0, 6)));
    type_in(&mut s, "NOTES.md");
    file(&mut s, FileAction::EditSubmit);
    assert!(matches!(s.files.edit, Edit::Idle));
    assert!(!names(&s).contains(&"README.md".to_owned()));
    assert_eq!(selected(&s), ["NOTES.md"]);
}

#[test]
fn rename_to_the_same_name_cancels_and_to_a_taken_one_errs() {
    let (mut s, rec) = recorded();
    click(&mut s, "README.md");
    file(&mut s, FileAction::Rename);
    file(&mut s, FileAction::EditSubmit);
    assert!(matches!(s.files.edit, Edit::Idle));
    assert!(file_ops(&rec).is_empty(), "same name: nothing sent");

    let mut s = demo();
    click(&mut s, "README.md");
    file(&mut s, FileAction::Rename);
    type_in(&mut s, "robots.txt");
    file(&mut s, FileAction::EditSubmit);
    assert_eq!(field_text(&s).and_then(|f| f.1).as_deref(), Some("‘robots.txt’ already exists here"));
    assert!(names(&s).contains(&"README.md".to_owned()));
}

#[test]
fn rename_needs_exactly_one_row_and_a_relisting_without_it_closes_the_field() {
    let (mut s, _rec) = recorded();
    click(&mut s, "README.md");
    also(&mut s, "robots.txt");
    file(&mut s, FileAction::Rename);
    assert!(matches!(s.files.edit, Edit::Idle));
    click(&mut s, "README.md");
    file(&mut s, FileAction::Rename);
    assert!(s.files.edit.field().is_some());
    let entries: Vec<_> = s.conn().unwrap().entries().iter().filter(|e| e.name != "README.md").cloned().collect();
    s.apply(ServiceEvent::Listing { host: host("prod"), path: p(SITE), entries });
    assert!(matches!(s.files.edit, Edit::Idle), "the row went away");
}

#[test]
fn delete_asks_then_deletes_with_a_toast_until_done() {
    let (mut s, rec) = recorded();
    click(&mut s, "README.md");
    also(&mut s, "assets");
    file(&mut s, FileAction::Delete);
    let Some(Sheet::Delete { names, .. }) = s.ui.sheets.open() else { panic!("the delete sheet") };
    assert_eq!(names, &["assets".to_owned(), "README.md".to_owned()]);
    assert!(file_ops(&rec).is_empty(), "nothing until it's confirmed");
    file(&mut s, FileAction::ConfirmDelete);
    assert!(!s.ui.sheets.is_open());
    assert_eq!(file_ops(&rec), [FileOp::Delete { paths: vec![p("/var/www/site/assets"), p("/var/www/site/README.md")] }]);
    let id = s.ops.ops.last().unwrap().id;
    let texts = |s: &App| s.toasts().into_iter().map(|t| t.text).collect::<Vec<_>>();
    assert_eq!(texts(&s), ["Deleting 2 items…"]);
    s.apply(ServiceEvent::FileOpProgress { id, host: host("prod"), done: 1204, total: Some(5005) });
    assert_eq!(texts(&s), ["Deleting… 1,204 of 5,005"]);
    s.apply(ServiceEvent::FileOpDone { id, host: host("prod") });
    assert!(texts(&s).is_empty());
}

#[test]
fn a_failed_delete_keeps_its_toast_until_dismissed_and_several_stack() {
    let (mut s, _rec) = recorded();
    for name in ["assets", "uploads"] {
        click(&mut s, name);
        file(&mut s, FileAction::Delete);
        file(&mut s, FileAction::ConfirmDelete);
    }
    let ids: Vec<OpId> = s.ops.ops.iter().map(|o| o.id).collect();
    assert_eq!(s.toasts().len(), 2, "they stack");
    s.apply(ServiceEvent::FileOpFailed { id: ids[0], host: host("prod"), error: Error::PermissionDenied(p("/var/www/site/assets/x")) });
    let t = s.toasts();
    assert_eq!(t[0].text, "Couldn't delete ‘assets’: You don't have access to /var/www/site/assets/x");
    assert_eq!(t[1].text, "Deleting ‘uploads’…");
    file(&mut s, FileAction::DismissToast(t[0].id));
    assert_eq!(s.toasts().len(), 1);
}

#[test]
fn a_delete_on_the_fixture_removes_the_rows() {
    let mut s = demo();
    click(&mut s, "robots.txt");
    file(&mut s, FileAction::Delete);
    file(&mut s, FileAction::ConfirmDelete);
    assert!(!names(&s).contains(&"robots.txt".to_owned()));
    assert!(s.toasts().is_empty(), "done at once");
}

#[test]
fn copy_and_cut_fill_the_clip_and_the_os_clipboard() {
    let mut s = demo();
    assert!(file(&mut s, FileAction::Copy).is_empty(), "nothing selected, nothing copied");
    click(&mut s, "index.html");
    also(&mut s, "assets");
    let fx = file(&mut s, FileAction::Copy);
    assert_eq!(fx, [Effect::Copy("/var/www/site/assets\n/var/www/site/index.html".into())]);
    let clip = s.clip().clone().unwrap();
    assert_eq!((clip.host, clip.cut, clip.items.len()), (host("prod"), false, 2));
    file(&mut s, FileAction::Cut);
    assert!(s.clip().as_ref().unwrap().cut);
}

#[test]
fn copy_path_writes_the_selection_or_the_folder_on_screen() {
    let mut s = demo();
    assert_eq!(file(&mut s, FileAction::CopyPath), [Effect::Copy(SITE.into())]);
    click(&mut s, "index.html");
    assert_eq!(file(&mut s, FileAction::CopyPath), [Effect::Copy("/var/www/site/index.html".into())]);
    assert_eq!(*s.clip(), None, "copy path isn't a copy");
}

#[test]
fn paste_on_the_same_host_copies_or_moves_and_a_cut_empties_the_clip() {
    let (mut s, rec) = recorded();
    click(&mut s, "index.html");
    let text = "/var/www/site/index.html".to_owned();
    file(&mut s, FileAction::Copy);
    file(&mut s, FileAction::Paste { text: text.clone(), as_move: false });
    let index = item("/var/www/site/index.html", EntryKind::File, 14_812);
    assert_eq!(started(&rec), [JobRequest::RemoteCopy { host: host("prod"), items: vec![index.clone()], into: p(SITE) }]);
    assert!(s.clip().is_some(), "a copy pastes again");

    file(&mut s, FileAction::Cut);
    file(&mut s, FileAction::Paste { text: text.clone(), as_move: false });
    assert_eq!(started(&rec)[1], JobRequest::Move { host: host("prod"), items: vec![index.clone()], into: p(SITE) });
    assert_eq!(*s.clip(), None, "a pasted cut is gone, as in Finder");

    // ⌥⌘V moves a copied clip.
    file(&mut s, FileAction::Copy);
    file(&mut s, FileAction::Paste { text, as_move: true });
    assert_eq!(started(&rec)[2], JobRequest::Move { host: host("prod"), items: vec![index], into: p(SITE) });
}

#[test]
fn paste_on_another_host_streams_across() {
    let (mut s, rec) = recorded();
    click(&mut s, "index.html");
    file(&mut s, FileAction::Copy);
    // nas is still connected from before: no new connection.
    s.act(Action::SelectHost(host("nas")));
    assert_eq!(s.conn().unwrap().cwd, p("/volume1"));
    file(&mut s, FileAction::Paste { text: "/var/www/site/index.html".into(), as_move: false });
    let index = item("/var/www/site/index.html", EntryKind::File, 14_812);
    assert_eq!(started(&rec), [JobRequest::CrossHostCopy { from: host("prod"), items: vec![index.clone()], to: host("nas"), into: p("/volume1") }]);
    s.act(Action::SelectHost(host("prod")));
    file(&mut s, FileAction::Cut);
    s.act(Action::SelectHost(host("nas")));
    file(&mut s, FileAction::Paste { text: "/var/www/site/index.html".into(), as_move: false });
    assert_eq!(started(&rec)[1], JobRequest::CrossHostMove { from: host("prod"), items: vec![index], to: host("nas"), into: p("/volume1") });
}

#[test]
fn a_foreign_clipboard_text_pastes_nothing() {
    let (mut s, rec) = recorded();
    click(&mut s, "index.html");
    file(&mut s, FileAction::Copy);
    file(&mut s, FileAction::Paste { text: "something copied in another app".into(), as_move: false });
    file(&mut s, FileAction::Paste { text: String::new(), as_move: true });
    assert!(started(&rec).is_empty());
    // ⌥⌘V and the menus read the clipboard first, only with a clip.
    assert_eq!(file(&mut s, FileAction::RequestPaste { as_move: true }), [Effect::ReadClipboard { as_move: true }]);
    s.set_clip(None);
    assert!(file(&mut s, FileAction::RequestPaste { as_move: true }).is_empty());
}

#[test]
fn nothing_changes_the_server_while_reconnecting() {
    let (mut s, rec) = recorded();
    click(&mut s, "index.html");
    file(&mut s, FileAction::Copy);
    s.apply(ServiceEvent::Status { host: host("prod"), status: ConnStatus::Reconnecting { attempt: 1 } });
    file(&mut s, FileAction::NewFolder);
    file(&mut s, FileAction::Rename);
    file(&mut s, FileAction::Delete);
    file(&mut s, FileAction::Paste { text: "/var/www/site/index.html".into(), as_move: false });
    file(&mut s, FileAction::DragStart("index.html".into()));
    assert!(s.files.edit.field().is_none() && !s.ui.sheets.is_open() && s.files.drag.is_none());
    assert!(rec.sent().is_empty());
    // Copying and copy path still work.
    assert_eq!(file(&mut s, FileAction::CopyPath), [Effect::Copy("/var/www/site/index.html".into())]);
    assert_eq!(file(&mut s, FileAction::Copy).len(), 1);
}

#[test]
fn a_field_open_when_the_connection_drops_closes_and_a_confirmed_delete_waits_for_nothing() {
    let (mut s, rec) = recorded();
    file(&mut s, FileAction::NewFolder);
    click(&mut s, "index.html");
    file(&mut s, FileAction::Delete);
    s.apply(ServiceEvent::Status { host: host("prod"), status: ConnStatus::Reconnecting { attempt: 1 } });
    assert!(s.files.edit.field().is_none());
    file(&mut s, FileAction::ConfirmDelete);
    assert!(file_ops(&rec).is_empty(), "the host can't take it now");
}

#[test]
fn dragging_an_unselected_row_selects_it_and_folders_take_it() {
    let (mut s, rec) = recorded();
    click(&mut s, "README.md");
    file(&mut s, FileAction::DragStart("index.html".into()));
    assert_eq!(selected(&s), ["index.html"]);
    file(&mut s, FileAction::DragOver { target: Some(DragTarget::Folder("uploads".into())), copy: false });
    assert_eq!(s.files.drag.as_ref().unwrap().pill().as_deref(), Some("Move 1 item to uploads"));
    file(&mut s, FileAction::DragDrop);
    let index = item("/var/www/site/index.html", EntryKind::File, 14_812);
    assert_eq!(started(&rec), [JobRequest::Move { host: host("prod"), items: vec![index.clone()], into: p("/var/www/site/uploads") }]);
    assert_eq!(s.files.drag, None);

    // A dragged selected row takes the whole selection; ⌥ copies.
    click(&mut s, "index.html");
    also(&mut s, "robots.txt");
    file(&mut s, FileAction::DragStart("robots.txt".into()));
    assert_eq!(s.files.drag.as_ref().unwrap().names, ["index.html", "robots.txt"]);
    file(&mut s, FileAction::DragOver { target: Some(DragTarget::Crumb(p("/var/www"))), copy: true });
    assert_eq!(s.files.drag.as_ref().unwrap().pill().as_deref(), Some("Copy 2 items to www"));
    file(&mut s, FileAction::DragDrop);
    assert!(matches!(&started(&rec)[1], JobRequest::RemoteCopy { into, items, .. } if *into == p("/var/www") && items.len() == 2));
}

#[test]
fn a_drag_refuses_itself_the_folder_on_screen_files_and_hosts_never_opened() {
    let (mut s, rec) = recorded();
    click(&mut s, "assets");
    file(&mut s, FileAction::DragStart("assets".into()));
    for target in [
        DragTarget::Folder("assets".into()),
        DragTarget::Folder("index.html".into()),
        DragTarget::Crumb(p(SITE)),
        DragTarget::Host(host("prod")),
        DragTarget::Host(host("staging")),
    ] {
        file(&mut s, FileAction::DragOver { target: Some(target.clone()), copy: false });
        assert_eq!(s.files.drag.as_ref().unwrap().over, None, "{target:?}");
    }
    file(&mut s, FileAction::DragDrop);
    assert!(started(&rec).is_empty());
}

#[test]
fn a_drag_onto_another_host_copies_into_its_folder_on_screen() {
    let (mut s, rec) = recorded();
    click(&mut s, "assets");
    file(&mut s, FileAction::DragStart("assets".into()));
    file(&mut s, FileAction::DragOver { target: Some(DragTarget::Host(host("nas"))), copy: false });
    assert_eq!(s.files.drag.as_ref().unwrap().pill().as_deref(), Some("Copy 1 item to nas"));
    file(&mut s, FileAction::DragDrop);
    let assets = item("/var/www/site/assets", EntryKind::Dir, 4096);
    assert_eq!(started(&rec), [JobRequest::CrossHostCopy { from: host("prod"), items: vec![assets], to: host("nas"), into: p("/volume1") }]);
    // Cancelled, nothing.
    file(&mut s, FileAction::DragStart("assets".into()));
    file(&mut s, FileAction::DragOver { target: Some(DragTarget::Folder("uploads".into())), copy: false });
    file(&mut s, FileAction::DragCancel);
    assert_eq!(started(&rec).len(), 1);
}

#[test]
fn the_list_keys_leave_a_focused_field_alone() {
    let (mut s, _rec) = recorded();
    click(&mut s, "index.html");
    s.act(Action::OpenFilter);
    assert!(s.text_focused());
    assert!(s.shortcut(crate::state::Shortcut::File(crate::state::keys::FileKey::Copy), 0.0).is_empty());
    assert_eq!(*s.clip(), None);
    s.ui.filter_edit.focused = false;
    assert_eq!(s.shortcut(crate::state::Shortcut::File(crate::state::keys::FileKey::Copy), 0.0).len(), 1);
}

#[test]
fn the_delete_sheet_lists_five_names_then_how_many_more() {
    let names: Vec<String> = (1..=17).map(|i| format!("f{i}")).collect();
    assert_eq!(delete_title(&names), "Delete 17 items permanently?");
    assert_eq!(delete_title(&names[..1]), "Delete ‘f1’ permanently?");
    assert_eq!(delete_more(&names).as_deref(), Some("and 12 more"));
    assert_eq!(delete_more(&names[..5]), None);
}
