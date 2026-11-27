//! Transfers driven headless: ⌘D, the toolbar button and its popover, the row buttons, files
//! dropped on the list or a folder row, ⌘V with files, and the conflict sheet. The real view
//! and handlers, on the demo fixture (whose engine finishes transfers at once) or on a
//! [`Recorder`] to read the commands back.

use std::path::PathBuf;

use dromon_core::{HostId, Prompt, PromptId, PromptKind, RemotePath};
use dromon_transfer::{ConflictView, JobId, JobKind, JobRequest, JobView, Progress, Side, TaskState};
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
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(k) && !e.exiting)
        .and_then(|e| e.text)
        .unwrap_or_default()
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

/// A clash with index.html in the site folder.
fn clash(existing: Side, incoming: Side) -> ConflictView {
    ConflictView {
        name: "index.html".into(),
        folder: RemotePath::new(SITE),
        existing_is_dir: false,
        existing,
        incoming,
        more: 0,
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
    assert_eq!(
        jobs.iter().map(|j| (j.title.as_str(), &j.state)).collect::<Vec<_>>(),
        [("robots.txt", &TaskState::Done)]
    );
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

/// `k` on screen (not on its way out).
fn get(rt: &Runtime<App>, k: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(k) && !e.exiting)
        .unwrap_or_else(|| panic!("no element {k:?}"))
}

/// Whether a `tb-pulse-*` element is on screen.
fn pulsing(rt: &Runtime<App>) -> bool {
    rt.elements()
        .into_iter()
        .any(|e| e.key.as_deref().is_some_and(|k| k.starts_with("tb-pulse-")) && !e.exiting && e.opacity > 0.01)
}

/// The popover open over a download at 40%, then the engine says it's done (as the service
/// would: a change no input caused). `forced`: the app forces reduce motion; `system`: the OS
/// asks for it.
fn finishing(forced: bool, system: bool) -> Runtime<App> {
    let (mut rt, _rec) = recorded();
    rt.set_system_reduced_motion(system);
    rt.update(move |s, _| s.ui.access.reduce_motion = forced);
    apply(&mut rt, ServiceEvent::Transfers(vec![job(1, JobKind::Download, TaskState::Running, 400, 1000)]));
    click(&mut rt, "tb-transfers");
    assert!(shown(&rt, "tr-1"));
    rt.update(|s, _| s.apply(ServiceEvent::Transfers(vec![job(1, JobKind::Download, TaskState::Done, 1000, 1000)])));
    rt
}

#[test]
fn a_finished_job_fills_its_bar_collapses_it_into_the_check_and_the_ring_pulses() {
    let mut rt = finishing(false, false);
    rt.frame(0.0);
    // Filling: the bar is still there and grows, the check waits, the ring stays full.
    let bar = get(&rt, "tr-bar-1");
    assert!(bar.opacity > 0.99 && (bar.scale - 1.0).abs() < 1e-3);
    assert!(get(&rt, "tr-check-1").opacity < 0.01, "the check waits");
    rt.frame(0.05);
    let path = get(&rt, "tr-bar-1").path;
    assert!(rt.elements().iter().any(|e| e.path.starts_with(&path) && e.path != path && e.moving), "the bar fills");
    assert!(shown(&rt, "tb-ring") && !shown(&rt, "tb-badge"), "the ring stays for it, with nothing left to count");
    assert!(!pulsing(&rt));

    // Filled: the bar collapses into the check, which lands with a bounce, and the ring pulses.
    rt.frame(0.3);
    let (mut most, mut pulsed) = (0.0_f32, false);
    for _ in 0..12 {
        rt.frame(0.03);
        most = most.max(get(&rt, "tr-check-1").scale);
        pulsed |= pulsing(&rt);
    }
    assert!(most > 1.001, "the check overshoots a little on `settle` ({most})");
    assert!(pulsed, "the ring pulses");
    settle(&mut rt);
    settle(&mut rt);
    let (check, bar) = (get(&rt, "tr-check-1"), get(&rt, "tr-bar-1"));
    assert!((check.scale - 1.0).abs() < 1e-3 && check.opacity > 0.999, "the check at rest");
    assert!(bar.opacity < 0.001 && bar.scale < 0.001, "the bar gone into it");
    assert_eq!(bar.role, "group", "no longer a progress bar");
    assert!(!shown(&rt, "tb-ring") && !pulsing(&rt), "the ring goes once the pulse is over");
    assert!(rt.state.transfers.flourish.pulse().is_none() && !rt.state.transfers.flourish.filling());
}

#[test]
fn under_reduce_motion_the_check_just_appears_and_nothing_pulses() {
    for (forced, system) in [(true, false), (false, true)] {
        let mut rt = finishing(forced, system);
        rt.frame(0.0);
        let (check, bar) = (get(&rt, "tr-check-1"), get(&rt, "tr-bar-1"));
        assert!(
            check.opacity > 0.999 && (check.scale - 1.0).abs() < 1e-3,
            "the check is there at once ({forced}, {system})"
        );
        assert!(bar.opacity < 0.001, "and the bar gone");
        assert!(!shown(&rt, "tb-ring"), "the ring goes with the last active job");
        for _ in 0..30 {
            rt.frame(0.03);
            assert!(!pulsing(&rt), "nothing pulses");
            let check = get(&rt, "tr-check-1");
            assert!((check.scale - 1.0).abs() < 1e-3 && check.opacity > 0.999, "nothing bounces or fades");
        }
    }
}

#[test]
fn a_row_that_arrives_finished_shows_the_check_at_rest() {
    let (mut rt, _rec) = recorded();
    apply(&mut rt, ServiceEvent::Transfers(vec![job(1, JobKind::Download, TaskState::Done, 10, 10)]));
    click(&mut rt, "tb-transfers");
    assert!(get(&rt, "tr-check-1").opacity > 0.999 && get(&rt, "tr-bar-1").opacity < 0.001);
    assert!(!shown(&rt, "tb-ring") && !pulsing(&rt));
}

#[test]
fn nothing_reaches_the_os_from_a_test_the_effects_wait_in_the_state() {
    use crate::state::transfers::{Bar, Dock, OsEffect};
    // Without the window's sink (connected only in a real window, `views::connect_os`).
    let (mut rt, _rec) = recorded();
    rt.state.transfers.watch.os.take();
    apply(&mut rt, ServiceEvent::Transfers(vec![job(1, JobKind::Download, TaskState::Running, 1, 2)]));
    assert_eq!(rt.state.transfers.watch.os.take(), [OsEffect::Taskbar(Dock { count: 1, bar: Bar::Busy })]);
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
    assert_eq!(
        uploads(&rec),
        [(paths(&["a.txt", "b.txt"]), SITE.to_owned()), (paths(&["c.png"]), format!("{SITE}/uploads"))]
    );
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
    j.conflict = Some(clash(Side::default(), Side::default()));
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
fn the_conflict_sheet_shows_both_sizes_and_dates_and_says_which_is_newer() {
    let (mut rt, _rec) = recorded();
    let mut j = job(1, JobKind::Upload, TaskState::Preparing { files_found: 0 }, 0, 0);
    // The server's file: 14.8 KB from Nov 20; the upload: 15.2 KB from Nov 25 (the newer).
    let old = Side { size: Some(14_812), mtime: Some(1_795_165_920) };
    let new = Side { size: Some(15_240), mtime: Some(1_795_614_480) };
    j.conflict = Some(clash(old, new));
    apply(&mut rt, ServiceEvent::Transfers(vec![j.clone()]));
    assert!(shown(&rt, "conflict"));
    assert_eq!(text_of(&rt, "conflict-existing-label"), "EXISTING");
    assert_eq!(text_of(&rt, "conflict-incoming-label"), "INCOMING");
    assert_eq!(text_of(&rt, "conflict-existing-size"), "14.8 KB");
    assert_eq!(text_of(&rt, "conflict-incoming-size"), "15.2 KB");
    assert_eq!(text_of(&rt, "conflict-existing-date"), "Nov 20, 2026");
    assert_eq!(text_of(&rt, "conflict-incoming-date"), "Nov 25, 2026 · newer", "marked in words, not by colour");

    // The other way round, and a folder (no size) over an older file.
    let mut swapped = j.clone();
    swapped.conflict = Some(clash(new, Side { size: None, mtime: Some(1_795_165_920) }));
    apply(&mut rt, ServiceEvent::Transfers(vec![swapped]));
    assert_eq!(text_of(&rt, "conflict-existing-date"), "Nov 25, 2026 · newer");
    assert_eq!(
        (text_of(&rt, "conflict-incoming-size"), text_of(&rt, "conflict-incoming-date")),
        ("—".into(), "Nov 20, 2026".into())
    );

    // The same date on both: neither is newer. Unknown on one side: a dash.
    let mut same = j.clone();
    same.conflict = Some(clash(old, Side { size: Some(1), mtime: old.mtime }));
    apply(&mut rt, ServiceEvent::Transfers(vec![same]));
    assert_eq!(text_of(&rt, "conflict-existing-date"), text_of(&rt, "conflict-incoming-date"));
    assert!(!text_of(&rt, "conflict-existing-date").contains("newer"));
    let mut unknown = j.clone();
    unknown.conflict = Some(clash(old, Side::default()));
    apply(&mut rt, ServiceEvent::Transfers(vec![unknown]));
    assert_eq!(
        (text_of(&rt, "conflict-incoming-size"), text_of(&rt, "conflict-incoming-date")),
        ("—".into(), "—".into())
    );
    assert!(!text_of(&rt, "conflict-existing-date").contains("newer"));

    // Nothing known of either: no comparison, the rest of the sheet stays.
    let mut none = j;
    none.conflict = Some(clash(Side::default(), Side::default()));
    apply(&mut rt, ServiceEvent::Transfers(vec![none]));
    assert!(!shown(&rt, "conflict-sides") && shown(&rt, "conflict-replace") && shown(&rt, "conflict-explain"));
}

/// The horizontal centre of `k`'s rect.
fn centre_x(rt: &Runtime<App>, k: &str) -> f32 {
    let r = get(rt, k).rect;
    r[0] + r[2] / 2.0
}

/// ⌘D on robots.txt, and one frame.
fn download_robots(rt: &mut Runtime<App>) {
    click(rt, "rows/robots.txt");
    rt.key(KeyEvent { key: Key::Command('d'), mods: primary() });
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_download_sends_a_circle_from_the_list_up_into_the_transfers_button() {
    let mut rt = runtime();
    download_robots(&mut rt);
    let first = get(&rt, "tb-started-1");
    assert!((centre_x(&rt, "tb-started-1") - centre_x(&rt, "tb-transfers")).abs() <= 0.5, "on the button's centre");
    // From the middle of the content area, between the toolbar and the status bar.
    let (toolbar, status) = (get(&rt, "toolbar").rect, get(&rt, "status").rect);
    let middle = (toolbar[1] + toolbar[3] + status[1]) / 2.0;
    assert!((first.rect[1] + first.rect[3] / 2.0 - middle).abs() <= 1.0, "{:?} from {middle}", first.rect);
    assert!(first.inert, "the pointer passes through it");
    let read = crate::ui_a11y_tests::tree(&rt);
    assert!(!read.iter().any(|n| n.key.starts_with("tb-started")), "screen readers don't hear it");
    // It rises at an even x, toward the button.
    let mut y = first.rect[1];
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
        let e = get(&rt, "tb-started-1");
        assert!(e.rect[1] <= y, "it only rises ({} after {y})", e.rect[1]);
        assert!((centre_x(&rt, "tb-started-1") - centre_x(&rt, "tb-transfers")).abs() <= 0.5);
        y = e.rect[1];
    }
    assert!(y < first.rect[1], "it moved");
    // Half a second in, it's gone.
    for _ in 0..12 {
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("tb-started-1")), "gone");
    assert_eq!(rt.state.transfers.flourish.started(), None);
}

#[test]
fn under_reduce_motion_a_download_sends_nothing_flying() {
    for (forced, system) in [(true, false), (false, true)] {
        let mut rt = runtime();
        rt.set_system_reduced_motion(system);
        rt.update(move |s, _| s.ui.access.reduce_motion = forced);
        download_robots(&mut rt);
        assert_eq!(rt.state.transfers.flourish.started(), Some(1), "it's the view that keeps still");
        for _ in 0..40 {
            assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("tb-started-1")), "({forced}, {system})");
            rt.frame(1.0 / 60.0);
        }
    }
}

#[test]
fn a_clash_waits_in_the_open_popover_until_resolve_brings_its_sheet() {
    let (mut rt, rec) = recorded();
    click(&mut rt, "tb-transfers");
    let mut j = job(7, JobKind::Upload, TaskState::Preparing { files_found: 0 }, 0, 0);
    j.conflict = Some(clash(Side::default(), Side::default()));
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
        Some(&TransferCommand::Resolve {
            job: JobId(7),
            choice: dromon_transfer::ConflictChoice::Replace,
            apply_all: false
        })
    );
}
