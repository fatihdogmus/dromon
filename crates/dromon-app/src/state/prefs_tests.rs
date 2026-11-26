//! Remembered settings: what changes save, and what a new launch starts from.

use std::cell::RefCell;
use std::rc::Rc;

use dromon_core::{Error, HostId, RemotePath, SortDir, SortKey};

use super::*;
use crate::fixture;
use crate::state::{Action, App, Command, Listing, ServiceEvent, Silent, Sort};
use crate::theme::size;

/// A store that keeps every snapshot it was handed.
#[derive(Clone, Default)]
struct Recorded(Rc<RefCell<Vec<Saved>>>);

impl Store for Recorded {
    fn save(&mut self, saved: &Saved) {
        self.0.borrow_mut().push(saved.clone());
    }
}

impl Recorded {
    fn last(&self) -> Saved {
        self.0.borrow().last().cloned().expect("something was saved")
    }
    fn count(&self) -> usize {
        self.0.borrow().len()
    }
}

fn prod() -> HostId {
    HostId("prod".into())
}

fn app_with(saved: Saved) -> (App, Recorded) {
    let mut s = fixture::app(fixture::demo());
    let store = Recorded::default();
    s.remember(saved, Box::new(store.clone()));
    (s, store)
}

#[test]
fn a_new_launch_starts_from_what_was_saved() {
    let saved = Saved {
        sort: Some(Sort { key: SortKey::Size, dir: SortDir::Descending }),
        show_hidden: Some(false),
        sidebar_width: Some(9_999.0),
        ..Saved::default()
    };
    let (mut s, store) = app_with(saved);
    assert_eq!(s.sort, Sort { key: SortKey::Size, dir: SortDir::Descending });
    assert!(!s.show_hidden_default);
    // An out-of-range width from the file is clamped, not trusted.
    assert_eq!(s.ui.sidebar_width, size::SIDEBAR_MAX);
    s.act(Action::SelectHost(prod()));
    let c = s.conn().unwrap();
    assert_eq!(c.sort.key, SortKey::Size);
    assert!(!c.show_hidden);
    // Starting from saved settings saves nothing by itself, except the host opened and the
    // folder now on screen.
    assert_eq!(store.count(), 2);
    assert_eq!(store.last().folder("prod"), Some("/var/www/site"));
    assert_eq!(store.last().last_host.as_deref(), Some("prod"));
}

#[test]
fn the_last_host_is_offered_at_launch_and_never_connected_by_itself() {
    let (mut s, store) = app_with(Saved::default());
    assert_eq!(s.last_host(), None, "nothing opened yet");
    s.act(Action::SelectHost(prod()));
    assert_eq!(store.last().last_host.as_deref(), Some("prod"));

    // The next launch: the welcome screen offers prod, and nothing connects (§12).
    let (mut next, _) = app_with(store.last());
    assert_eq!(next.active, None);
    assert!(next.conns.is_empty(), "never auto-connects");
    assert_eq!(next.last_host(), Some(prod()));
    next.act(Action::SelectHost(prod()));
    assert_eq!(next.active, Some(prod()));

    // A host the sidebar no longer has isn't offered.
    let (gone, _) = app_with(Saved { last_host: Some("retired".into()), ..Saved::default() });
    assert_eq!(gone.last_host(), None);
}

#[test]
fn sort_and_sidebar_changes_are_saved_and_the_hidden_toggle_is_for_the_session() {
    let (mut s, store) = app_with(Saved::default());
    s.act(Action::Sort(SortKey::Modified));
    assert_eq!(store.last().sort, Some(Sort { key: SortKey::Modified, dir: SortDir::Descending }));
    // ⌘⇧. switches this session only: Settings holds the remembered default (`settings.rs`).
    let saves = store.count();
    s.act(Action::ToggleHidden);
    assert!(!s.show_hidden_default);
    assert_eq!(store.count(), saves);
    s.ui.sidebar_width = 300.0;
    s.sidebar_resized();
    assert_eq!(store.last().sidebar_width, Some(300.0));
}

#[test]
fn the_folder_on_screen_is_remembered_per_host_and_reopened() {
    let (mut s, store) = app_with(Saved::default());
    s.act(Action::SelectHost(prod()));
    s.act(Action::Open("assets".into()));
    assert_eq!(store.last().folder("prod"), Some("/var/www/site/assets"));
    s.act(Action::Back);
    assert_eq!(store.last().folder("prod"), Some("/var/www/site"));
    s.act(Action::Forward);
    let saved = store.last();

    // The next launch opens prod where it was left, as a start (no history to go back to).
    let (mut s, _) = app_with(saved);
    s.act(Action::SelectHost(prod()));
    let c = s.conn().unwrap();
    assert_eq!(c.cwd.as_str(), "/var/www/site/assets");
    assert!(matches!(c.listing, Listing::Ready(_)));
    assert!(!c.history.can_back());
    assert_eq!(c.fallback, None, "listed fine: nothing to fall back to any more");
}

#[test]
fn a_remembered_folder_that_fails_falls_back_to_home() {
    let mut saved = Saved::default();
    saved.set_folder("prod", "/var/www/gone");
    let (mut s, store) = app_with(saved);
    let sent = crate::state::Recorder::default();
    s.backend = Box::new(sent.clone());
    s.act(Action::SelectHost(prod()));
    s.apply(ServiceEvent::Opened { host: prod(), start: RemotePath::new("/home/deploy") });
    assert_eq!(s.conn().unwrap().cwd.as_str(), "/var/www/gone");
    assert_eq!(sent.sent().last(), Some(&Command::List { host: prod(), path: RemotePath::new("/var/www/gone") }));

    let gone = RemotePath::new("/var/www/gone");
    s.apply(ServiceEvent::ListFailed { host: prod(), path: gone.clone(), error: Error::NotFound(gone) });
    let c = s.conn().unwrap();
    assert_eq!(c.cwd.as_str(), "/home/deploy");
    assert_eq!(c.listing, Listing::Loading);
    assert!(!c.history.can_back(), "the failed folder isn't in history");
    assert_eq!(sent.sent().last(), Some(&Command::List { host: prod(), path: RemotePath::new("/home/deploy") }));
    assert_eq!(store.last().folder("prod"), Some("/home/deploy"));

    // Home failing too is shown as it is: the fallback is used once.
    let home = RemotePath::new("/home/deploy");
    s.apply(ServiceEvent::ListFailed { host: prod(), path: home.clone(), error: Error::PermissionDenied(home) });
    assert!(matches!(s.conn().unwrap().listing, Listing::Failed(_)));
}

#[test]
fn a_folder_left_by_hand_never_falls_back() {
    let mut saved = Saved::default();
    saved.set_folder("prod", "/srv/a");
    let (mut s, _) = app_with(saved);
    s.backend = Box::new(Silent::default());
    s.act(Action::SelectHost(prod()));
    s.apply(ServiceEvent::Opened { host: prod(), start: RemotePath::new("/home/deploy") });
    s.act(Action::GoTo(RemotePath::new("/srv/b")));
    let b = RemotePath::new("/srv/b");
    s.apply(ServiceEvent::ListFailed { host: prod(), path: b.clone(), error: Error::NotFound(b) });
    assert_eq!(s.conn().unwrap().cwd.as_str(), "/srv/b");
    assert!(matches!(s.conn().unwrap().listing, Listing::Failed(_)));
}

#[test]
fn only_the_most_recent_hosts_folders_are_kept() {
    let mut saved = Saved::default();
    for i in 0..MAX_FOLDERS + 5 {
        saved.set_folder(&format!("h{i}"), "/x");
    }
    // Using h0 again doesn't bring it back; h5 is the oldest left and moves to the end when used.
    saved.set_folder("h5", "/y");
    assert_eq!(saved.last_folder.len(), MAX_FOLDERS);
    assert_eq!(saved.folder("h4"), None);
    assert_eq!(saved.folder("h5"), Some("/y"));
    assert_eq!(saved.folder_order.last().map(String::as_str), Some("h5"));
}

#[test]
fn scenes_and_fixtures_keep_nothing() {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::Sort(SortKey::Size));
    // NoStore: nothing to observe but that it doesn't panic, and the in-memory copy follows.
    assert_eq!(s.saved().sort, Some(Sort { key: SortKey::Size, dir: SortDir::Descending }));
}
