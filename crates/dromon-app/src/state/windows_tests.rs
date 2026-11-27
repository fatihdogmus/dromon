//! Several main windows as their states see each other: two `App`s on one fixture backend and
//! one `Common`, with test peers that record what reaches the Rune side.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use dromon_core::{HostId, RemotePath};
use dromon_transfer::{JobId, JobKind, JobView, Progress, TaskState};

use super::*;
use crate::fixture;
use crate::state::prefs::{Saved, Store};
use crate::state::transfers::OsEffect;
use crate::state::{Action, FileAction, Listing, SettingsChange, Shortcut, TransferCommand};
use crate::theme::Appearance;

#[derive(Default)]
struct Log {
    opened: Vec<App>,
    news: Vec<(WindowId, News)>,
    frames: BTreeMap<WindowId, Frame>,
    closed: Vec<WindowId>,
}

/// Peers that keep what they're handed.
#[derive(Clone, Default)]
struct TestPeers(Rc<RefCell<Log>>);

impl Peers for TestPeers {
    fn can_open(&self) -> bool {
        true
    }
    fn open_window(&self, _: WindowId, app: App) {
        self.0.borrow_mut().opened.push(app);
    }
    fn tell(&self, from: WindowId, news: News) {
        self.0.borrow_mut().news.push((from, news));
    }
    fn frame(&self, window: WindowId) -> Option<Frame> {
        self.0.borrow().frames.get(&window).copied()
    }
    fn closed(&self, window: WindowId) {
        self.0.borrow_mut().closed.push(window);
    }
}

impl TestPeers {
    /// Hands every news item to the windows it's for (as `crate::windows` does).
    fn deliver(&self, windows: &mut [&mut App]) {
        let news = std::mem::take(&mut self.0.borrow_mut().news);
        for (from, n) in news {
            for w in windows.iter_mut().filter(|w| w.window != from) {
                w.hear(n.clone());
            }
        }
    }
}

/// What a store was given, last first.
#[derive(Clone, Default)]
struct Recorded(Rc<RefCell<Vec<Saved>>>);

impl Store for Recorded {
    fn save(&mut self, saved: &Saved) {
        self.0.borrow_mut().push(saved.clone());
    }
}

fn prod() -> HostId {
    HostId("prod".into())
}

/// The first window on the demo fixture, with test peers.
fn first() -> (App, TestPeers) {
    let mut a = fixture::app(fixture::demo());
    let peers = TestPeers::default();
    a.set_peers(Rc::new(peers.clone()));
    (a, peers)
}

/// A second window opened from `a` with ⌘N.
fn second(a: &mut App, peers: &TestPeers) -> App {
    a.act(Action::NewWindow);
    peers.0.borrow_mut().opened.pop().expect("a window opened")
}

fn cwd(a: &App) -> Option<RemotePath> {
    a.conn().map(|c| c.cwd.clone())
}

fn rows(a: &App) -> Vec<String> {
    a.conn().map(|c| c.row_names().into_iter().map(str::to_owned).collect()).unwrap_or_default()
}

#[test]
fn a_new_window_starts_on_the_welcome_state_with_the_same_hosts() {
    let (mut a, peers) = first();
    a.act(Action::SelectHost(prod()));
    assert!(a.conn().is_some_and(|c| c.opened));
    assert_eq!(a.shortcut(Shortcut::NewWindow, 0.0), Vec::new());
    let b = peers.0.borrow_mut().opened.pop().expect("⌘N opens a window");
    assert_eq!(b.window, WindowId(1));
    assert_eq!(b.active, None, "Pick a host");
    assert_eq!(b.hosts.hosts, a.hosts.hosts);
    assert_eq!((a.windows_open(), b.windows_open()), (2, 2));
    assert!(a.leads() && !b.leads(), "the first window speaks for the app");
}

#[test]
fn each_window_has_its_own_folder_on_one_connection() {
    let (mut a, peers) = first();
    a.act(Action::SelectHost(prod()));
    let home = cwd(&a).unwrap();
    let mut b = second(&mut a, &peers);
    // The host is up already: the second window opens on it at once, without signing in again.
    b.act(Action::SelectHost(prod()));
    assert!(b.conn().is_some_and(|c| c.opened && c.interactive()));
    let first_dir = a.conn().unwrap().rows().find(|e| e.kind.is_dir_like()).unwrap().name.clone();
    b.act(Action::Open(first_dir.clone()));
    let there = home.join(&first_dir);
    assert_eq!(cwd(&b), Some(there.clone()));
    // The first window catches up on what the second's commands brought, and stays put.
    a.catch_up();
    assert_eq!(cwd(&a), Some(home.clone()), "a's folder isn't b's");
    assert!(matches!(a.conn().unwrap().listing, Listing::Ready(_)));
    // Going back in the first window doesn't move the second.
    a.act(Action::GoUp);
    b.catch_up();
    assert_eq!(cwd(&b), Some(there));
    assert_eq!(cwd(&a), home.parent());
}

#[test]
fn the_clip_is_the_apps_a_copy_in_one_window_pastes_in_another() {
    let (mut a, peers) = first();
    a.act(Action::SelectHost(prod()));
    let mut b = second(&mut a, &peers);
    b.act(Action::SelectHost(prod()));
    let file = a.conn().unwrap().rows().find(|e| !e.kind.is_dir_like()).unwrap().name.clone();
    a.act(Action::Click { name: file.clone(), extend: false, toggle: false });
    let fx = a.act(Action::File(FileAction::Cut));
    let Some(Effect::Copy(text)) = fx.first().cloned() else { panic!("{fx:?}") };
    assert!(b.clip().as_ref().is_some_and(|c| c.cut), "b sees a's cut");
    assert!(peers.0.borrow().news.iter().any(|(from, n)| *from == a.window && *n == News::Clip), "and redraws for it");
    // Pasting in b moves it (a cut), and the clip empties for both.
    let dir = b.conn().unwrap().rows().find(|e| e.kind.is_dir_like()).unwrap().name.clone();
    b.act(Action::Open(dir));
    b.act(Action::File(FileAction::Paste { text, as_move: false }));
    assert_eq!(*a.clip(), None);
    assert!(b.transfers.jobs.iter().any(|j| j.kind == JobKind::Move), "{:?}", b.transfers.jobs);
    // Every window shows every job.
    a.catch_up();
    assert_eq!(a.transfers.jobs.len(), b.transfers.jobs.len());
    assert!(!rows(&a).contains(&file), "gone from the folder a shows: {:?}", rows(&a));
}

#[test]
fn settings_and_recent_hosts_reach_every_window() {
    let (mut a, peers) = first();
    let store = Recorded::default();
    a.remember(Saved::default(), Box::new(store.clone()));
    let mut b = second(&mut a, &peers);
    a.act(Action::Setting(SettingsChange::Theme(Appearance::Light)));
    a.act(Action::Setting(SettingsChange::TransfersAtOnce(5)));
    peers.deliver(&mut [&mut a, &mut b]);
    assert_eq!(b.settings.theme, Appearance::Light);
    assert_eq!(b.ui.appearance, Appearance::Light);
    assert_eq!(b.settings.transfers_at_once, 5);
    assert_eq!(
        store.0.borrow().last().and_then(|s| s.settings.clone()),
        Some(a.settings.clone()),
        "remembered once, shared"
    );

    // A quick connect in b is a recent host in a too.
    b.act(Action::QuickConnect);
    b.ui.quick.set_text(dromon_ssh::QuickField::Host, "deploy@prod.example.com");
    b.act(Action::HostBook(crate::state::HostAction::Connect));
    peers.deliver(&mut [&mut a, &mut b]);
    let names = |s: &App| s.recent_hosts().into_iter().map(|h| h.alias).collect::<Vec<_>>();
    assert_eq!(names(&a), names(&b));
    assert!(!names(&a).is_empty());
    assert_eq!(store.0.borrow().last().map(|s| s.recent.len()), Some(names(&a).len()));
}

fn running_job() -> JobView {
    JobView {
        id: JobId(9),
        kind: JobKind::Download,
        host: prod(),
        title: "big.iso".into(),
        state: TaskState::Running,
        progress: Progress { done: 1, total: 10 },
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
fn a_download_plays_the_started_animation_only_in_the_window_that_started_it() {
    let (mut a, peers) = first();
    a.act(Action::SelectHost(prod()));
    let mut b = second(&mut a, &peers);
    b.act(Action::SelectHost(prod()));
    let file = a.conn().unwrap().rows().find(|e| !e.kind.is_dir_like()).unwrap().name.clone();
    a.act(Action::Open(file));
    b.catch_up();
    assert_eq!(b.transfers.jobs.len(), 1, "b shows the job");
    assert_eq!((a.transfers.flourish.started(), b.transfers.flourish.started()), (Some(1), None));
}

#[test]
fn only_the_last_window_asks_before_closing_with_transfers_running() {
    let (mut a, peers) = first();
    let mut b = second(&mut a, &peers);
    for w in [&mut a, &mut b] {
        w.apply(crate::state::ServiceEvent::Transfers(vec![running_job()]));
        assert_eq!(w.transfers.active(), 1);
    }
    // Two open: either closes at once.
    assert_eq!(b.act(Action::CloseWindow), [Effect::CloseWindow]);
    assert!(!b.ui.confirm_quit);
    b.window_closed();
    assert_eq!(peers.0.borrow().closed, [WindowId(1)]);
    assert_eq!(a.windows_open(), 1);
    assert!(a.leads());
    // The last one asks; Quit there closes it.
    assert_eq!(a.act(Action::CloseWindow), Vec::new());
    assert!(a.ui.confirm_quit);
    assert_eq!(a.act(Action::QuitAnyway), [Effect::CloseWindow]);
    let _ = a.act(Action::Transfer(TransferCommand::ClearFinished));
}

#[test]
fn the_last_windows_frame_is_kept_and_the_store_goes_once_every_window_quit() {
    let (mut a, peers) = first();
    let store = Recorded::default();
    a.remember(Saved::default(), Box::new(store.clone()));
    let mut b = second(&mut a, &peers);
    let mut c = second(&mut a, &peers);
    let frame = |x| Frame { x, y: 40, width: 1080, height: 680, maximized: false };
    for (w, x) in [(a.window, 10), (b.window, 40), (c.window, 70)] {
        peers.0.borrow_mut().frames.insert(w, frame(x));
    }
    // A window closing while others stay forgets its frame.
    c.window_closed();
    assert!(store.0.borrow().last().is_none_or(|s| s.frames.is_empty()));
    // A quit: every window's frame, the first first; the store is dropped (written) once the
    // last window's hook ran.
    a.quitting();
    assert_eq!(store.0.borrow().last().map(|s| s.frames.clone()), Some(vec![frame(10)]));
    let before = store.0.borrow().len();
    b.quitting();
    assert_eq!(store.0.borrow().last().map(|s| s.frames.clone()), Some(vec![frame(10), frame(40)]));
    assert_eq!(store.0.borrow().len(), before + 1);
    // The store is gone: later saves go nowhere.
    a.sidebar_resized();
    assert_eq!(store.0.borrow().len(), before + 1);
}

#[test]
fn the_last_window_closing_keeps_its_frame() {
    let (mut a, peers) = first();
    let store = Recorded::default();
    a.remember(Saved::default(), Box::new(store.clone()));
    let mut b = second(&mut a, &peers);
    peers.0.borrow_mut().frames.insert(a.window, Frame { x: 5, y: 6, width: 900, height: 600, maximized: false });
    peers.0.borrow_mut().frames.insert(b.window, Frame { x: 50, y: 60, width: 1000, height: 700, maximized: true });
    a.window_closed();
    b.window_closed();
    assert_eq!(
        store.0.borrow().last().map(|s| s.frames.clone()),
        Some(vec![Frame { x: 50, y: 60, width: 1000, height: 700, maximized: true }])
    );
    b.quitting();
}

#[test]
fn file_operations_of_two_windows_never_share_an_id() {
    let (mut a, peers) = first();
    a.act(Action::SelectHost(prod()));
    let mut b = second(&mut a, &peers);
    b.act(Action::SelectHost(prod()));
    a.act(Action::File(FileAction::NewFolder));
    a.act(Action::File(FileAction::EditSubmit));
    b.act(Action::File(FileAction::NewFolder));
    b.act(Action::File(FileAction::EditSubmit));
    let (ia, ib) = (a.ops.ops[0].id, b.ops.ops[0].id);
    assert_ne!(ia, ib);
    // The other window's answers (it hears them too) change nothing of its own.
    a.catch_up();
    b.catch_up();
    assert_eq!((a.ops.ops.len(), b.ops.ops.len()), (1, 1));
}

/// What reached the OS from the windows, as macOS takes it: the Dock shows the sum of every
/// window's last badge count (a closed window's too); notifications by the window that posted.
#[derive(Default)]
struct Desk {
    badges: BTreeMap<WindowId, u32>,
    notices: Vec<(WindowId, String)>,
}

impl Desk {
    /// Takes what each window handed its OS sink since the last time.
    fn take(&mut self, windows: &mut [&mut App]) {
        for w in windows.iter_mut() {
            for fx in w.transfers.watch.os.take() {
                match fx {
                    OsEffect::Taskbar(dock) => {
                        self.badges.insert(w.window, dock.count);
                    }
                    OsEffect::Notify(n) => self.notices.push((w.window, n.title)),
                }
            }
        }
    }

    /// The Dock icon's badge.
    fn badge(&self) -> u32 {
        self.badges.values().sum()
    }
}

fn at_0() -> i64 {
    0
}

fn at_20s() -> i64 {
    20_000
}

fn at_40s() -> i64 {
    40_000
}

fn done(job: &JobView) -> JobView {
    JobView { state: TaskState::Done, progress: Progress { done: 10, total: 10 }, ..job.clone() }
}

fn snapshot(windows: &mut [&mut App], clock: fn() -> i64, jobs: &[JobView]) {
    for w in windows.iter_mut() {
        w.transfers.watch.clock = clock;
        w.apply(crate::state::ServiceEvent::Transfers(jobs.to_vec()));
    }
}

#[test]
fn only_the_leading_window_sets_the_badge() {
    let (mut a, peers) = first();
    let mut b = second(&mut a, &peers);
    let mut desk = Desk::default();
    desk.take(&mut [&mut a, &mut b]);
    let other = JobView { id: JobId(10), title: "small.iso".into(), ..running_job() };
    snapshot(&mut [&mut a, &mut b], at_0, &[running_job(), other]);
    desk.take(&mut [&mut a, &mut b]);
    // Two windows show the two jobs; the Dock says 2, not 4.
    assert_eq!(desk.badges, BTreeMap::from([(a.window, 2)]));
    assert_eq!(desk.badge(), 2);
    assert!(a.transfers.watch.speaks() && !b.transfers.watch.speaks());
    // b knows what the badge says, for when it leads, and a new native window of its own
    // (`views::connect_window`) shows nothing either.
    assert_eq!(b.transfers.watch.shown(), a.transfers.watch.shown());
    b.transfers.watch.reapply();
    desk.take(&mut [&mut a, &mut b]);
    assert_eq!(desk.badges, BTreeMap::from([(a.window, 2)]));
    // Both see a job end; the badge goes down once.
    snapshot(&mut [&mut a, &mut b], at_0, &[running_job()]);
    desk.take(&mut [&mut a, &mut b]);
    assert_eq!(desk.badge(), 1);
}

#[test]
fn a_job_is_notified_once_for_two_windows_and_only_while_no_window_has_focus() {
    let (mut a, peers) = first();
    let mut b = second(&mut a, &peers);
    let mut desk = Desk::default();
    let other = JobView { id: JobId(10), title: "small.iso".into(), ..running_job() };
    snapshot(&mut [&mut a, &mut b], at_0, &[running_job(), other.clone()]);
    // Nobody looks at the app: big.iso ends 20 s later, in both windows' snapshots.
    snapshot(&mut [&mut a, &mut b], at_20s, &[done(&running_job()), other.clone()]);
    desk.take(&mut [&mut a, &mut b]);
    assert_eq!(desk.notices, [(a.window, "Downloaded big.iso".to_owned())]);
    // The done flourish is each window's own.
    assert!(a.transfers.flourish.get(JobId(9)).is_some() && b.transfers.flourish.get(JobId(9)).is_some());

    // b has focus: the app has it, in a (which notifies) too. The focus moving from b to a,
    // a's gain heard before b's loss, keeps it.
    b.focus_moved(true);
    peers.deliver(&mut [&mut a, &mut b]);
    assert!(a.transfers.watch.focused && b.transfers.watch.focused);
    a.focus_moved(true);
    b.focus_moved(false);
    peers.deliver(&mut [&mut a, &mut b]);
    assert!(a.transfers.watch.focused && b.transfers.watch.focused);
    snapshot(&mut [&mut a, &mut b], at_40s, &[done(&running_job()), done(&other)]);
    desk.take(&mut [&mut a, &mut b]);
    assert_eq!(desk.notices.len(), 1, "small.iso ended while the app had focus: {:?}", desk.notices);

    // No window has it any more: the app is in the background.
    a.focus_moved(false);
    peers.deliver(&mut [&mut a, &mut b]);
    assert!(!a.transfers.watch.focused && !b.transfers.watch.focused);
}

#[test]
fn when_the_leading_window_closes_the_next_one_takes_the_badge_and_the_notifications() {
    let (mut a, peers) = first();
    // The job starts before the other windows open: they time it from then too.
    snapshot(&mut [&mut a], at_0, &[running_job()]);
    let mut b = second(&mut a, &peers);
    let mut c = second(&mut a, &peers);
    let mut desk = Desk::default();
    desk.take(&mut [&mut a, &mut b, &mut c]);
    snapshot(&mut [&mut a, &mut b, &mut c], at_0, &[running_job()]);
    desk.take(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(desk.badge(), 1);

    // a closes: its badge goes, b leads and shows it at once (no snapshot needed).
    a.window_closed();
    peers.deliver(&mut [&mut b, &mut c]);
    assert!(b.leads() && !c.leads());
    desk.take(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(desk.badges, BTreeMap::from([(a.window, 0), (b.window, 1)]));
    assert_eq!(desk.badge(), 1, "one badge, not a stale one from the closed window");

    // The job ends 20 s after it started: b notifies, once.
    snapshot(&mut [&mut b, &mut c], at_20s, &[done(&running_job())]);
    desk.take(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(desk.notices, [(b.window, "Downloaded big.iso".to_owned())]);
    assert_eq!(desk.badge(), 0);
}

#[test]
fn a_notification_click_shows_in_the_focused_window_else_the_leading_one() {
    let (mut a, peers) = first();
    let mut b = second(&mut a, &peers);
    assert_eq!((a.notice_window(), b.notice_window()), (Some(a.window), Some(a.window)));
    b.focus_moved(true);
    assert_eq!((a.notice_window(), b.notice_window()), (Some(b.window), Some(b.window)));
    b.focus_moved(false);
    a.window_closed();
    assert_eq!(b.notice_window(), Some(b.window));
}

#[test]
fn without_real_windows_new_window_does_nothing() {
    let mut a = fixture::app(fixture::demo());
    a.act(Action::NewWindow);
    assert_eq!(a.windows_open(), 1);
}
