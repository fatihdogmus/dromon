//! The Settings window (§6.8: a separate small window; §7: ⌘,) and how it shares the settings
//! with the main window.
//!
//! It is a second window of the same event loop, added with Rune's `App::window`: its own state
//! (`views::settings::Pane`), focus and frames, created when first opened, hidden (not
//! destroyed) when closed, and never keeping the app running alone. The two windows' states
//! talk through each other's `AppHandle`, on the UI thread, in call order:
//!
//! - main → pane: the app state's `settings_window` is a [`Link`]; `Action::OpenSettings` and
//!   every applied change hand it the app's [`SettingsView`], which replaces the pane's copy,
//!   so the window only ever shows the app's own values. Opening shows and focuses the one
//!   window (opening it again brings it forward).
//! - pane → main: each control's change goes to the main window as `Action::Setting`, where it
//!   applies, is remembered, and comes back as the above.
//!
//! The main window's app menu gets "Settings…" (⌘,) on macOS; on Windows and Linux Rune has no
//! app menu, and Ctrl+, reaches the main window's `on_key` instead.

use rune::{App as RuneApp, AppHandle, AppSpec, ColorScheme, MenuItem, Tx, WindowOptions, WindowSpec};

use crate::state::{Action, App, SettingsView, SettingsWindow};
use crate::theme::{self, Appearance};
use crate::views::{self, settings::Pane};

/// The window's key (`--window settings` selects it in a headless run).
pub const KEY: &str = "settings";

/// The real window, as the app state sees it.
struct Link {
    pane: AppHandle<Pane>,
    main: AppHandle<App>,
}

impl SettingsWindow for Link {
    fn open(&mut self, view: SettingsView) {
        self.show(view);
        self.pane.open();
        self.pane.focus();
    }

    fn show(&mut self, view: SettingsView) {
        // The windows' own frames (title bars, the macOS appearance) follow the theme too.
        self.main.set_color_scheme(match view.settings.theme {
            Appearance::System => ColorScheme::System,
            Appearance::Dark => ColorScheme::Dark,
            Appearance::Light => ColorScheme::Light,
        });
        self.pane.update(move |p, _| {
            p.view = view;
            p.error = None;
        });
    }
}

/// The window's app on `pane` (tests drive it with `headless_runtime`).
pub(crate) fn pane_app(pane: Pane) -> RuneApp<Pane> {
    let app = AppSpec {
        min_size: Some(views::settings::SIZE),
        fonts: theme::FONTS,
        families: theme::FAMILIES,
        on_key: Some(views::settings::on_key),
        ..AppSpec::new("Settings", views::settings::SIZE, || Pane::new(|_| {}), views::settings::view)
    };
    // Every control is a stop for Tab, whatever the system's keyboard navigation setting.
    RuneApp::new(app).init(move || pane).tab_visits_controls(true)
}

/// The window, sending its changes to `main`.
fn spec(main: AppHandle<App>) -> WindowSpec<Pane> {
    let pane = Pane::new(move |change| main.update(move |s, tx| views::run(s, Action::Setting(change), tx)));
    // Made when first opened; one fixed size.
    WindowSpec::from_app(KEY, pane_app(pane).windowless())
        .options(WindowOptions { resizable: false, max_size: Some(views::settings::SIZE), ..WindowOptions::default() })
}

/// Adds the Settings window and the app menu's "Settings…" to the main window's app. The app
/// state learns of the window once the app runs (a headless runtime never does: its state keeps
/// the default `NoWindow`).
pub fn attach(mut app: RuneApp<App>) -> RuneApp<App> {
    let main = app.handle();
    let (pane, _window) = app.window(spec(main.clone()));
    let link = Link { pane, main: main.clone() };
    main.update(move |s, _| s.attach_settings_window(Box::new(link)));
    let item = MenuItem::new("Settings…", open).id("settings");
    let item = match "mod+comma".parse() {
        Ok(chord) => item.key_equivalent(chord),
        Err(_) => item,
    };
    app.menu_item(item)
}

fn open(s: &mut App, tx: &mut Tx) {
    views::run(s, Action::OpenSettings, tx);
}
