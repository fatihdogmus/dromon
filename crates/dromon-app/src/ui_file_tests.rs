//! File operations driven headless: the new-folder and rename field, the delete sheet and its
//! toast, the clipboard (with Rune's in-memory OS clipboard), row drags and context menus. The
//! real view, handlers and `on_key`, on the demo fixture (which carries operations out at once)
//! or on a [`Recorder`] to read the commands back.

use dromon_core::{ConnStatus, EntryKind, Error, HostId, RemotePath};
use dromon_transfer::{JobRequest, RemoteItem};
use rune::clipboard::MemoryClipboard;
use rune::{ClipboardContent, Env, Key, KeyEvent, KeyPlatform, Mods, PhysicalInput, Runtime};

use crate::state::{Action, App, Command, FileOp, Recorder, ServiceEvent, TransferCommand};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);
const SITE: &str = "/var/www/site";

fn host(s: &str) -> HostId {
    HostId(s.into())
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// The demo with nas opened, then prod on screen at its site, on the fixture's backend; the
/// OS clipboard is the returned in-memory one.
fn runtime() -> (Runtime<App>, MemoryClipboard) {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    let board = rune::clipboard::memory();
    rt.use_clipboard(&board);
    rt.update(|s, _| {
        *s = scenes::base();
        s.act(Action::SelectHost(host("nas")));
        s.act(Action::SelectHost(host("prod")));
    });
    settle(&mut rt);
    (rt, board)
}

/// [`runtime`], with later commands going to the returned recorder.
fn recorded() -> (Runtime<App>, MemoryClipboard, Recorder) {
    let (mut rt, board) = runtime();
    let rec = Recorder::default();
    let backend = rec.clone();
    rt.update(move |s, _| s.backend = Box::new(backend));
    (rt, board, rec)
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn at(rt: &Runtime<App>, key: &str) -> (f32, f32) {
    rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"))
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = at(rt, key);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

fn type_text(rt: &mut Runtime<App>, text: &str) {
    press(rt, Key::Text(text.into()), Mods::default());
}

/// ⌘ on macOS, Ctrl elsewhere.
fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

fn with(m: Mods, f: impl FnOnce(&mut Mods)) -> Mods {
    let mut m = m;
    f(&mut m);
    m
}

/// Delete's key: ⌘⌫ on macOS, Delete elsewhere.
fn delete_key(rt: &mut Runtime<App>) {
    if KeyPlatform::current() == KeyPlatform::Mac {
        press(rt, Key::Backspace, primary());
    } else {
        press(rt, Key::Delete, Mods::default());
    }
}

fn shown(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

fn text_of(rt: &Runtime<App>, k: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).and_then(|e| e.text).unwrap_or_default()
}

fn focused(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && e.focused)
}

fn selected(rt: &Runtime<App>) -> Vec<String> {
    let c = rt.state.conn().unwrap();
    c.row_names().into_iter().filter(|n| c.selection.contains(n)).map(str::to_owned).collect()
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

fn index_html() -> RemoteItem {
    item("/var/www/site/index.html", EntryKind::File, 14_812)
}

/// Presses on `from`, drags to `to` in steps, and lets go there.
fn drag(rt: &mut Runtime<App>, from: (f32, f32), to: (f32, f32), before_release: impl FnOnce(&mut Runtime<App>)) {
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    rt.frame(0.02);
    for i in 1..=4 {
        let k = i as f32 / 4.0;
        rt.pointer_move(from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k);
        rt.frame(0.02);
    }
    settle(rt);
    before_release(rt);
    rt.pointer_up(to.0, to.1);
    settle(rt);
}

#[test]
fn new_folder_opens_a_focused_field_and_return_makes_it() {
    let (mut rt, _) = runtime();
    press(&mut rt, Key::Command('n'), with(primary(), |m| m.shift = true));
    assert!(shown(&rt, "new-folder"));
    assert!(focused(&rt, "edit-input"), "the field has the keyboard");
    assert_eq!(text_of(&rt, "edit-input"), "New folder");
    // The prefilled name is selected: typing replaces it.
    type_text(&mut rt, "logs");
    assert_eq!(text_of(&rt, "edit-input"), "logs");
    press(&mut rt, Key::Enter, Mods::default());
    assert!(!shown(&rt, "new-folder") && !shown(&rt, "edit-input"));
    assert!(rt.find("rows/logs").is_some(), "the folder lists");
    assert_eq!(selected(&rt), ["logs"]);
    assert_eq!(rt.state.files.scroll, None, "scrolled to it, once");
}

#[test]
fn new_folder_with_a_taken_name_keeps_the_field_with_an_error() {
    let (mut rt, _) = runtime();
    press(&mut rt, Key::Command('n'), with(primary(), |m| m.shift = true));
    type_text(&mut rt, "assets");
    press(&mut rt, Key::Enter, Mods::default());
    assert!(shown(&rt, "edit-input"), "the field stays");
    assert_eq!(text_of(&rt, "edit-error-text"), "‘assets’ already exists here");
    assert!(focused(&rt, "edit-input"), "still focused after the shake");
    // Fixing the name makes it.
    press(&mut rt, Key::Backspace, Mods::default());
    type_text(&mut rt, "-2");
    assert!(!shown(&rt, "edit-error"), "typing clears the error");
    press(&mut rt, Key::Enter, Mods::default());
    assert!(rt.find("rows/asset-2").is_some());
    // Escape on a new one makes nothing.
    press(&mut rt, Key::Command('n'), with(primary(), |m| m.shift = true));
    press(&mut rt, Key::Escape, Mods::default());
    assert!(!shown(&rt, "new-folder") && rt.find("rows/New folder").is_none());
}

#[test]
fn f2_renames_with_the_stem_selected() {
    let (mut rt, _) = runtime();
    click(&mut rt, "rows/README.md");
    press(&mut rt, Key::Function(2), Mods::default());
    assert!(focused(&rt, "edit-input"));
    let range = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("edit-input")).and_then(|e| e.edit_range);
    assert_eq!(range, Some([0, 6]), "README of README.md");
    type_text(&mut rt, "NOTES");
    press(&mut rt, Key::Enter, Mods::default());
    assert!(rt.find("rows/README.md").is_none() && rt.find("rows/NOTES.md").is_some());
    assert_eq!(selected(&rt), ["NOTES.md"], "the renamed item stays selected");
}

#[test]
fn rename_to_the_same_name_cancels_and_a_taken_name_errs() {
    let (mut rt, _, rec) = recorded();
    click(&mut rt, "rows/robots.txt");
    press(&mut rt, Key::Function(2), Mods::default());
    press(&mut rt, Key::Enter, Mods::default());
    assert!(!shown(&rt, "edit-input"));
    assert!(file_ops(&rec).is_empty(), "the same name sends nothing");

    let (mut rt, _) = runtime();
    click(&mut rt, "rows/package-lock.json");
    press(&mut rt, Key::Function(2), Mods::default());
    type_text(&mut rt, "package");
    press(&mut rt, Key::Enter, Mods::default());
    assert_eq!(text_of(&rt, "edit-error-text"), "‘package.json’ already exists here");
    assert!(rt.find("rows/package-lock.json").is_some());
}

#[test]
fn the_delete_sheet_doesnt_delete_on_return() {
    let (mut rt, _) = runtime();
    click(&mut rt, "rows/robots.txt");
    delete_key(&mut rt);
    assert!(shown(&rt, "delete"));
    assert_eq!(text_of(&rt, "sheet-title"), "Delete ‘robots.txt’ permanently?");
    assert_eq!(text_of(&rt, "delete-note"), "This can't be undone on the server.");
    press(&mut rt, Key::Enter, Mods::default());
    assert!(shown(&rt, "delete"), "Return isn't Delete");
    assert!(rt.find("rows/robots.txt").is_some());
    press(&mut rt, Key::Escape, Mods::default());
    assert!(!shown(&rt, "delete"));
    assert!(rt.find("rows/robots.txt").is_some());
    delete_key(&mut rt);
    click(&mut rt, "delete-confirm");
    assert!(!shown(&rt, "delete"));
    assert!(rt.find("rows/robots.txt").is_none());
}

#[test]
fn a_delete_shows_its_progress_toast_and_a_failure_stays_until_dismissed() {
    let (mut rt, _, rec) = recorded();
    click(&mut rt, "rows/assets");
    rt.physical_input(PhysicalInput::Modifiers(primary()));
    click(&mut rt, "rows/uploads");
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    delete_key(&mut rt);
    assert_eq!(text_of(&rt, "sheet-title"), "Delete 2 items permanently?");
    click(&mut rt, "delete-confirm");
    assert_eq!(file_ops(&rec), [FileOp::Delete { paths: vec![p("/var/www/site/assets"), p("/var/www/site/uploads")] }]);
    let id = rt.state.ops.ops.last().unwrap().id;
    let key = format!("toast-op-{}", id.0);
    assert_eq!(text_of(&rt, &format!("{key}-text")), "Deleting 2 items…");
    let ev = ServiceEvent::FileOpProgress { id, host: host("prod"), done: 1204, total: Some(5005) };
    rt.update(move |s, _| s.apply(ev));
    settle(&mut rt);
    assert_eq!(text_of(&rt, &format!("{key}-text")), "Deleting… 1,204 of 5,005");
    assert!(!shown(&rt, &format!("{key}-dismiss")), "a running delete can't be dismissed");
    let ev = ServiceEvent::FileOpFailed { id, host: host("prod"), error: Error::PermissionDenied(p("/var/www/site/uploads")) };
    rt.update(move |s, _| s.apply(ev));
    settle(&mut rt);
    assert_eq!(text_of(&rt, &format!("{key}-text")), "Couldn't delete 2 items: You don't have access to /var/www/site/uploads");
    click(&mut rt, &format!("{key}-dismiss"));
    assert!(!shown(&rt, &key));
}

#[test]
fn copy_then_paste_in_the_same_folder_copies_on_the_server() {
    let (mut rt, board, rec) = recorded();
    click(&mut rt, "rows/index.html");
    press(&mut rt, Key::Command('c'), primary());
    assert_eq!(board.text().as_deref().map(String::as_str), Some("/var/www/site/index.html"));
    press(&mut rt, Key::Command('v'), primary());
    assert_eq!(started(&rec), [JobRequest::RemoteCopy { host: host("prod"), items: vec![index_html()], into: p(SITE) }]);
    // ⌥⌘V moves it, and the clip is spent.
    press(&mut rt, Key::Command('v'), with(primary(), |m| m.alt = true));
    assert_eq!(started(&rec)[1], JobRequest::Move { host: host("prod"), items: vec![index_html()], into: p(SITE) });
    assert_eq!(*rt.state.clip(), None);
}

#[test]
fn on_the_fixture_a_paste_in_its_own_folder_makes_a_copy() {
    let (mut rt, _) = runtime();
    click(&mut rt, "rows/robots.txt");
    press(&mut rt, Key::Command('c'), primary());
    press(&mut rt, Key::Command('v'), primary());
    assert!(rt.find("rows/robots (1).txt").is_some());
}

#[test]
fn cut_rows_dim_and_paste_on_another_host_moves_across() {
    let (mut rt, _, rec) = recorded();
    click(&mut rt, "rows/index.html");
    press(&mut rt, Key::Command('x'), primary());
    let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("index.html") && e.role == "row").expect("the row");
    assert_eq!(row.opacity, 0.5, "the cut row is dimmed");
    assert_eq!(row.label.as_deref(), Some("index.html (cut)"), "and says so (§6.9)");
    click(&mut rt, "host-nas");
    press(&mut rt, Key::Command('v'), primary());
    assert_eq!(
        started(&rec),
        [JobRequest::CrossHostMove { from: host("prod"), items: vec![index_html()], to: host("nas"), into: p("/volume1") }]
    );
    assert_eq!(*rt.state.clip(), None);
}

#[test]
fn copy_on_one_host_pastes_as_a_cross_host_copy() {
    let (mut rt, _, rec) = recorded();
    click(&mut rt, "rows/index.html");
    press(&mut rt, Key::Command('c'), primary());
    click(&mut rt, "host-nas");
    press(&mut rt, Key::Command('v'), primary());
    assert_eq!(
        started(&rec),
        [JobRequest::CrossHostCopy { from: host("prod"), items: vec![index_html()], to: host("nas"), into: p("/volume1") }]
    );
}

#[test]
fn a_paste_with_something_else_on_the_os_clipboard_does_nothing() {
    let (mut rt, board, rec) = recorded();
    click(&mut rt, "rows/index.html");
    press(&mut rt, Key::Command('c'), primary());
    board.set_text("copied in another app");
    press(&mut rt, Key::Command('v'), primary());
    press(&mut rt, Key::Command('v'), with(primary(), |m| m.alt = true));
    assert!(started(&rec).is_empty());
    assert!(rt.state.clip().is_some(), "kept, in case it's copied again");
}

#[test]
fn finder_files_still_upload_with_a_clip_around() {
    let (mut rt, _, rec) = recorded();
    click(&mut rt, "rows/index.html");
    press(&mut rt, Key::Command('c'), primary());
    rt.paste_content(ClipboardContent::default().with_files(vec!["/Users/me/a.pdf".into()]));
    settle(&mut rt);
    assert_eq!(started(&rec), [JobRequest::Upload { host: host("prod"), items: vec!["/Users/me/a.pdf".into()], into: p(SITE) }]);
}

#[test]
fn copy_path_puts_the_paths_on_the_clipboard() {
    let (mut rt, board) = runtime();
    press(&mut rt, Key::Command('c'), with(primary(), |m| m.alt = true));
    assert_eq!(board.text().as_deref().map(String::as_str), Some(SITE), "nothing selected: the folder");
    click(&mut rt, "rows/robots.txt");
    rt.physical_input(PhysicalInput::Modifiers(with(Mods::default(), |m| m.shift = true)));
    click(&mut rt, "rows/package.json");
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    press(&mut rt, Key::Command('c'), with(primary(), |m| m.alt = true));
    // In the order on screen.
    let paths = "/var/www/site/package.json\n/var/www/site/README.md\n/var/www/site/robots.txt";
    assert_eq!(board.text().as_deref().map(String::as_str), Some(paths));
    assert_eq!(*rt.state.clip(), None);
}

#[test]
fn dragging_rows_onto_a_folder_moves_them_and_the_pill_says_so() {
    let (mut rt, _, rec) = recorded();
    let from = at(&rt, "rows/index.html");
    let to = at(&rt, "rows/uploads");
    drag(&mut rt, from, to, |rt| {
        assert_eq!(text_of(rt, "drop-pill-text"), "Move 1 item to uploads");
    });
    assert_eq!(started(&rec), [JobRequest::Move { host: host("prod"), items: vec![index_html()], into: p("/var/www/site/uploads") }]);
    assert_eq!(selected(&rt), ["index.html"], "the dragged row was selected first");
    assert!(!shown(&rt, "drop-pill"));
}

#[test]
fn a_held_copy_key_copies_and_a_breadcrumb_takes_the_rows() {
    let (mut rt, _, rec) = recorded();
    let from = at(&rt, "rows/index.html");
    let to = at(&rt, "crumb-2");
    let copy = if KeyPlatform::current() == KeyPlatform::Mac { Mods { alt: true, ..Mods::default() } } else { primary() };
    rt.physical_input(PhysicalInput::Modifiers(copy));
    drag(&mut rt, from, to, |rt| assert_eq!(text_of(rt, "drop-pill-text"), "Copy 1 item to www"));
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    assert_eq!(started(&rec), [JobRequest::RemoteCopy { host: host("prod"), items: vec![index_html()], into: p("/var/www") }]);
}

#[test]
fn a_sidebar_host_takes_a_copy_into_its_folder_on_screen() {
    let (mut rt, _, rec) = recorded();
    let from = at(&rt, "rows/index.html");
    let to = at(&rt, "host-nas");
    drag(&mut rt, from, to, |rt| assert_eq!(text_of(rt, "drop-pill-text"), "Copy 1 item to nas"));
    assert_eq!(
        started(&rec),
        [JobRequest::CrossHostCopy { from: host("prod"), items: vec![index_html()], to: host("nas"), into: p("/volume1") }]
    );
    // A host never opened takes nothing.
    let to = at(&rt, "host-staging");
    drag(&mut rt, from, to, |rt| assert!(!shown(rt, "drop-pill")));
    assert_eq!(started(&rec).len(), 1);
}

#[test]
fn a_folder_dropped_on_itself_or_the_folder_on_screen_goes_nowhere_and_escape_cancels() {
    let (mut rt, _, rec) = recorded();
    let assets = at(&rt, "rows/assets");
    drag(&mut rt, assets, (assets.0 + 60.0, assets.1 + 2.0), |rt| assert!(!shown(rt, "drop-pill")));
    let here = at(&rt, "crumb-3");
    drag(&mut rt, assets, here, |rt| assert!(!shown(rt, "drop-pill")));
    let uploads = at(&rt, "rows/uploads");
    drag(&mut rt, assets, uploads, |rt| {
        assert!(shown(rt, "drop-pill"));
        press(rt, Key::Escape, Mods::default());
        assert!(!shown(rt, "drop-pill"));
    });
    assert!(started(&rec).is_empty());
    assert_eq!(rt.state.files.drag, None);
}

#[test]
fn double_clicking_a_file_row_downloads_it() {
    let (mut rt, _, rec) = recorded();
    let (x, y) = at(&rt, "rows/index.html");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.05);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(
        matches!(&started(&rec)[..], [JobRequest::Download { host: h, items, .. }] if *h == host("prod") && *items == [index_html()]),
        "one download of index.html: {:?}",
        started(&rec)
    );
    assert_eq!(rt.state.conn().unwrap().cwd.as_str(), SITE, "a download doesn't navigate");
}

#[test]
fn nothing_changes_the_server_while_reconnecting() {
    let (mut rt, board, rec) = recorded();
    click(&mut rt, "rows/index.html");
    press(&mut rt, Key::Command('c'), primary());
    rt.update(|s, _| s.apply(ServiceEvent::Status { host: host("prod"), status: ConnStatus::Reconnecting { attempt: 1 } }));
    settle(&mut rt);
    press(&mut rt, Key::Command('n'), with(primary(), |m| m.shift = true));
    press(&mut rt, Key::Function(2), Mods::default());
    delete_key(&mut rt);
    press(&mut rt, Key::Command('v'), primary());
    press(&mut rt, Key::Command('v'), with(primary(), |m| m.alt = true));
    let from = at(&rt, "rows/index.html");
    let to = at(&rt, "rows/uploads");
    drag(&mut rt, from, to, |_| {});
    assert!(!shown(&rt, "edit-input") && !shown(&rt, "delete"));
    assert!(rec.sent().is_empty(), "nothing sent: {:?}", rec.sent());
    // Copy path still works.
    press(&mut rt, Key::Command('c'), with(primary(), |m| m.alt = true));
    assert_eq!(board.text().as_deref().map(String::as_str), Some("/var/www/site/index.html"));
}
