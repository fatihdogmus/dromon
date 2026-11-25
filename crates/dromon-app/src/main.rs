//! Dromon's binary: the Rune UI, or the askpass helper when ssh invokes us with
//! `DROMON_ASKPASS_ENDPOINT` set (not yet). For now the UI runs against `mock`'s hosts.

mod fmt;
// The icon set and the tokens are the spec's whole catalogue (§6.3–6.7); sheets, popovers and
// transfers use the rest in later milestones.
#[allow(dead_code)]
mod icons;
mod mock;
mod scenes;
mod state;
#[allow(dead_code)]
mod theme;
mod views;

#[cfg(test)]
mod ui_tests;

use rune::{App as RuneApp, AppSpec, KeyEvent, KeyPlatform, Titlebar, Tx};

use state::App;

/// What the app opens at; also the sidebar's own minimum plus a usable list.
const SIZE: (f32, f32) = (1080.0, 680.0);
const MIN_SIZE: (f32, f32) = (720.0, 420.0);

fn init() -> App {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(mock::NOW, |d| d.as_secs() as i64);
    App::new(mock::hosts(), now, Box::new(mock::MockService { now }))
}

/// Window keys nothing focused took: the list's shortcuts (§7).
fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
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

/// The app with its window chrome: one 44 pt band for title bar and toolbar (§6.2).
fn app() -> RuneApp<App> {
    RuneApp::new(spec()).titlebar(Titlebar::new(theme::size::TOOLBAR))
}

fn main() {
    rune::set_app_id("dromon");
    app().run();
}
