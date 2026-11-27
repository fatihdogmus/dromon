// The app crate's root items, included by `lib.rs` (the library) and `hot.rs` (the live-editing
// binary, `dromon-hot`); the crate docs are in lib.rs.

mod fixture;
mod fmt;
// The icon set and the tokens are the spec's whole catalogue (§6.3–6.7), not all of it drawn yet.
// The icons' tests draw every icon, so their dead code shows in some builds only: `allow`.
#[allow(dead_code, reason = "the whole icon catalogue (§6.3–6.7); unused outside the in-crate tests")]
mod icons;
mod persist;
mod reveal;
mod scenes;
pub mod service;
pub mod state;
mod terminal;
#[expect(dead_code, reason = "the whole token catalogue (§6.3–6.7); not every token is drawn yet")]
mod theme;
mod views;
mod windows;

#[cfg(test)]
mod ui_a11y_tests;
#[cfg(test)]
mod ui_access_tests;
#[cfg(test)]
mod ui_file_tests;
#[cfg(test)]
mod ui_keyboard_tests;
#[cfg(test)]
mod ui_menu_tests;
#[cfg(test)]
mod ui_mouse_tests;
#[cfg(test)]
mod ui_palette_tests;
#[cfg(test)]
mod ui_poll_tests;
#[cfg(test)]
mod ui_preview_tests;
#[cfg(test)]
mod ui_properties_tests;
#[cfg(test)]
mod ui_quick_tests;
#[cfg(test)]
mod ui_remote_edit_tests;
#[cfg(test)]
mod ui_search_tests;
#[cfg(test)]
mod ui_settings_tests;
#[cfg(test)]
mod ui_sheet_tests;
#[cfg(test)]
mod ui_tests;
#[cfg(test)]
mod ui_transfer_tests;
#[cfg(test)]
mod ui_upload_tests;
#[cfg(test)]
mod ui_window_tests;

use rune::{App as RuneApp, AppSpec, KeyEvent, KeyPlatform, Tx, WindowSpec};

use state::{App, HostDirectory, Shortcut};

/// What the app opens at; also the sidebar's own minimum plus a usable list.
const SIZE: (f32, f32) = (1080.0, 680.0);
const MIN_SIZE: (f32, f32) = (720.0, 420.0);

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(fixture::DEMO_NOW, |d| d.as_secs() as i64)
}

/// The app on the active fixture (the demo unless `--fixture` chose one).
fn init() -> App {
    scenes::base()
}

/// Window keys nothing focused took: the list's shortcuts (§7).
fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(sc) = state::keys::shortcut(ev, KeyPlatform::current()) else { return false };
    // A sheet is modal: its own controls and Rune's modal handling (Escape) take the keys; ⌘N
    // and ⌘W are about windows, not this one's content.
    if s.ui.sheets.is_open() && !matches!(sc, Shortcut::NewWindow | Shortcut::CloseWindow) {
        return false;
    }
    // Typing while the filter field has focus is the field's.
    if matches!(sc, state::Shortcut::Type(_)) && tx.focused().is_some() {
        return false;
    }
    let now = tx.clock();
    let fx = s.shortcut(sc.clone(), now);
    // Shift+F10 and the Menu key the list has no menu for (a text field has the keyboard, the
    // folder isn't listed) are left to Rune: a field's own Cut, Copy and Paste menu.
    if sc == Shortcut::ContextMenu && fx.is_empty() {
        return false;
    }
    for fx in fx {
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
        // The done flourish's phases, on the window's clock; a preview's image decoded.
        poll: Some(views::poll),
        // Rune's waker, for a preview's image that finished decoding.
        setup: Some(views::setup),
        ..AppSpec::new("Dromon", SIZE, init, views::view)
    }
}

/// The app's first main window with its chrome: one 44 pt band for title bar and toolbar
/// (§6.2), its hooks and menus (`windows.rs`). Its state starts on the demo fixture; [`run`]
/// gives it the real service or `--fixture`'s instead. (⌘N's windows open only in a real run.)
pub fn app() -> RuneApp<App> {
    windows::first(&windows::Fleet::default())
}

/// Whether `DROMON_MOCK=1` asks for the built-in demo fixture.
fn mock_requested() -> bool {
    std::env::var_os("DROMON_MOCK").is_some_and(|v| v == "1")
}

/// The value of the app's own `--fixture PATH` (or `--fixture=PATH`) among the arguments Rune
/// left to the app. Rune's debug builds and its `tooling` feature take their own flags and hand
/// the rest on; a release build without it hands on everything, so this works in both.
fn fixture_arg(args: &[String]) -> Result<Option<String>, String> {
    value_arg(args, "--fixture", "the path of a fixture JSON file")
}

/// The value of the app's own `flag VALUE` (or `flag=VALUE`), once at most; `needs` says what
/// the value is when it's missing.
fn value_arg(args: &[String], flag: &str, needs: &str) -> Result<Option<String>, String> {
    let mut found = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let value = if a == flag {
            it.next().filter(|v| !v.is_empty()).cloned().ok_or_else(|| format!("{flag} needs {needs}"))?
        } else if let Some(v) = a.strip_prefix(flag).and_then(|v| v.strip_prefix('=')) {
            v.to_owned()
        } else {
            continue;
        };
        if found.replace(value).is_some() {
            return Err(format!("{flag} is given twice"));
        }
    }
    Ok(found)
}

/// `--accent a|b|c` paints with another accent candidate (§6.3) for comparing them
/// (`scripts/accents.sh`); without it, the default ([`theme::ACCENT`]).
fn choose_accent(args: &[String]) -> Result<(), String> {
    match value_arg(args, "--accent", "a, b or c")? {
        Some(id) => theme::choose_accent(&id).map(drop),
        None => Ok(()),
    }
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
    // The brand kit's app icon on every window and the taskbar on Windows, and on the windows
    // on Linux. macOS keeps the bundle's: Rune leaves the Dock alone until asked. The bytes are
    // ours, checked by a test (`views/brand_tests.rs`).
    #[cfg(not(target_os = "macos"))]
    {
        let set = rune::set_app_icon_choice(rune::AppIcon::Image(views::APP_ICON.to_vec()));
        debug_assert!(set.is_ok(), "the embedded app icon: {set:?}");
    }
    if let Err(msg) = choose_accent(rune::extra_args()) {
        eprintln!("dromon: {msg}");
        std::process::exit(2);
    }
    let fixture = match chosen_fixture() {
        Ok(fixture) => fixture,
        Err(msg) => {
            eprintln!("dromon: {msg}");
            std::process::exit(2);
        }
    };
    // The main windows: the first one now, more with ⌘N (`windows.rs`).
    let fleet = windows::Fleet::default();
    // The taskbar, the Dock badge and notifications, once for the whole app and before its
    // first window opens (not in a headless run).
    fleet.connect_os();
    if let Some(fx) = fixture {
        fixture::activate(fx.clone());
        let windows = fleet.clone();
        windows::first(&fleet)
            .init(move || {
                let mut app = fixture::app(fx);
                windows::join(&windows, &mut app);
                app
            })
            .run();
        return;
    }

    let (backend, commands) = service::channel();
    // Remembered settings, read before the UI starts. A headless run (`--snap`, `--tree`)
    // neither reads nor writes them, so its output doesn't depend on them.
    let remembered = rune::tooling::headless_run().is_none().then(|| {
        let prefs = rune::prefs::Prefs::open(persist::APP_ID);
        (persist::load(&prefs), prefs)
    });
    // Only the first window reopens, where it was; nothing reconnects (§12).
    let frame = remembered.as_ref().and_then(|(saved, _)| saved.frames.first().copied());
    let windows = fleet.clone();
    let app = windows::first(&fleet).init(move || {
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
        windows::join(&windows, &mut app);
        app
    });
    let mut pending = Some((commands, fleet.sink()));
    let app = app.on_ready(move |_| {
        // Once, after the first frame: start the network side, for every main window.
        if let Some((commands, sink)) = pending.take()
            && let Err(e) = service::start(commands, sink, service::Config::for_app())
        {
            eprintln!("dromon: couldn't start the service thread: {e}");
        }
    });
    // Quitting (`windows.rs`, `App::quitting`): each window's end of the channel goes, which
    // stops the service (connections close, secrets are wiped), and the settings are written.
    match frame {
        // The displays are known only once the event loop runs: the window fits on one of them.
        Some(frame) => rune::run_windows(move |displays| {
            [WindowSpec::from_app("main", windows::at_frame(app, Some(frame), displays))]
        }),
        None => app.run(),
    }
}
