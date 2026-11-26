//! The Settings window (§6.8: a separate small window; §7: ⌘,) and how it shares the settings
//! with the main window.
//!
//! It is a second window of the same event loop, added with Rune's `App::window`: its own state
//! (`views::settings::Pane`), focus and frames, created when first opened, hidden (not
//! destroyed) when closed, and never keeping the app running alone. The two windows' states
//! talk through each other's `AppHandle`, on the UI thread, in call order:
//!
//! - main → pane: each main window's state has a [`Link`] as its `settings_window`;
//!   `Action::OpenSettings` and every applied change hand it the app's [`SettingsView`], which
//!   replaces the pane's copy, so the window only ever shows the app's own values. Opening
//!   shows and focuses the one window (opening it again brings it forward).
//! - pane → main: each control's change goes to the first main window open as
//!   `Action::Setting`, where it applies, is remembered, comes back as the above, and goes on to
//!   the other main windows (`News::Settings`).
//!
//! Every main window's app menu gets "Settings…" (⌘,) on macOS; on Windows and Linux Rune has
//! no app menu, and Ctrl+, reaches the window's `on_key` instead.

use rune::{App as RuneApp, AppHandle, AppSpec, ColorScheme, MenuItem, Tx, WindowOptions, WindowSpec};

use crate::state::{Action, App, SettingsView, SettingsWindow};
use crate::theme::{self, Appearance};
use crate::views::{self, settings::Pane};
use crate::windows::Fleet;

/// The window's key (`--window settings` selects it in a headless run).
pub const KEY: &str = "settings";

/// The real window, as a main window's state sees it.
#[derive(Clone)]
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

    fn another(&self) -> Box<dyn SettingsWindow> {
        Box::new(self.clone())
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

/// The window, sending its changes to the first main window open.
fn spec(fleet: Fleet) -> WindowSpec<Pane> {
    let pane = Pane::new(move |change| {
        if let Some(main) = fleet.first() {
            main.update(move |s, tx| views::run(s, Action::Setting(change), tx));
        }
    });
    // Made when first opened; one fixed size.
    WindowSpec::from_app(KEY, pane_app(pane).windowless())
        .options(WindowOptions { resizable: false, max_size: Some(views::settings::SIZE), ..WindowOptions::default() })
}

/// Adds the Settings window to the first main window's app (it lasts as long as the app;
/// later main windows' states get the same link). The app state learns of the window once the
/// app runs (a headless runtime never does: its state keeps the default `NoWindow`).
pub fn attach(mut app: RuneApp<App>, fleet: &Fleet) -> RuneApp<App> {
    let main = app.handle();
    let (pane, _window) = app.window(spec(fleet.clone()));
    let link = Link { pane, main: main.clone() };
    main.update(move |s, _| s.attach_settings_window(Box::new(link)));
    app
}

/// The app menu's "Settings…" (⌘,), in every main window's app.
pub fn menu_item() -> MenuItem<App> {
    let item = MenuItem::new("Settings…", open).id("settings");
    match "mod+comma".parse() {
        Ok(chord) => item.key_equivalent(chord),
        Err(_) => item,
    }
}

fn open(s: &mut App, tx: &mut Tx) {
    views::run(s, Action::OpenSettings, tx);
}
