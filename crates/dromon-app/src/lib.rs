//! Dromon's app: the Rune UI, its state and the service thread. The `dromon` binary
//! (`main.rs`) runs the askpass helper when ssh starts it as one, and [`run`] otherwise.
//!
//! The UI talks to the service thread (`service`), which discovers hosts from the user's ssh
//! config and connects for real. `--fixture <file.json>` runs it on a fixture instead (`fixture`,
//! `docs/fixtures.md`): hosts, folders, errors and sign-in prompts from the file, no network.
//! `DROMON_MOCK=1` is the same with the built-in demo (`assets/fixtures/demo.json`), which
//! scenes and the in-crate tests always use.
//!
//! The public surface is what the end-to-end tests (`tests/e2e.rs`) drive the app with:
//!
//! - [`app`]: the real app (view, handlers, `on_key`, window chrome), for
//!   `app().headless_runtime(..)`; its state starts on the demo fixture, which a test replaces.
//! - [`state`]: `App`, `Action`, `ServiceEvent`, the sheets, and everything a test reads back.
//! - [`service`]: the service thread (`start`, `channel`, `Config`, `Sink`).
//! - [`run`]: what the binary does after the helper check.

mod fixture;
mod fmt;
// The icon set and the tokens are the spec's whole catalogue (§6.3–6.7); sheets, popovers and
// transfers use the rest in later milestones.
#[allow(dead_code)]
mod icons;
mod persist;
mod reveal;
mod scenes;
pub mod service;
pub mod state;
#[allow(dead_code)]
mod theme;
mod views;

#[cfg(test)]
mod ui_file_tests;
#[cfg(test)]
mod ui_menu_tests;
#[cfg(test)]
mod ui_sheet_tests;
#[cfg(test)]
mod ui_tests;
#[cfg(test)]
mod ui_transfer_tests;

use rune::{App as RuneApp, AppSpec, KeyEvent, KeyPlatform, Titlebar, Tx};

use state::{App, HostDirectory};

/// What the app opens at; also the sidebar's own minimum plus a usable list.
const SIZE: (f32, f32) = (1080.0, 680.0);
const MIN_SIZE: (f32, f32) = (720.0, 420.0);

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(fixture::DEMO_NOW, |d| d.as_secs() as i64)
}

/// The app on the active fixture (the demo unless `--fixture` chose one).
fn init() -> App {
    scenes::base()
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
/// starts on the demo fixture; [`run`] gives it the real service or `--fixture`'s instead.
pub fn app() -> RuneApp<App> {
    RuneApp::new(spec())
        .titlebar(Titlebar::new(theme::size::TOOLBAR))
        // Closing the window with transfers running asks first (the quit sheet).
        .on_close_request(|s, _tx| if s.close_requested() { rune::CloseResponse::Close } else { rune::CloseResponse::Defer })
}

/// Whether `DROMON_MOCK=1` asks for the built-in demo fixture.
fn mock_requested() -> bool {
    std::env::var_os("DROMON_MOCK").is_some_and(|v| v == "1")
}

/// The value of the app's own `--fixture PATH` (or `--fixture=PATH`) among the arguments Rune
/// left to the app. Rune's debug builds and its `tooling` feature take their own flags and hand
/// the rest on; a release build without it hands on everything, so this works in both.
fn fixture_arg(args: &[String]) -> Result<Option<String>, String> {
    let mut found = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let value = if a == "--fixture" {
            it.next().filter(|v| !v.is_empty()).cloned().ok_or("--fixture needs the path of a fixture JSON file")?
        } else if let Some(v) = a.strip_prefix("--fixture=") {
            v.to_owned()
        } else {
            continue;
        };
        if found.replace(value).is_some() {
            return Err("--fixture is given twice".into());
        }
    }
    Ok(found)
}

/// The fixture to run on: `--fixture`'s, else the demo with `DROMON_MOCK=1`, else none (the
/// real service). A fixture that can't be read or used is the caller's to report.
fn chosen_fixture() -> Result<Option<std::sync::Arc<fixture::Fixture>>, String> {
    match fixture_arg(rune::extra_args())? {
        Some(path) => fixture::load(&path).map(|f| Some(std::sync::Arc::new(f))),
        None if mock_requested() => Ok(Some(fixture::demo())),
        None => Ok(None),
    }
}

/// Runs the app: a window (or Rune's headless tooling, `--snap` and friends) on the real
/// service, or on a fixture (`--fixture`, `DROMON_MOCK=1`): no service thread, no ssh, no ssh
/// config. An unusable fixture exits with status 2 and a message naming where it's wrong.
/// Returns when the app quits.
pub fn run() {
    rune::set_app_id("dromon");
    match chosen_fixture() {
        Ok(Some(fx)) => {
            fixture::activate(fx.clone());
            app().init(move || fixture::app(fx)).run();
            return;
        }
        Ok(None) => {}
        Err(msg) => {
            eprintln!("dromon: {msg}");
            std::process::exit(2);
        }
    }

    let (backend, commands) = service::channel();
    // Remembered settings, read before the UI starts. A headless run (`--snap`, `--tree`)
    // neither reads nor writes them, so its output doesn't depend on them.
    let remembered = rune::tooling::headless_run().is_none().then(|| {
        let prefs = rune::prefs::Prefs::open(persist::APP_ID);
        (persist::load(&prefs), prefs)
    });
    let app = app().init(move || {
        // A headless run never reaches `on_ready`, so it reads the config here; no window
        // waits on it. A window gets its hosts from the service.
        let hosts = match rune::tooling::headless_run() {
            Some(_) => {
                let src = dromon_ssh::ConfigSources::default_for_platform();
                HostDirectory { hosts: service::hosts::aliases(&src) }
            }
            None => HostDirectory::default(),
        };
        let mut app = App::new(hosts, now_secs(), Box::new(backend));
        // The user's Downloads folder (XDG_DOWNLOAD_DIR on Linux), else the home folder.
        let dirs = rune::dirs::AppDirs::new(persist::APP_ID);
        if let Some(dir) = dirs.as_ref().and_then(|d| d.downloads().or(d.home())) {
            app.downloads = dir.to_path_buf();
        }
        if let Some((saved, prefs)) = remembered {
            app.remember(saved, Box::new(persist::PrefsWriter::spawn(prefs)));
        }
        app
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
        // Dropping the settings writer waits for its last write.
        drop(s.take_store());
    })
    .run();
}
