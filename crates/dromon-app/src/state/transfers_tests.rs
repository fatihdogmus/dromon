//! Transfers in the app state: what downloads and uploads send, and how the mirror of the
//! engine's jobs drives the popover, the ring and the conflict sheet.

use std::path::PathBuf;

use dromon_core::{EntryKind, HostId, LinkTarget, RemotePath};
use dromon_transfer::{
    ConflictChoice, ConflictView, JobId, JobKind, JobRequest, JobView, Progress, RemoteItem, Reveal, Side, TaskState,
};

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
        folders: Vec::new(),
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
    assert_eq!(
        picked,
        [("current", EntryKind::Symlink { target: Some(LinkTarget::Dir) }), ("robots.txt", EntryKind::File)]
    );

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
    let clash = |name: &str| ConflictView {
        name: name.into(),
        folder: RemotePath::new(SITE),
        existing_is_dir: false,
        existing: Side::default(),
        incoming: Side::default(),
        more: 0,
    };
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
        Some(&Command::Transfer(TransferCommand::Resolve {
            job: JobId(7),
            choice: ConflictChoice::KeepBoth,
            apply_all: true
        }))
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
    j.conflict = Some(ConflictView {
        name: "index.html".into(),
        folder: RemotePath::new(SITE),
        existing_is_dir: false,
        existing: Side::default(),
        incoming: Side::default(),
        more: 0,
    });
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

#[test]
fn closing_with_transfers_running_asks_first() {
    let (mut s, _) = connected();
    // Nothing running (a finished job doesn't count): the window just closes.
    s.apply(ServiceEvent::Transfers(vec![job(1, JobKind::Download, TaskState::Done, 1, 1)]));
    assert!(s.close_requested());
    assert!(!s.ui.confirm_quit);

    s.apply(ServiceEvent::Transfers(vec![job(2, JobKind::Upload, TaskState::Running, 1, 9)]));
    assert!(!s.close_requested(), "held for the question");
    assert!(s.ui.confirm_quit);
    // The list's shortcuts sleep behind the sheet.
    let cursor = s.conn().unwrap().selection.cursor().map(str::to_owned);
    s.shortcut(crate::state::Shortcut::Move { delta: 1, extend: false }, 0.0);
    assert_eq!(s.conn().unwrap().selection.cursor().map(str::to_owned), cursor);

    // Cancel (or Escape) keeps going; the next close asks again.
    s.act(Action::CancelQuit);
    assert!(!s.ui.confirm_quit);
    assert!(!s.close_requested());
    s.act(Action::DismissSheet);
    assert!(!s.ui.confirm_quit);

    // Quit closes the window, and the close it starts goes through.
    assert!(!s.close_requested());
    assert_eq!(s.act(Action::QuitAnyway), [Effect::CloseWindow]);
    assert!(!s.ui.confirm_quit);
    assert!(s.close_requested());
}

#[test]
fn the_folder_on_screen_is_listed_again_when_a_job_changing_it_ends() {
    let (mut s, rec) = connected();
    let lists = |rec: &Recorder| rec.sent().iter().filter(|c| matches!(c, Command::List { .. })).count();
    let site = (prod(), RemotePath::new(SITE));
    let elsewhere = (prod(), RemotePath::new("/srv/other"));
    let with =
        |id, state, folders: Vec<(HostId, RemotePath)>| JobView { folders, ..job(id, JobKind::Move, state, 0, 1) };

    // Running, then done: one listing, when it ends.
    s.apply(ServiceEvent::Transfers(vec![with(1, TaskState::Running, vec![elsewhere.clone(), site.clone()])]));
    assert_eq!(lists(&rec), 0);
    s.apply(ServiceEvent::Transfers(vec![with(1, TaskState::Done, vec![elsewhere.clone(), site.clone()])]));
    assert_eq!(rec.sent().last(), Some(&Command::List { host: prod(), path: RemotePath::new(SITE) }));
    assert_eq!(lists(&rec), 1, "only the folder on screen");
    // Seen again (another job moved on): nothing more.
    s.apply(ServiceEvent::Transfers(vec![with(1, TaskState::Done, vec![site.clone()])]));
    assert_eq!(lists(&rec), 1);
    // A job that is over in its first snapshot (failed, or fast) counts as ending.
    s.apply(ServiceEvent::Transfers(vec![with(2, TaskState::Failed { error: "x".into() }, vec![site.clone()])]));
    assert_eq!(lists(&rec), 2);
    // Another folder, or a download (no remote folders), lists nothing.
    s.apply(ServiceEvent::Transfers(vec![with(3, TaskState::Done, vec![elsewhere])]));
    s.apply(ServiceEvent::Transfers(vec![job(4, JobKind::Download, TaskState::Done, 1, 1)]));
    assert_eq!(lists(&rec), 2);
}

#[test]
fn changed_folders_are_the_destination_and_for_moves_the_sources() {
    let item = |p: &str| RemoteItem { path: RemotePath::new(p), kind: EntryKind::File, size: 1 };
    let (a, b) = (prod(), HostId("nas".into()));
    let into = RemotePath::new("/dst");
    let items = vec![item("/src/one"), item("/src/two"), item("/other/three")];
    let copy = JobRequest::RemoteCopy { host: a.clone(), items: items.clone(), into: into.clone() };
    assert_eq!(copy.changed_folders(), [(a.clone(), into.clone())]);
    let moving = JobRequest::CrossHostMove { from: a.clone(), items: items.clone(), to: b.clone(), into: into.clone() };
    assert_eq!(
        moving.changed_folders(),
        [(b.clone(), into.clone()), (a.clone(), RemotePath::new("/src")), (a.clone(), RemotePath::new("/other"))]
    );
    let download = JobRequest::Download { host: a, items, into: PathBuf::from("/tmp") };
    assert!(download.changed_folders().is_empty());
}

#[test]
fn a_flourish_fills_then_collapses_and_the_ring_pulses_once() {
    use super::{FILL, PULSE};
    let f = Flourish::Finished { from: 0.4 };
    assert_eq!(f.step(10.0), Some(Flourish::Filling { until: 10.0 + FILL }));
    let filling = Flourish::Filling { until: 10.3 };
    assert_eq!(filling.step(10.2), Some(filling), "still filling");
    assert_eq!(filling.step(10.3), None, "filled: it collapses");

    let mut tr = Transfers::default();
    tr.set_at(vec![job(1, JobKind::Download, TaskState::Running, 4, 10)], 0);
    assert_eq!(tr.flourish.step(0.0), Stepped::default(), "nothing finished: nothing to do");
    tr.set_at(vec![job(1, JobKind::Download, TaskState::Done, 10, 10)], 1_000);
    assert_eq!(tr.flourish.get(JobId(1)), Some(Flourish::Finished { from: 0.4 }), "from where its bar was");
    assert_eq!(tr.ring_fraction(), Some(1.0), "the ring stays, full, for it");

    // The first frame starts the fill; the next steps wait for it.
    let s = tr.flourish.step(5.0);
    assert_eq!(s, Stepped { changed: true, wake: Some(5.0 + FILL) });
    assert_eq!(tr.flourish.step(5.1), Stepped { changed: false, wake: Some(5.0 + FILL) });
    // Filled: the bar collapses into the check and the ring pulses.
    let s = tr.flourish.step(5.0 + FILL);
    assert_eq!(s, Stepped { changed: true, wake: Some(5.0 + FILL + PULSE) });
    assert_eq!(tr.flourish.get(JobId(1)), None, "at rest: the check");
    assert_eq!(tr.flourish.pulse(), Some(1));
    assert_eq!(tr.ring_fraction(), Some(1.0));
    // The pulse ends; then nothing is left to do, and the ring goes.
    assert!(!tr.flourish.step(5.5).changed);
    assert_eq!(tr.flourish.step(5.0 + FILL + PULSE), Stepped { changed: true, wake: None });
    assert_eq!((tr.flourish.pulse(), tr.ring_fraction()), (None, None));
    assert_eq!(tr.flourish.step(9.0), Stepped::default());
}

#[test]
fn each_download_the_user_starts_plays_the_started_animation_once() {
    let (mut s, rec) = connected();
    assert_eq!(s.transfers.flourish.started(), None);
    // Two rows, one action: one animation.
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    s.act(Action::Click { name: "index.html".into(), extend: false, toggle: true });
    s.act(Action::Download);
    assert_eq!(s.transfers.flourish.started(), Some(1));
    // ⌘D and opening a file start one each, in place of the one playing.
    s.shortcut(crate::state::Shortcut::Download, 0.0);
    assert_eq!(s.transfers.flourish.started(), Some(2));
    s.act(Action::Open("README.md".into()));
    assert_eq!(s.transfers.flourish.started(), Some(3));
    assert_eq!(started(&rec).len(), 3);
    // Nothing to download: nothing plays.
    s.act(Action::Escape);
    s.act(Action::Download);
    assert_eq!((started(&rec).len(), s.transfers.flourish.started()), (3, Some(3)));
}

#[test]
fn the_started_animation_plays_half_a_second_from_the_next_frame() {
    use super::STARTED;
    assert_eq!(Started::Asked.step(2.0), Some(Started::Playing { until: 2.0 + STARTED }));
    let playing = Started::Playing { until: 2.5 };
    assert_eq!(playing.step(2.4), Some(playing));
    assert_eq!(playing.step(2.5), None);

    let (mut s, _rec) = connected();
    s.act(Action::Open("robots.txt".into()));
    let fl = &mut s.transfers.flourish;
    // The first frame times it (it shows already) and asks to be woken when it's over.
    assert_eq!(fl.step(10.0), Stepped { changed: false, wake: Some(10.0 + STARTED) });
    assert_eq!(fl.step(10.2), Stepped { changed: false, wake: Some(10.0 + STARTED) });
    assert_eq!(fl.started(), Some(1));
    assert_eq!(fl.step(10.0 + STARTED), Stepped { changed: true, wake: None });
    assert_eq!(fl.started(), None);
    assert_eq!(fl.step(11.0), Stepped::default());
}

#[test]
fn uploads_copies_moves_and_an_open_popover_play_no_started_animation() {
    let (mut s, rec) = connected();
    s.act(Action::Upload { paths: vec![PathBuf::from("/Users/me/a.txt")], folder: None });
    s.act(Action::Click { name: "index.html".into(), extend: false, toggle: false });
    s.act(Action::File(crate::state::FileAction::Copy));
    s.act(Action::File(crate::state::FileAction::Paste { text: format!("{SITE}/index.html"), as_move: false }));
    s.act(Action::File(crate::state::FileAction::Paste { text: format!("{SITE}/index.html"), as_move: true }));
    let kinds: Vec<&str> = started(&rec)
        .iter()
        .map(|r| match r {
            JobRequest::Upload { .. } => "upload",
            JobRequest::RemoteCopy { .. } => "copy",
            JobRequest::Move { .. } => "move",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["upload", "copy", "move"]);
    assert_eq!(s.transfers.flourish.started(), None);

    // With the popover open, the new row shows there instead.
    s.act(Action::ToggleTransfers);
    s.act(Action::Download);
    assert_eq!(started(&rec).len(), 4);
    assert_eq!(s.transfers.flourish.started(), None);
    // Closed again, a download plays the first one.
    s.act(Action::ToggleTransfers);
    s.act(Action::Download);
    assert_eq!(s.transfers.flourish.started(), Some(1));
}

#[test]
fn only_jobs_seen_running_get_a_flourish_and_clearing_ends_it() {
    let mut tr = Transfers::default();
    // Arrived finished, failed or cancelled: no flourish.
    tr.set_at(vec![job(1, JobKind::Download, TaskState::Done, 1, 1)], 0);
    let running = |id| job(id, JobKind::Upload, TaskState::Running, 1, 2);
    tr.set_at(vec![running(3), running(2), job(1, JobKind::Download, TaskState::Done, 1, 1)], 0);
    tr.set_at(
        vec![
            job(3, JobKind::Upload, TaskState::Cancelled, 1, 2),
            job(2, JobKind::Upload, TaskState::Failed { error: "x".into() }, 1, 2),
            job(1, JobKind::Download, TaskState::Done, 1, 1),
        ],
        0,
    );
    assert!(!tr.flourish.filling());
    assert_eq!(tr.ring_fraction(), None);

    // Two finish together: both fill; another job still runs, so the ring keeps its bytes.
    tr.set_at(vec![running(6), running(5), running(4)], 0);
    tr.set_at(
        vec![
            running(6),
            job(5, JobKind::Upload, TaskState::Done, 2, 2),
            job(4, JobKind::Upload, TaskState::Done, 2, 2),
        ],
        0,
    );
    assert!(tr.flourish.get(JobId(5)).is_some() && tr.flourish.get(JobId(4)).is_some());
    assert_eq!(tr.ring_fraction(), Some(0.5));
    // Clear finished takes them away mid-flourish.
    tr.set_at(vec![running(6)], 0);
    assert!(!tr.flourish.filling());
}

#[test]
fn a_notification_click_opens_the_popover_on_its_jobs_tab() {
    let (mut s, _rec) = connected();
    s.apply(ServiceEvent::Transfers(vec![
        job(2, JobKind::Upload, TaskState::Done, 1, 1),
        job(1, JobKind::Download, TaskState::Done, 1, 1),
    ]));
    s.show_transfer(JobId(1));
    assert!(s.transfers.open);
    assert_eq!(s.transfers.tab, super::Tab::Downloads);
    s.transfers.open = false;
    s.show_transfer(JobId(2));
    assert_eq!((s.transfers.open, s.transfers.tab), (true, super::Tab::Uploads));
    // A job cleared since: the popover opens where it was.
    s.show_transfer(JobId(9));
    assert_eq!((s.transfers.open, s.transfers.tab), (true, super::Tab::Uploads));
}

#[test]
fn the_app_s_jobs_reach_the_taskbar_and_focus_decides_notifications() {
    let (mut s, _rec) = connected();
    s.transfers.watch.os.take();
    s.transfers.window_focus(false);
    s.apply(ServiceEvent::Transfers(vec![job(1, JobKind::Download, TaskState::Running, 1, 2)]));
    s.transfers.set_at(
        vec![job(1, JobKind::Download, TaskState::Failed { error: "Permission denied".into() }, 1, 2)],
        i64::MAX / 2,
    );
    let held = s.transfers.watch.os.take();
    assert!(held.contains(&OsEffect::Taskbar(Dock { count: 1, bar: notify::Bar::Busy })), "{held:?}");
    assert!(held.contains(&OsEffect::Taskbar(Dock { count: 0, bar: notify::Bar::Error })), "{held:?}");
    let lines: Vec<String> =
        held.iter().filter_map(|fx| if let OsEffect::Notify(n) = fx { Some(n.line()) } else { None }).collect();
    assert_eq!(lines, ["Couldn't download job 1: Permission denied"]);
    s.transfers.window_focus(true);
    assert_eq!(s.transfers.watch.os.take(), [OsEffect::Taskbar(Dock::default())]);
}

#[test]
fn picking_files_to_upload_asks_for_the_picker_only_while_interactive() {
    let (mut s, rec) = connected();
    let folder = || Effect::PickUpload { folder: "site".into() };
    let palette_upload = |s: &mut App| {
        s.act(Action::Palette(crate::state::PaletteAction::Toggle));
        let item = s.palette_items().into_iter().find(|i| i.key == "pal-upload");
        s.act(Action::Palette(crate::state::PaletteAction::Toggle));
        item
    };
    assert_eq!(s.act(Action::PickUpload), [folder()]);
    assert_eq!(s.shortcut(crate::state::Shortcut::Upload, 0.0), [folder()], "⌘U");
    let item = palette_upload(&mut s).expect("the palette lists it");
    assert_eq!((item.title.as_str(), item.shortcut, item.action), ("Upload files…", Some("mod+u"), Action::PickUpload));
    assert!(started(&rec).is_empty(), "asking uploads nothing");

    s.conn_mut().unwrap().status = dromon_core::ConnStatus::Reconnecting { attempt: 1 };
    assert!(s.act(Action::PickUpload).is_empty());
    assert!(s.shortcut(crate::state::Shortcut::Upload, 0.0).is_empty());
    assert!(palette_upload(&mut s).is_none(), "hidden while reconnecting");

    let mut none = fixture::app(fixture::demo());
    assert!(none.act(Action::PickUpload).is_empty(), "no host on screen");
    assert!(none.shortcut(crate::state::Shortcut::Upload, 0.0).is_empty());
    assert!(palette_upload(&mut none).is_none());
}
