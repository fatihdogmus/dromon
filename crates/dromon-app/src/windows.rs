//! The main windows in Rune (§5 M4 "⌘N new window", §7, §4.7 "window frames"): what every
//! main window's app has, opening another for ⌘N where it cascades from its opener, and the
//! frames they're remembered at. The state side is `state/windows.rs`.
//!
//! [`Fleet`] is the app's set of main windows: each one's `AppHandle`, which the service's
//! events go to (`service::RuneSink`), and its `WindowHandle`, which says where it is. Each
//! window's state reaches the others through it as its `Peers`.
//!
//! The File menu (macOS) has New Window (⌘N) and Close Window (⌘W); their keys reach the
//! window first (`keys_first`), so on every platform `on_key` takes them. Closing the last main
//! window quits the app, on macOS too. The app menu's "Settings…" (⌘,, macOS) acts in the key
//! window, as Rune's menu bar holds the key window's items: it shows that window's Settings page
//! (`state/page.rs`). On Windows and Linux Rune has no app menu, and Ctrl+, reaches `on_key`.
//!
//! The taskbar, the Dock badge and notifications are hooked up once for the app
//! ([`Fleet::connect_os`]); every main window's native window joins that link, and only the
//! leading window's state (`App::leads`) sends anything through it.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use rune::{
    App as RuneApp, AppHandle, ColorScheme, Display, MenuItem, Titlebar, Tx, WindowHandle, WindowOptions,
    WindowPlacement, WindowSpec,
};

use crate::service::RuneSink;
use crate::state::frames::{self, Area, CASCADE, Frame};
use crate::state::{Action, App, News, Peers, WindowId};
use crate::theme::Appearance;
use crate::views::OsLink;
use crate::{theme, views};

/// The app's main windows.
#[derive(Clone, Default)]
pub struct Fleet {
    sink: RuneSink,
    windows: Arc<Mutex<BTreeMap<WindowId, WindowHandle>>>,
    /// The app's way to the taskbar, the Dock and notifications, once it's hooked up to them
    /// ([`Fleet::connect_os`]); each main window's native window gets it.
    os: Arc<OnceLock<OsLink>>,
}

impl Fleet {
    /// Hooks the app up to the taskbar, the Dock badge and notifications, once per run and
    /// before its first window opens (`views::connect_os`). Not in a headless run.
    pub fn connect_os(&self) {
        if self.os.get().is_none()
            && let Some(link) = views::connect_os(self.sink())
        {
            let _ = self.os.set(link);
        }
    }

    /// Where the service's events go: every main window.
    pub fn sink(&self) -> RuneSink {
        self.sink.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<WindowId, WindowHandle>> {
        self.windows.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn handle(&self, id: WindowId) -> Option<WindowHandle> {
        self.lock().get(&id).cloned()
    }

    /// The first main window still open.
    pub fn first(&self) -> Option<AppHandle<App>> {
        self.sink.handles().into_iter().next().map(|(_, h)| h)
    }

    /// Window `id` catches up on what its backend holds for it (a fixture's `pending`).
    pub fn wake(&self, id: WindowId) {
        if let Some((_, h)) = self.sink.handles().into_iter().find(|(w, _)| *w == id) {
            h.update_if(|s, tx| {
                s.catch_up();
                if s.take_animate() {
                    tx.animated();
                }
                true
            });
        }
    }
}

impl Peers for Fleet {
    fn can_open(&self) -> bool {
        !rune::is_headless()
    }

    fn open_window(&self, from: WindowId, app: App) {
        open(self, from, app);
    }

    fn tell(&self, from: WindowId, news: News) {
        for (id, h) in self.sink.handles() {
            if id != from {
                let news = news.clone();
                h.update(move |s, _| s.hear(news));
            }
        }
    }

    fn frame(&self, window: WindowId) -> Option<Frame> {
        self.handle(window)?.placement().map(frame_of)
    }

    fn closed(&self, window: WindowId) {
        self.sink.remove(window);
        self.lock().remove(&window);
    }

    /// The app's color scheme, which every window without one of its own follows (Rune keeps
    /// it for windows made later; nothing in a headless run).
    fn theme(&self, theme: Appearance) {
        if let Some(window) = self.first() {
            window.set_color_scheme(match theme {
                Appearance::System => ColorScheme::System,
                Appearance::Dark => ColorScheme::Dark,
                Appearance::Light => ColorScheme::Light,
            });
        }
    }
}

/// A placement as a frame to reopen at: a maximized window's restored frame where the system
/// keeps one (Windows), else the frame it has.
fn frame_of(p: WindowPlacement) -> Frame {
    let ((x, y), (width, height)) = match p.restore {
        Some(restore) if p.maximized => restore,
        _ => (p.position, p.content_size),
    };
    Frame { x, y, width, height, maximized: p.maximized }
}

/// The displays' usable areas, the primary first.
pub fn areas(displays: &[Display]) -> Vec<Area> {
    let mut sorted: Vec<&Display> = displays.iter().collect();
    sorted.sort_by_key(|d| !d.primary);
    sorted
        .into_iter()
        .map(|d| {
            let ((x, y), (width, height)) = d.work_area();
            Area { x, y, width, height }
        })
        .collect()
}

/// The first main window's app, opening at `frame` (fitted to `displays`) when one is
/// remembered.
pub fn at_frame(app: RuneApp<App>, frame: Option<Frame>, displays: &[Display]) -> RuneApp<App> {
    match frame.and_then(|f| frames::fit(f, &areas(displays))) {
        Some(f) => app.window_options(options_at(f)).maximized(f.maximized),
        None => app,
    }
}

fn options_at(f: Frame) -> WindowOptions {
    WindowOptions { position: Some((f.x, f.y)), physical_size: Some((f.width, f.height)), ..WindowOptions::default() }
}

/// The first main window's app.
pub fn first(fleet: &Fleet) -> RuneApp<App> {
    main_window(fleet, WindowId::FIRST, RuneApp::new(crate::spec()))
}

/// What every main window's app has: one 44 pt band for title bar and toolbar (§6.2), the
/// close and quit hooks, polling by visibility, the File and app menu items, its place among
/// the fleet's windows, and its taskbar button and notifications (which only the leading
/// window uses, `App::leads`).
fn main_window(fleet: &Fleet, id: WindowId, app: RuneApp<App>) -> RuneApp<App> {
    let handle = app.handle();
    fleet.sink.add(id, handle.clone());
    let windows = fleet.clone();
    app.titlebar(Titlebar::new(theme::size::TOOLBAR))
        // Closing the last window with transfers running asks first (the quit sheet).
        .on_close_request(
            |s, _tx| if s.close_requested() { rune::CloseResponse::Close } else { rune::CloseResponse::Defer },
        )
        .on_close(|s, _tx| s.window_closed())
        .on_quit(|s, _tx| s.quitting())
        // Nothing polls while the window is off screen (§4.5).
        .on_visibility(views::window_visibility)
        // Each new native window (again after a GPU release): where it is, and the app's way
        // to the taskbar and notifications with what the taskbar showed.
        .on_window(move |window| {
            windows.lock().insert(id, window.clone());
            if let Some(os) = windows.os.get() {
                let (window, os) = (window.clone(), os.clone());
                handle.update_if(move |s, _| {
                    views::connect_window(s, window, os);
                    false
                });
            }
        })
        .menu_item(menu_item("New Window", "new-window", "mod+n", |s, tx| views::run(s, Action::NewWindow, tx)))
        .menu_item(menu_item("Close Window", "close-window", "mod+w", |s, tx| views::run(s, Action::CloseWindow, tx)))
        .menu_item(settings_item())
}

fn menu_item(title: &str, id: &str, chord: &str, action: fn(&mut App, &mut Tx)) -> MenuItem<App> {
    let item = MenuItem::new(title, action).id(id).menu("File").keys_first();
    match chord.parse() {
        Ok(chord) => item.key_equivalent(chord),
        Err(_) => item,
    }
}

/// The app menu's "Settings…" (⌘,): the Settings page in the window it acts in.
fn settings_item() -> MenuItem<App> {
    let item = MenuItem::new("Settings…", open_settings).id("settings");
    match "mod+comma".parse() {
        Ok(chord) => item.key_equivalent(chord),
        Err(_) => item,
    }
}

fn open_settings(s: &mut App, tx: &mut Tx) {
    views::run(s, Action::OpenSettings, tx);
}

/// The fleet as a window state's peers, and its backend waking the windows (a fixture's); the
/// windows' frames take the theme it starts with. Outside a headless run, its Settings page
/// writes folders under the home folder as `~/…`.
pub fn join(fleet: &Fleet, app: &mut App) {
    app.set_peers(Rc::new(fleet.clone()));
    fleet.theme(app.settings.theme);
    if rune::tooling::headless_run().is_none() {
        app.ui.settings_page.home = rune::dirs::home();
    }
    let waker = fleet.clone();
    app.backend.set_wake(Box::new(move |w| waker.wake(w)));
}

/// Opens a main window on `app` (a new window's state, which shares its opener's peers), a
/// step right and down from `from`.
fn open(fleet: &Fleet, from: WindowId, app: App) {
    let id = app.window;
    let scale = fleet.handle(from).and_then(|h| h.scale()).unwrap_or(1.0);
    let step = (CASCADE * scale).round() as i32;
    let at = fleet.frame(from).map(|f| frames::cascade(f, step, &areas(&rune::displays())));
    let mut window = main_window(fleet, id, RuneApp::new(crate::spec())).init(move || app);
    if let Some(f) = at {
        window = window.window_options(options_at(f));
    }
    rune::open_window(WindowSpec::from_app(format!("main-{}", id.0 + 1), window));
}
