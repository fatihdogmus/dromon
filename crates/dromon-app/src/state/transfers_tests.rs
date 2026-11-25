//! Transfers in the app state: what downloads and uploads send, and how the mirror of the
//! engine's jobs drives the popover, the ring and the conflict sheet.

use std::path::PathBuf;

use dromon_core::{EntryKind, HostId, LinkTarget, RemotePath};
use dromon_transfer::{ConflictChoice, ConflictView, JobId, JobKind, JobRequest, JobView, Progress, RemoteItem, Reveal, TaskState};

use super::*;
use crate::fixture;
use crate::state::{Action, App, Command, Effect, Recorder, ServiceEvent, TransferCommand};

fn prod() -> HostId {
    HostId("prod".into())
}

const SITE: &str = "/var/www/site";

/// The demo connected to prod at /var/www/site; later commands go to the returned recorder.
fn connected() -> (App, Recorder) {
    let mut s = fixture::app(fixture::demo());
    s.downloads = PathBuf::from("/Users/me/Downloads");
    s.act(Action::SelectHost(prod()));
    let rec = Recorder::default();
    s.backend = Box::new(rec.clone());
    (s, rec)
}

fn started(rec: &Recorder) -> Vec<JobRequest> {
    rec.sent()
        .into_iter()
        .filter_map(|c| match c {
            Command::Transfer(TransferCommand::Start(req)) => Some(req),
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
    }
}

#[test]
fn opening_a_file_downloads_it_and_a_folder_still_navigates() {
    let (mut s, rec) = connected();
    s.act(Action::Open("index.html".into()));
    let item = RemoteItem { path: RemotePath::new("/var/www/site/index.html"), kind: EntryKind::File, size: 14812 };
    assert_eq!(
        started(&rec),
        [JobRequest::Download { host: prod(), items: vec![item], into: PathBuf::from("/Users/me/Downloads") }]
    );
    assert_eq!(s.conn().unwrap().cwd.as_str(), SITE, "a download doesn't navigate");
    s.act(Action::Open("assets".into()));
    assert_eq!(s.conn().unwrap().cwd.as_str(), "/var/www/site/assets");
    assert_eq!(started(&rec).len(), 1);
}

#[test]
fn download_takes_the_selection_in_row_order_or_else_the_cursor() {
    let (mut s, rec) = connected();
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    s.act(Action::Click { name: "current".into(), extend: false, toggle: true });
    s.act(Action::Download);
    let JobRequest::Download { items, .. } = &started(&rec)[0] else { panic!("not a download") };
    let picked: Vec<(&str, EntryKind)> = items.iter().map(|i| (i.path.file_name().unwrap(), i.kind)).collect();
    assert_eq!(picked, [("current", EntryKind::Symlink { target: Some(LinkTarget::Dir) }), ("robots.txt", EntryKind::File)]);

    // Nothing selected: nothing to download.
    s.act(Action::Escape);
    s.act(Action::Download);
    assert_eq!(started(&rec).len(), 1);
}

#[test]
fn nothing_transfers_while_the_connection_isnt_ready() {
    let (mut s, rec) = connected();
    s.conn_mut().unwrap().status = dromon_core::ConnStatus::Reconnecting { attempt: 1 };
    s.act(Action::Open("index.html".into()));
    s.act(Action::Upload { paths: vec![PathBuf::from("/tmp/a.txt")], folder: None });
    assert!(started(&rec).is_empty());
}

#[test]
fn uploads_go_to_the_folder_on_screen_or_a_folder_row() {
    let (mut s, rec) = connected();
    let paths = vec![PathBuf::from("/Users/me/a.txt"), PathBuf::from("/Users/me/photos")];
    s.act(Action::Upload { paths: paths.clone(), folder: None });
    s.act(Action::Upload { paths: paths.clone(), folder: Some("uploads".into()) });
    // A file row or a missing name isn't a place to upload into; an empty drop is nothing.
    s.act(Action::Upload { paths: paths.clone(), folder: Some("index.html".into()) });
    s.act(Action::Upload { paths: paths.clone(), folder: Some("nope".into()) });
    s.act(Action::Upload { paths: Vec::new(), folder: None });
    assert_eq!(
        started(&rec),
        [
            JobRequest::Upload { host: prod(), items: paths.clone(), into: RemotePath::new(SITE) },
            JobRequest::Upload { host: prod(), items: paths, into: RemotePath::new("/var/www/site/uploads") },
        ]
    );
}

#[test]
fn the_ring_sums_active_jobs_and_the_tab_follows_the_newest_job() {
    let (mut s, _) = connected();
    assert_eq!(s.transfers.ring(), None);
    s.apply(ServiceEvent::Transfers(vec![
        job(3, JobKind::Upload, TaskState::Running, 10, 100),
        job(2, JobKind::Download, TaskState::Paused, 50, 100),
        job(1, JobKind::Download, TaskState::Done, 100, 100),
    ]));
    assert_eq!(s.transfers.ring(), Some(Progress { done: 60, total: 200 }));
    assert_eq!(s.transfers.active(), 2);
    assert_eq!(s.transfers.tab, Tab::Uploads);
    assert_eq!(s.transfers.in_tab(Tab::Downloads).map(|j| j.id.0).collect::<Vec<_>>(), [2, 1]);

    // With the popover open, the user's tab stays.
    s.act(Action::ToggleTransfers);
    s.act(Action::TransfersTab(Tab::Downloads));
    s.apply(ServiceEvent::Transfers(vec![job(4, JobKind::Upload, TaskState::Queued, 0, 0)]));
    assert_eq!(s.transfers.tab, Tab::Downloads);
    // Escape closes the popover before anything else.
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    s.act(Action::Escape);
    assert!(!s.transfers.open);
    assert_eq!(s.conn().unwrap().selection.cursor(), Some("robots.txt"));
}

#[test]
fn the_oldest_clash_is_asked_first_and_apply_to_all_goes_with_the_answer() {
    let (mut s, rec) = connected();
    let clash = |name: &str| ConflictView { name: name.into(), folder: RemotePath::new(SITE), existing_is_dir: false, more: 0 };
    let mut newer = job(8, JobKind::Upload, TaskState::Preparing { files_found: 0 }, 0, 0);
    newer.conflict = Some(clash("b.txt"));
    let mut older = job(7, JobKind::Upload, TaskState::Preparing { files_found: 0 }, 0, 0);
    older.conflict = Some(clash("a.txt"));
    s.apply(ServiceEvent::Transfers(vec![newer, older]));
    assert_eq!(s.transfers.conflict().map(|(id, c)| (id, c.name.as_str())), Some((JobId(7), "a.txt")));

    s.act(Action::ToggleApplyAll);
    s.act(Action::ResolveConflict(ConflictChoice::KeepBoth));
    assert_eq!(
        rec.sent().last(),
        Some(&Command::Transfer(TransferCommand::Resolve { job: JobId(7), choice: ConflictChoice::KeepBoth, apply_all: true }))
    );
    assert!(!s.transfers.apply_all, "the next clash starts unticked");
}

#[test]
fn row_buttons_pass_through_to_the_engine() {
    let (mut s, rec) = connected();
    for cmd in [TransferCommand::Pause(JobId(1)), TransferCommand::Retry(JobId(2)), TransferCommand::ClearFinished] {
        s.act(Action::Transfer(cmd.clone()));
        assert_eq!(rec.sent().last(), Some(&Command::Transfer(cmd)));
    }
}

#[test]
fn revealing_a_download_shows_it_in_the_file_manager() {
    let (mut s, _) = connected();
    let mut j = job(1, JobKind::Download, TaskState::Done, 1, 1);
    j.reveal = Some(Reveal::Local(PathBuf::from("/Users/me/Downloads/index.html")));
    s.apply(ServiceEvent::Transfers(vec![j]));
    let fx = s.act(Action::RevealTransfer(JobId(1)));
    assert_eq!(fx, [Effect::ShowInFolder(PathBuf::from("/Users/me/Downloads/index.html"))]);
    assert!(s.act(Action::RevealTransfer(JobId(9))).is_empty());
}

#[test]
fn revealing_an_upload_lists_its_folder_and_selects_it() {
    // The fixture's own backend, so the folder really lists.
    let mut s = fixture::app(fixture::demo());
    s.act(Action::SelectHost(prod()));
    let mut j = job(1, JobKind::Upload, TaskState::Done, 1, 1);
    j.reveal = Some(Reveal::Remote(prod(), RemotePath::new("/var/www/site/releases/20261125-1348/app.min.js")));
    let mut here = job(2, JobKind::Upload, TaskState::Done, 1, 1);
    here.reveal = Some(Reveal::Remote(prod(), RemotePath::new("/var/www/site/robots.txt")));
    s.apply(ServiceEvent::Transfers(vec![j, here]));
    s.transfers.open = true;

    // In the folder on screen: selected and scrolled to.
    let fx = s.act(Action::RevealTransfer(JobId(2)));
    assert!(fx.contains(&Effect::Reveal("robots.txt".into())));
    assert_eq!(s.conn().unwrap().selection.cursor(), Some("robots.txt"));
    assert!(!s.transfers.open);

    // Elsewhere: its folder is listed, then it's selected.
    s.act(Action::RevealTransfer(JobId(1)));
    let c = s.conn().unwrap();
    assert_eq!(c.cwd.as_str(), "/var/www/site/releases/20261125-1348");
    assert_eq!(c.selection.cursor(), Some("app.min.js"));
}

#[test]
fn a_file_drag_shows_where_it_would_upload_and_moving_on_doesnt_lose_it() {
    let (mut s, rec) = connected();
    let hover = |s: &mut App, target: DropTarget, over: bool| s.act(Action::DropHover { target, over, count: 3 });
    hover(&mut s, DropTarget::List, true);
    assert_eq!(s.transfers.drop, Some(DropHover { target: DropTarget::List, count: 3 }));
    // Onto a folder row: the list hears its hover end first, then the row its start.
    hover(&mut s, DropTarget::List, false);
    hover(&mut s, DropTarget::Folder("uploads".into()), true);
    assert_eq!(s.transfers.drop.as_ref().map(|d| &d.target), Some(&DropTarget::Folder("uploads".into())));
    // A late end from a target the drag already left changes nothing.
    hover(&mut s, DropTarget::List, false);
    assert!(s.transfers.drop.is_some());
    // The drop ends it, and uploads.
    s.act(Action::Upload { paths: vec![PathBuf::from("/a")], folder: Some("uploads".into()) });
    assert_eq!(s.transfers.drop, None);
    assert_eq!(started(&rec).len(), 1);

    // A file row isn't a folder, and an empty drag shows nothing.
    hover(&mut s, DropTarget::Folder("index.html".into()), true);
    assert_eq!(s.transfers.drop, None);
    s.act(Action::DropHover { target: DropTarget::List, over: true, count: 0 });
    assert_eq!(s.transfers.drop, None);
    // Nor while the connection is away.
    s.conn_mut().unwrap().status = dromon_core::ConnStatus::Reconnecting { attempt: 1 };
    hover(&mut s, DropTarget::List, true);
    assert_eq!(s.transfers.drop, None);
}

#[test]
fn the_conflict_sheet_waits_behind_prompts_and_holds_the_keyboard() {
    let (mut s, rec) = connected();
    let mut j = job(1, JobKind::Upload, TaskState::Preparing { files_found: 0 }, 0, 0);
    j.conflict = Some(ConflictView { name: "index.html".into(), folder: RemotePath::new(SITE), existing_is_dir: false, more: 0 });
    s.apply(ServiceEvent::Transfers(vec![j]));
    assert_eq!(s.conflict_on_screen().map(|(id, _)| id), Some(JobId(1)));

    // The list's shortcuts don't reach behind it.
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    s.shortcut(crate::state::Shortcut::Move { delta: 1, extend: false }, 0.0);
    s.shortcut(crate::state::Shortcut::Download, 0.0);
    assert_eq!(s.conn().unwrap().selection.cursor(), Some("robots.txt"));
    assert!(started(&rec).is_empty());

    // A prompt comes first.
    let prompt = dromon_core::Prompt {
        id: dromon_core::PromptId(1),
        host: prod(),
        kind: dromon_core::PromptKind::Password,
        text: "pw: ".into(),
        error: None,
    };
    s.apply(ServiceEvent::Prompt(prompt));
    assert!(s.conflict_on_screen().is_none());
}

#[test]
fn cmd_d_downloads_the_selection() {
    let (mut s, rec) = connected();
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    s.shortcut(crate::state::Shortcut::Download, 0.0);
    assert_eq!(started(&rec).len(), 1);
}

#[test]
fn revealing_never_connects_a_host() {
    let mut s = fixture::app(fixture::demo());
    let mut j = job(1, JobKind::Upload, TaskState::Done, 1, 1);
    j.reveal = Some(Reveal::Remote(prod(), RemotePath::new("/var/www/site/a")));
    s.apply(ServiceEvent::Transfers(vec![j]));
    s.act(Action::RevealTransfer(JobId(1)));
    assert!(s.active.is_none());
    assert!(s.conns.is_empty());
}
