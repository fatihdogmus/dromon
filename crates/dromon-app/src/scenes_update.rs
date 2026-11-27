//! Scenes of updates: a newer release found over the listing (the toast), the Settings page on
//! General with it, and "Update anyway?" with transfers running. The offered version is fixed and
//! no text shows the running one, so the pictures don't change with each release.

use crate::state::update::{Method, Offer, Package, UpdateAction, UpdateEvent, Version};
use crate::state::{Action, App, ServiceEvent, SettingsSection};

/// The service's news of 9.9.0, which this copy installs itself.
fn offered(s: &mut App) {
    let version = Version::parse("9.9.0").expect("a version");
    let page = format!("https://github.com/fatihdogmus/dromon/releases/tag/{version}");
    let offer = Offer { version, page, method: Method::Install(Package::Dmg) };
    s.apply(ServiceEvent::Update(UpdateEvent::Available(offer)));
}

/// The listing with the toast: "Dromon 9.9.0 is available", Update, Later, Release notes.
pub fn update_toast(s: &mut App) {
    super::listing(s);
    offered(s);
}

/// The Settings page on General with 9.9.0 found and its toast put off (Later): the switch,
/// Check now, the status line and its Update and Release notes buttons.
pub fn settings_general(s: &mut App) {
    super::settings::settings(s);
    offered(s);
    s.act(Action::Update(UpdateAction::Later));
    s.act(Action::SettingsSection(SettingsSection::General));
}

/// Update with two transfers running: the sheet asks first.
pub fn update_confirm(s: &mut App) {
    super::toolbar_ring(s);
    offered(s);
    s.act(Action::Update(UpdateAction::Update));
}
