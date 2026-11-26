//! Two main windows driven headless: the real app's view, handlers and `on_key` in two Rune
//! runtimes, whose states share the demo fixture's backend and the app-wide state as two real
//! windows do. A headless run opens no windows of its own, so the test plays the window system:
//! ⌘N's state goes to a second runtime, a window's close hook is called by hand, and a window
//! catches up on what the other's commands brought as its wake would make it.

use std::cell::RefCell;
use std::rc::Rc;

use dromon_core::HostId;
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::state::frames::Frame;
use crate::state::{App, News, Peers, WindowId};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

/// Peers that keep the windows ⌘N opens.
#[derive(Clone, Default)]
struct TestPeers(Rc<RefCell<Vec<App>>>);

impl Peers for TestPeers {
    fn can_open(&self) -> bool {
        true
    }
    fn open_window(&self, _: WindowId, app: App) {
        self.0.borrow_mut().push(app);
    }
    fn tell(&self, _: WindowId, _: News) {}
    fn frame(&self, _: WindowId) -> Option<Frame> {
        None
    }
    fn closed(&self, _: WindowId) {}
}

fn runtime(state: App) -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.state = state;
    rt.frame(0.0);
    rt
}

/// The first window on the demo, with test peers.
fn first() -> (Runtime<App>, TestPeers) {
    let peers = TestPeers::default();
    let mut s = scenes::base();
    s.set_peers(Rc::new(peers.clone()));
    (runtime(s), peers)
}

fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

fn key(rt: &mut Runtime<App>, k: Key, mods: Mods) {
    rt.key(KeyEvent { key: k, mods });
    rt.frame(1.0 / 60.0);
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn double_click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.05);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

/// What the other window's commands brought, as its wake applies it.
fn catch_up(rt: &mut Runtime<App>) {
    rt.update(|s, _| s.catch_up());
    rt.frame(1.0 / 60.0);
}

fn cwd(rt: &Runtime<App>) -> String {
    rt.state.conn().map(|c| c.cwd.to_string()).unwrap_or_default()
}

/// ⌘N in `rt`: the new window's runtime.
fn new_window(rt: &mut Runtime<App>, peers: &TestPeers) -> Runtime<App> {
    key(rt, Key::Command('n'), primary());
    let state = peers.0.borrow_mut().pop().expect("⌘N opened a window");
    runtime(state)
}

#[test]
fn cmd_n_opens_a_window_on_the_welcome_state_beside_a_connected_one() {
    let (mut one, peers) = first();
    click(&mut one, "host-prod");
    assert_eq!(cwd(&one), "/var/www/site");
    let mut two = new_window(&mut one, &peers);
    assert!(two.find("welcome").is_some(), "Pick a host");
    assert!(one.find("welcome").is_none(), "the first window keeps its host");
    assert!(two.find("host-prod").is_some(), "the same hosts");
    // The second window opens prod on the same connection, then goes elsewhere.
    click(&mut two, "host-prod");
    assert_eq!(cwd(&two), "/var/www/site");
    double_click(&mut two, "rows/releases");
    assert_eq!(cwd(&two), "/var/www/site/releases");
    catch_up(&mut one);
    assert_eq!(cwd(&one), "/var/www/site", "each window has its own folder");
    assert!(one.find("rows/index.html").is_some());
    // The first window navigates; the second stays.
    double_click(&mut one, "rows/assets");
    catch_up(&mut two);
    assert_eq!((cwd(&one).as_str(), cwd(&two).as_str()), ("/var/www/site/assets", "/var/www/site/releases"));
    assert_eq!(two.state.active, Some(HostId("prod".into())));
}

#[test]
fn cmd_w_closes_a_window_and_the_last_one_asks_while_transfers_run() {
    let (mut one, peers) = first();
    let mut two = new_window(&mut one, &peers);
    assert_eq!(one.state.windows_open(), 2);
    // ⌘W in the second: it closes (the close hook is the window system's to call).
    key(&mut two, Key::Command('w'), primary());
    assert!(!two.state.ui.confirm_quit);
    two.state.window_closed();
    assert_eq!(one.state.windows_open(), 1);
    // A running transfer: ⌘W in the last window asks first (the quit sheet).
    one.update(|s, _| s.apply(crate::state::ServiceEvent::Transfers(vec![running()])));
    key(&mut one, Key::Command('w'), primary());
    assert!(one.state.ui.confirm_quit);
    assert!(one.find("quit-explain").is_some(), "the quit sheet is up");
}

#[test]
fn cmd_n_works_while_a_sheet_is_up() {
    let (mut one, peers) = first();
    one.update(|s, _| {
        s.act(crate::state::Action::SelectHost(HostId("prod".into())));
        s.act(crate::state::Action::OpenLog);
    });
    one.frame(1.0 / 60.0);
    assert!(one.state.ui.sheets.is_open());
    let two = new_window(&mut one, &peers);
    assert!(!two.state.ui.sheets.is_open(), "the sheet is the first window's");
}

#[test]
fn each_window_shows_its_own_settings_page() {
    let (mut one, peers) = first();
    click(&mut one, "host-prod");
    let mut two = new_window(&mut one, &peers);
    key(&mut two, Key::Command(','), primary());
    assert!(two.find("settings").is_some(), "⌘, shows the page in the window it's pressed in");
    assert!(!one.state.on_settings() && one.find("settings").is_none(), "the other keeps its folder view");
    assert_eq!(cwd(&one), "/var/www/site");
    // A window opened from one showing Settings opens on its folder view.
    let three = new_window(&mut two, &peers);
    assert!(!three.state.on_settings() && three.find("welcome").is_some());
}

fn running() -> dromon_transfer::JobView {
    use dromon_transfer::{JobId, JobKind, JobView, Progress, TaskState};
    JobView {
        id: JobId(1),
        kind: JobKind::Download,
        host: HostId("prod".into()),
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
