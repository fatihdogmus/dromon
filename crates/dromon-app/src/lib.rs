//! Dromon's app: the Rune UI, its state and the service thread. The `dromon` binary
//! (`main.rs`) runs the askpass helper when ssh starts it as one, and [`run`] otherwise.
//!
//! The UI talks to the service thread (`service`), which discovers hosts from the user's ssh
//! config and connects for real. `DROMON_MOCK=1` runs it against `mock`'s hosts and folders
//! instead (demos, the goldens script); scenes and tests always use the mock.
//!
//! The public surface is what the end-to-end tests (`tests/e2e.rs`) drive the app with:
//!
//! - [`app`]: the real app (view, handlers, `on_key`, window chrome), for
//!   `app().headless_runtime(..)`; its state starts as the mock's, which a test replaces.
//! - [`state`]: `App`, `Action`, `ServiceEvent`, the sheets, and everything a test reads back.
//! - [`service`]: the service thread (`start`, `channel`, `Config`, `Sink`).
//! - [`run`]: what the binary does after the helper check.

mod fmt;
// The icon set and the tokens are the spec's whole catalogue (§6.3–6.7); sheets, popovers and
// transfers use the rest in later milestones.
#[allow(dead_code)]
mod icons;
mod mock;
mod scenes;
pub mod service;
pub mod state;
#[allow(dead_code)]
mod theme;
mod views;

#[cfg(test)]
mod ui_sheet_tests;
#[cfg(test)]
mod ui_tests;

use rune::{App as RuneApp, AppSpec, KeyEvent, KeyPlatform, Titlebar, Tx};

use state::{App, HostDirectory};

/// What the app opens at; also the sidebar's own minimum plus a usable list.
const SIZE: (f32, f32) = (1080.0, 680.0);
const MIN_SIZE: (f32, f32) = (720.0, 420.0);

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(mock::NOW, |d| d.as_secs() as i64)
}

/// The mock app: what scenes, tests and `DROMON_MOCK=1` start from.
fn init() -> App {
    let now = now_secs();
    App::new(mock::hosts(), now, Box::new(mock::MockService { now }))
}

/// Window keys nothing focused took: the list's shortcuts (§7).
fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // A sheet is modal: its own controls and Rune's modal handling (Escape) take the keys.
    if s.ui.sheets.is_open() {
        return false;
    }
    let Some(sc) = state::keys::shortcut(ev, KeyPlatform::current()) else { return false };
    // Typing while the filter field has focus is the field's.
    if matches!(sc, state::Shortcut::Type(_)) && tx.focused().is_some() {
        return false;
    }
    let now = tx.clock();
    for fx in s.shortcut(sc, now) {
        views::effect(fx, tx);
    }
    true
}

fn spec() -> AppSpec<App> {
    AppSpec {
        min_size: Some(MIN_SIZE),
        scenes: scenes::SCENES,
        fonts: theme::FONTS,
        families: theme::FAMILIES,
        on_key: Some(on_key),
        ..AppSpec::new("Dromon", SIZE, init, views::view)
    }
}

/// The app with its window chrome: one 44 pt band for title bar and toolbar (§6.2). Its state
/// starts as the mock app; [`run`] gives it the real service instead.
pub fn app() -> RuneApp<App> {
    RuneApp::new(spec()).titlebar(Titlebar::new(theme::size::TOOLBAR))
}

/// Whether `DROMON_MOCK=1` asks for the mock backend.
fn mock_requested() -> bool {
    std::env::var_os("DROMON_MOCK").is_some_and(|v| v == "1")
}

/// Runs the app: a window (or Rune's headless tooling, `--snap` and friends) on the real
/// service, or on the mock with `DROMON_MOCK=1`. Returns when the app quits.
pub fn run() {
    rune::set_app_id("dromon");
    if mock_requested() {
        app().run();
        return;
    }

    let (backend, commands) = service::channel();
    let app = app().init(move || {
        // A headless run (`--snap`, `--tree`) never reaches `on_ready`, so it reads the
        // config here; no window waits on it. A window gets its hosts from the service.
        let hosts = match rune::tooling::headless_run() {
            Some(_) => {
                let src = dromon_ssh::ConfigSources::default_for_platform();
                HostDirectory { hosts: service::hosts::aliases(&src) }
            }
            None => HostDirectory::default(),
        };
        App::new(hosts, now_secs(), Box::new(backend))
    });
    let handle = app.handle();
    let mut pending = Some((commands, handle));
    app.on_ready(move |_| {
        // Once, after the first frame: start the network side.
        if let Some((commands, handle)) = pending.take()
            && let Err(e) = service::start(commands, service::RuneSink(handle), service::Config::for_app())
        {
            eprintln!("dromon: couldn't start the service thread: {e}");
        }
    })
    .on_quit(|s, _tx| {
        // Dropping the channel stops the service: connections close and secrets are wiped.
        s.backend = Box::new(state::Silent::default());
    })
    .run();
}
