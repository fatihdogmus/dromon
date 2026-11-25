//! Transfers driven headless: ⌘D, the toolbar button and its popover, the row buttons, files
//! dropped on the list or a folder row, ⌘V with files, and the conflict sheet. The real view
//! and handlers, on the demo fixture (whose engine finishes transfers at once) or on a
//! [`Recorder`] to read the commands back.

use std::path::PathBuf;

use dromon_core::{HostId, Prompt, PromptId, PromptKind, RemotePath};
use dromon_transfer::{ConflictView, JobId, JobKind, JobRequest, JobView, Progress, TaskState};
use rune::{ClipboardContent, Env, Key, KeyEvent, Mods, Runtime};

use crate::state::{Action, App, Command, DropTarget, Recorder, ServiceEvent, Tab, TransferCommand};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);
const SITE: &str = "/var/www/site";

fn prod() -> HostId {
    HostId("prod".into())
}

/// The demo, connected to prod at its site, on the fixture's own backend.
fn runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| {
        *s = scenes::base();
        s.act(Action::SelectHost(prod()));
    });
    settle(&mut rt);
    rt
}

/// [`runtime`], with later commands going to the returned recorder instead.
fn recorded() -> (Runtime<App>, Recorder) {
    let mut rt = runtime();
    let rec = Recorder::default();
    let backend = rec.clone();
    rt.update(move |s, _| s.backend = Box::new(backend));
    (rt, rec)
}

/// Lets springs and exits finish.
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

fn key(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

/// ⌘ on macOS, Ctrl elsewhere, as the platform reports the primary modifier.
fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

/// Whether `k` is on screen (not on its way out).
fn shown(rt: &Runtime<App>, k: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(k) && !e.exiting)
}

fn text_of(rt: &Runtime<App>, k: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).and_then(|e| e.text).unwrap_or_default()
}

fn apply(rt: &mut Runtime<App>, ev: ServiceEvent) {
    rt.update(move |s, _| s.apply(ev));
    settle(rt);
}

/// The transfer commands sent so far.
fn transfers(rec: &Recorder) -> Vec<TransferCommand> {
    rec.sent().into_iter().filter_map(|c| if let Command::Transfer(t) = c { Some(t) } else { None }).collect()
}

/// The uploads started so far: their items and folder.
fn uploads(rec: &Recorder) -> Vec<(Vec<PathBuf>, String)> {
    transfers(rec)
        .into_iter()
        .filter_map(|t| match t {
            TransferCommand::Start(JobRequest::Upload { items, into, .. }) => Some((items, into.to_string())),
            _ => None,
        })
        .collect()
}

fn job(id: u64, kind: JobKind, state: TaskState, done: u64, total: u64) -> JobView {
    JobView {
        id: JobId(id),
        kind,
        host: prod(),
        title: format!("job {id}"),
        state,
        progress: Progress { done, total },
        files_done: 0,
        files_total: 1,
        speed: None,
        eta: None,
        waiting_for_connection: false,
        conflict: None,
        reveal: None,
        errors: Vec::new(),
        skipped: Vec::new(),
        folders: Vec::new(),
    }
}

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(|n| PathBuf::from(format!("/Users/me/{n}"))).collect()
}

/// Somewhere over the list but not over a row: below the last one.
fn empty_list_spot(rt: &Runtime<App>) -> (f32, f32) {
    let (x, y) = rt.find("rows/server.log").expect("the last row");
    (x, y + 80.0)
}

#[test]
fn cmd_d_downloads_the_selection_and_the_ring_follows_the_engine() {
    // On the fixture: the download finishes at once, so the glyph is plain again.
    let mut rt = runtime();
    click(&mut rt, "rows/robots.txt");
    assert!(!shown(&rt, "tb-ring"));
    key(&mut rt, Key::Command('d'), primary());
    let jobs = &rt.state.transfers.jobs;
    assert_eq!(jobs.iter().map(|j| (j.title.as_str(), &j.state)).collect::<Vec<_>>(), [("robots.txt", &TaskState::Done)]);
    assert!(!shown(&rt, "tb-ring") && !shown(&rt, "tb-badge"), "nothing active");

    // While the engine reports it running: the ring and a badge counting it.
    let (mut rt, rec) = recorded();
    click(&mut rt, "rows/README.md");
    key(&mut rt, Key::Command('d'), primary());
    let started = transfers(&rec);
    assert!(matches!(&started[..], [TransferCommand::Start(JobRequest::Download { items, .. })] if items.len() == 1));
    apply(&mut rt, ServiceEvent::Transfers(vec![job(1, JobKind::Download, TaskState::Running, 400, 1000)]));
    assert!(shown(&rt, "tb-ring"));
    assert_eq!(text_of(&rt, "tb-badge-count"), "1");
    apply(&mut rt, ServiceEvent::Transfers(vec![job(1, JobKind::Download, TaskState::Done, 1000, 1000)]));
    assert!(!shown(&rt, "tb-ring") && !shown(&rt, "tb-badge"));
}

#[test]
fn the_button_opens_the_popover_its_tabs_switch_and_a_click_outside_closes_it() {
    let mut rt = runtime();
    assert!(!shown(&rt, "transfers"));
    click(&mut rt, "tb-transfers");
    assert!(rt.state.transfers.open && shown(&rt, "transfers"));
    assert!(shown(&rt, "tr-empty"));
    assert_eq!(text_of(&rt, "tr-summary"), "Nothing running");
    click(&mut rt, "tr-tab-uploads");
    assert_eq!(rt.state.transfers.tab, Tab::Uploads);
    click(&mut rt, "tr-tab-downloads");
    assert_eq!(rt.state.transfers.tab, Tab::Downloads);

    // The button again (outside the popover) closes it; so does Escape.
    click(&mut rt, "tb-transfers");
    assert!(!rt.state.transfers.open && !shown(&rt, "transfers"));
    click(&mut rt, "tb-transfers");
    assert!(shown(&rt, "transfers"));
    key(&mut rt, Key::Escape, Mods::default());
    assert!(!shown(&rt, "transfers"));
}

#[test]
fn row_buttons_send_their_commands_and_finished_rows_clear() {
    let (mut rt, rec) = recorded();
    let mut failed = job(6, JobKind::Download, TaskState::Failed { error: "1 file failed".into() }, 0, 10);
    failed.errors = vec!["a: denied".into()];
    apply(
        &mut rt,
        ServiceEvent::Transfers(vec![
            failed,
            job(5, JobKind::Download, TaskState::Running, 3, 10),
            job(4, JobKind::Download, TaskState::Paused, 3, 10),
            job(3, JobKind::Download, TaskState::Done, 10, 10),
        ]),
    );
    click(&mut rt, "tb-transfers");
    assert_eq!(text_of(&rt, "tr-line-6"), "Failed: 1 file failed");
    assert_eq!(text_of(&rt, "tr-line-4"), "Paused");
    click(&mut rt, "tr-pause-5");
    click(&mut rt, "tr-retry-6");
    click(&mut rt, "tr-resume-4");
    click(&mut rt, "tr-cancel-5");
    click(&mut rt, "tr-clear");
    assert_eq!(
        transfers(&rec),
        [
            TransferCommand::Pause(JobId(5)),
            TransferCommand::Retry(JobId(6)),
            TransferCommand::Resume(JobId(4)),
            TransferCommand::Cancel(JobId(5)),
            TransferCommand::ClearFinished,
        ]
    );
    assert!(!shown(&rt, "tr-pause-3") && !shown(&rt, "tr-retry-3"), "a done row has no buttons");
}

#[test]
fn files_dropped_on_the_list_or_a_folder_row_upload_there() {
    let (mut rt, rec) = recorded();
    let (row_x, row_y) = rt.find("rows/uploads").unwrap();
    let spot = empty_list_spot(&rt);

    // The drag shows where the files would go, and follows it.
    rt.drag_files(paths(&["a.txt", "b.txt", "c.txt"]), Some(spot));
    settle(&mut rt);
    assert_eq!(rt.state.transfers.drop.as_ref().map(|d| (&d.target, d.count)), Some((&DropTarget::List, 3)));
    assert!(shown(&rt, "drop-outline"));
    assert_eq!(text_of(&rt, "drop-pill-text"), "Upload 3 items to /var/www/site");
    rt.drag_files_to(Some((row_x, row_y)));
    settle(&mut rt);
    assert_eq!(rt.state.transfers.drop.as_ref().map(|d| &d.target), Some(&DropTarget::Folder("uploads".into())));
    assert!(!shown(&rt, "drop-outline"), "a folder row lifts instead");
    assert_eq!(text_of(&rt, "drop-pill-text"), "Upload 3 items to uploads");
    // Over a file row, the list takes it.
    let (fx, fy) = rt.find("rows/index.html").unwrap();
    rt.drag_files_to(Some((fx, fy)));
    settle(&mut rt);
    assert_eq!(rt.state.transfers.drop.as_ref().map(|d| &d.target), Some(&DropTarget::List));
    rt.drag_files_leave();
    settle(&mut rt);
    assert_eq!(rt.state.transfers.drop, None);
    assert!(!shown(&rt, "drop-pill"));

    rt.drag_files(paths(&["a.txt", "b.txt"]), Some(spot));
    rt.drop_files(paths(&["a.txt", "b.txt"]), Some(spot));
    settle(&mut rt);
    rt.drag_files(paths(&["c.png"]), Some((row_x, row_y)));
    rt.drop_files(paths(&["c.png"]), Some((row_x, row_y)));
    settle(&mut rt);
    assert_eq!(uploads(&rec), [(paths(&["a.txt", "b.txt"]), SITE.to_owned()), (paths(&["c.png"]), format!("{SITE}/uploads"))]);
    assert_eq!(rt.state.transfers.drop, None);
}

#[test]
fn nothing_takes_a_drop_before_a_host_is_on_screen() {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| *s = scenes::base());
    settle(&mut rt);
    rt.drag_files(paths(&["a.txt"]), Some((600.0, 300.0)));
    settle(&mut rt);
    assert_eq!(rt.drop_hovered(), None);
    assert_eq!(rt.state.transfers.drop, None);
}

#[test]
fn pasting_files_copied_in_the_file_manager_uploads_them() {
    let (mut rt, rec) = recorded();
    rt.paste_content(ClipboardContent::default().with_files(paths(&["x.pdf", "y.pdf"])));
    settle(&mut rt);
    rt.paste_content(ClipboardContent::new("just text"));
    settle(&mut rt);
    assert_eq!(uploads(&rec), [(paths(&["x.pdf", "y.pdf"]), SITE.to_owned())]);
}

#[test]
fn the_conflict_sheet_answers_each_clash_and_keep_both_adds_a_copy() {
    let mut rt = runtime();
    let spot = empty_list_spot(&rt);
    rt.drag_files(paths(&["index.html", "robots.txt", "notes.txt"]), Some(spot));
    rt.drop_files(paths(&["index.html", "robots.txt", "notes.txt"]), Some(spot));
    settle(&mut rt);
    assert!(shown(&rt, "conflict"));
    assert_eq!(text_of(&rt, "sheet-title"), "‘index.html’ already exists in /var/www/site");
    assert!(shown(&rt, "conflict-apply-all"), "two clashes: it offers to apply to both");

    click(&mut rt, "conflict-keep-both");
    assert_eq!(text_of(&rt, "sheet-title"), "‘robots.txt’ already exists in /var/www/site", "the next clash");
    assert!(!shown(&rt, "conflict-apply-all"), "the last one");
    click(&mut rt, "conflict-skip");
    assert!(!shown(&rt, "conflict"));
    let j = &rt.state.transfers.jobs[0];
    assert_eq!((&j.state, j.files_total, j.skipped.clone()), (&TaskState::Done, 2, vec!["robots.txt".to_owned()]));
    // The folder lists again with the copy.
    assert!(rt.find("rows/index (1).html").is_some());
    assert!(rt.find("rows/notes.txt").is_some());
    assert!(rt.find("rows/robots (1).txt").is_none());
}

#[test]
fn apply_to_all_answers_every_clash_at_once() {
    let mut rt = runtime();
    let spot = empty_list_spot(&rt);
    let files = paths(&["index.html", "robots.txt", "favicon.ico"]);
    rt.drag_files(files.clone(), Some(spot));
    rt.drop_files(files, Some(spot));
    settle(&mut rt);
    assert!(rt.elements().into_iter().any(|e| e.label.as_deref() == Some("Apply to all 3 conflicts")));
    click(&mut rt, "conflict-apply-all");
    assert!(rt.state.transfers.apply_all);
    // Return presses Keep both, the default.
    key(&mut rt, Key::Enter, Mods::default());
    assert!(!shown(&rt, "conflict"));
    assert!(!rt.state.transfers.apply_all, "the next clash starts unticked");
    for name in ["index (1).html", "robots (1).txt", "favicon (1).ico"] {
        assert!(rt.find(&format!("rows/{name}")).is_some(), "{name}");
    }
}

#[test]
fn a_prompt_sheet_comes_before_the_conflict_sheet() {
    let (mut rt, _rec) = recorded();
    let mut j = job(1, JobKind::Upload, TaskState::Preparing { files_found: 0 }, 0, 0);
    j.conflict = Some(ConflictView { name: "index.html".into(), folder: RemotePath::new(SITE), existing_is_dir: false, more: 0 });
    let prompt = Prompt { id: PromptId(1), host: prod(), kind: PromptKind::Password, text: "pw: ".into(), error: None };
    apply(&mut rt, ServiceEvent::Prompt(prompt));
    apply(&mut rt, ServiceEvent::Transfers(vec![j]));
    assert!(shown(&rt, "sheet") && !shown(&rt, "conflict"));
    assert_eq!(text_of(&rt, "sheet-title"), "Sign in to prod");
    apply(&mut rt, ServiceEvent::PromptClosed(PromptId(1)));
    assert!(shown(&rt, "conflict"));
    // Its keys are its own: the list's shortcuts don't reach behind it.
    let before = rt.state.conn().unwrap().selection.cursor().map(str::to_owned);
    key(&mut rt, Key::Down, Mods::default());
    assert_eq!(rt.state.conn().unwrap().selection.cursor().map(str::to_owned), before);
}

#[test]
fn a_clash_waits_in_the_open_popover_until_resolve_brings_its_sheet() {
    let (mut rt, rec) = recorded();
    click(&mut rt, "tb-transfers");
    let mut j = job(7, JobKind::Upload, TaskState::Preparing { files_found: 0 }, 0, 0);
    j.conflict = Some(ConflictView { name: "index.html".into(), folder: RemotePath::new(SITE), existing_is_dir: false, more: 0 });
    apply(&mut rt, ServiceEvent::Transfers(vec![j]));
    assert!(shown(&rt, "transfers") && !shown(&rt, "conflict"));
    assert_eq!(rt.state.transfers.tab, Tab::Downloads, "the open popover keeps its tab");
    click(&mut rt, "tr-tab-uploads");
    assert_eq!(text_of(&rt, "tr-line-7"), "Waiting for you: ‘index.html’ already exists");
    click(&mut rt, "tr-resolve-7");
    assert!(!shown(&rt, "transfers") && shown(&rt, "conflict"));
    click(&mut rt, "conflict-replace");
    assert_eq!(
        transfers(&rec).last(),
        Some(&TransferCommand::Resolve { job: JobId(7), choice: dromon_transfer::ConflictChoice::Replace, apply_all: false })
    );
}
