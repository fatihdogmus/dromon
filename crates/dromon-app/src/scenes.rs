//! Named states for headless snapshots (`--scene NAME`), each reached by the same actions and
//! events a user and the service would produce, on the mock clock.

use dromon_core::{ConnStatus, HostId, RemotePath};
use rune::TextEdit;

use crate::mock::{self, MockService};
use crate::state::{Action, App, Effect, ServiceEvent, Silent};
use crate::theme::Appearance;

/// The app as it launches against the mock: hosts listed, nothing connected.
pub fn base() -> App {
    App::new(mock::hosts(), mock::NOW, Box::new(MockService { now: mock::NOW }))
}

fn prod() -> HostId {
    HostId("prod".into())
}

fn act(s: &mut App, a: Action) -> Vec<Effect> {
    s.act(a)
}

pub fn welcome(s: &mut App) {
    *s = base();
}

pub fn connecting(s: &mut App) {
    *s = base();
    s.backend = Box::new(Silent::default());
    act(s, Action::SelectHost(prod()));
}

pub fn listing(s: &mut App) {
    *s = base();
    // A second live connection, so Connected holds two.
    act(s, Action::SelectHost(HostId("nas".into())));
    act(s, Action::SelectHost(prod()));
    act(s, Action::Click { name: "index.html".into(), extend: false, toggle: false });
}

pub fn listing_sorted_size(s: &mut App) {
    listing(s);
    act(s, Action::Sort(dromon_core::SortKey::Size));
}

pub fn big_folder(s: &mut App) {
    *s = base();
    act(s, Action::SelectHost(prod()));
    act(s, Action::GoTo(RemotePath::new(mock::BIG_FOLDER)));
    let name = "IMG_05000.jpg".to_owned();
    act(s, Action::Click { name: name.clone(), extend: false, toggle: false });
    s.ui.center_on_mount = Some(name);
}

pub fn empty_folder(s: &mut App) {
    *s = base();
    act(s, Action::SelectHost(prod()));
    act(s, Action::Open("uploads".into()));
}

pub fn permission_denied(s: &mut App) {
    *s = base();
    act(s, Action::SelectHost(prod()));
    act(s, Action::GoTo(RemotePath::new("/root")));
}

pub fn reconnecting(s: &mut App) {
    listing(s);
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 2 } });
}

pub fn filter(s: &mut App) {
    listing(s);
    act(s, Action::OpenFilter);
    s.ui.filter_edit = TextEdit::new("se");
    s.ui.filter_edit.focused = true;
    act(s, Action::FilterChanged("se".into()));
}

pub fn light(s: &mut App) {
    listing(s);
    s.ui.appearance = Appearance::Light;
}

/// A named state: what `AppSpec::scenes` takes.
pub type Scene = (&'static str, fn(&mut App));

/// Every scene, for `AppSpec::scenes`; names are fixed (goldens are named after them).
pub const SCENES: &[Scene] = &[
    ("welcome", welcome),
    ("connecting", connecting),
    ("listing", listing),
    ("listing-sorted-size", listing_sorted_size),
    ("big-folder", big_folder),
    ("empty-folder", empty_folder),
    ("permission-denied", permission_denied),
    ("reconnecting", reconnecting),
    ("filter", filter),
    ("light", light),
];
