//! Refreshes of the folder on screen in the app state (§4.5, §6.8): rows change in place, the
//! selection and cursor keep their place, navigation never animates, a vanished folder climbs
//! to the nearest parent with a toast, and the window's state goes to the service.

use dromon_core::{Entry, EntryKind, Error, HostId, RemotePath};

use super::*;
use crate::fixture;
use crate::state::edit::Edit;
use crate::state::{Action, App, Command, FileAction, Listing, ListError, Recorder, ServiceEvent};

const SITE: &str = "/var/www/site";

fn prod() -> HostId {
    HostId("prod".into())
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// The demo with prod on screen at its site.
fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::SelectHost(prod()));
    s
}

fn recorded(mut s: App) -> (App, Recorder) {
    let rec = Recorder::default();
    s.backend = Box::new(rec.clone());
    (s, rec)
}

fn names(s: &App) -> Vec<String> {
    s.conn().unwrap().row_names().into_iter().map(str::to_owned).collect()
}

fn selected(s: &App) -> Vec<String> {
    let c = s.conn().unwrap();
    c.row_names().into_iter().filter(|n| c.selection.contains(n)).map(str::to_owned).collect()
}

fn click(s: &mut App, name: &str, toggle: bool) {
    s.act(Action::Click { name: name.into(), extend: false, toggle });
}

fn entries(s: &App) -> Vec<Entry> {
    s.conn().unwrap().entries().to_vec()
}

fn file(name: &str, size: u64) -> Entry {
    Entry { name: name.into(), kind: EntryKind::File, size, mtime: Some(fixture::DEMO_NOW), mode: None, owner: None, group: None }
}

fn refreshed(s: &mut App, path: &str, entries: Vec<Entry>) {
    s.apply(ServiceEvent::Refreshed { host: prod(), path: p(path), entries });
}

fn toasts(s: &App) -> Vec<String> {
    s.toasts().into_iter().map(|t| t.text).collect()
}

#[test]
fn the_cursor_goes_to_the_next_row_still_there_else_the_one_before() {
    let before: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
    assert_eq!(neighbour(&before, "b", |n| n != "b" && n != "c"), Some("d"));
    assert_eq!(neighbour(&before, "d", |n| n != "d"), Some("c"));
    assert_eq!(neighbour(&before, "a", |_| false), None);
    assert_eq!(neighbour(&before, "zz", |_| true), None);
}

#[test]
fn a_highlight_is_shown_once_per_refresh() {
    let mut f = Fresh::default().next(vec!["x".into()]);
    assert_eq!(f.seq, 1);
    assert!(f.is_new("x") && !f.is_new("y"));
    assert!(!f.shown(0), "an older refresh's highlight");
    assert!(f.shown(1));
    assert!(!f.shown(1), "already fading");
}

#[test]
fn a_refresh_changes_rows_in_place_and_keeps_the_selection_where_it_can() {
    let mut s = demo();
    s.take_animate();
    click(&mut s, "index.html", false);
    click(&mut s, "robots.txt", true);
    let before = names(&s);
    let at = before.iter().position(|n| n == "robots.txt").unwrap();
    let version = s.conn().unwrap().rows_version;
    // Elsewhere: robots.txt goes, new.txt comes, server.log grows.
    let mut now: Vec<Entry> = entries(&s).into_iter().filter(|e| e.name != "robots.txt").collect();
    now.push(file("new.txt", 12));
    now.iter_mut().find(|e| e.name == "server.log").unwrap().size += 1000;
    refreshed(&mut s, SITE, now);

    let c = s.conn().unwrap();
    assert_eq!(c.cwd.as_str(), SITE);
    assert!(names(&s).contains(&"new.txt".to_owned()) && !names(&s).contains(&"robots.txt".to_owned()));
    assert_eq!(c.find("server.log").unwrap().size, entries(&demo()).iter().find(|e| e.name == "server.log").unwrap().size + 1000);
    assert_eq!(selected(&s), ["index.html"], "the row that left left the selection quietly");
    assert_eq!(c.selection.cursor(), Some(before[at + 1].as_str()), "the cursor moved to the row after it, unselected");
    assert_eq!(c.fresh.inserted.iter().collect::<Vec<_>>(), ["new.txt"]);
    assert!(!c.fresh.seen);
    assert_ne!(c.rows_version, version);
    assert!(!c.history.can_back(), "not a navigation");
    assert!(s.take_animate(), "the commit animates");
    assert!(!s.take_animate());
}

#[test]
fn a_refresh_that_changes_nothing_changes_nothing() {
    let mut s = demo();
    s.take_animate();
    click(&mut s, "index.html", false);
    let (version, fresh) = (s.conn().unwrap().rows_version, s.conn().unwrap().fresh.clone());
    let mut same = entries(&s);
    same.reverse();
    refreshed(&mut s, SITE, same.clone());
    // A ⌘R answered with pages does the same.
    s.apply(ServiceEvent::ListingPage { host: prod(), path: p(SITE), entries: same, first: true, done: true });
    let c = s.conn().unwrap();
    assert_eq!((c.rows_version, &c.fresh), (version, &fresh));
    assert_eq!(selected(&s), ["index.html"]);
    assert!(!s.take_animate());
}

#[test]
fn a_paged_refresh_animates_like_a_poll() {
    let mut s = demo();
    let mut now = entries(&s);
    now.push(file("fresh.txt", 1));
    s.apply(ServiceEvent::ListingPage { host: prod(), path: p(SITE), entries: now[..5].to_vec(), first: true, done: false });
    assert!(s.conn().unwrap().fresh.inserted.is_empty(), "nothing until the refresh is whole");
    s.apply(ServiceEvent::ListingPage { host: prod(), path: p(SITE), entries: now[5..].to_vec(), first: false, done: true });
    assert!(s.conn().unwrap().fresh.is_new("fresh.txt"));
    assert!(s.take_animate());
}

#[test]
fn navigation_and_a_first_listing_never_animate() {
    let mut s = demo();
    s.take_animate();
    s.act(Action::Open("assets".into()));
    assert!(s.conn().unwrap().fresh.inserted.is_empty());
    assert!(!s.take_animate());
    // A refresh arriving before the folder listed is just its listing.
    let (mut s, _) = recorded(s);
    s.act(Action::GoUp);
    refreshed(&mut s, SITE, vec![file("only.txt", 1)]);
    assert_eq!(names(&s), ["only.txt"]);
    assert!(s.conn().unwrap().fresh.inserted.is_empty());
    assert!(!s.take_animate());
}

#[test]
fn a_rename_closes_when_a_poll_takes_its_row() {
    let mut s = demo();
    click(&mut s, "index.html", false);
    s.act(Action::File(FileAction::Rename));
    assert!(matches!(s.files.edit, Edit::Editing(_)));
    let now: Vec<Entry> = entries(&s).into_iter().filter(|e| e.name != "index.html").collect();
    refreshed(&mut s, SITE, now);
    assert!(s.files.edit.field().is_none());
}

#[test]
fn a_vanished_folder_climbs_to_the_nearest_parent_and_says_so() {
    let mut s = demo();
    s.act(Action::GoTo(p("/var/www/site/assets")));
    let (mut s, rec) = recorded(s);
    let gone = |path: &str| ServiceEvent::ListFailed { host: prod(), path: p(path), error: Error::NotFound(p(path)) };

    s.apply(gone("/var/www/site/assets"));
    assert_eq!(s.conn().unwrap().cwd.as_str(), SITE);
    assert_eq!(rec.sent().last(), Some(&Command::List { host: prod(), path: p(SITE) }));
    assert!(toasts(&s).is_empty(), "not until the climb ends");
    assert!(!s.conn().unwrap().history.can_back(), "Back would only have led here");
    // The parent went too: one listing per level.
    s.apply(gone(SITE));
    assert_eq!(s.conn().unwrap().cwd.as_str(), "/var/www");
    assert_eq!(rec.sent().iter().filter(|c| matches!(c, Command::List { .. })).count(), 2);
    s.apply(ServiceEvent::Listing { host: prod(), path: p("/var/www"), entries: vec![file("index.html", 1)] });
    assert_eq!(toasts(&s), ["/var/www/site/assets was removed"]);
    assert_eq!(s.conn().unwrap().vanished, None);
    // The climb added no history: nothing to go back into.
    let sent = rec.sent().len();
    s.act(Action::Back);
    assert_eq!(rec.sent().len(), sent);
}

#[test]
fn a_vanished_folder_whose_parent_is_locked_shows_that_and_the_toast() {
    let (mut s, _) = recorded(demo());
    s.apply(ServiceEvent::ListFailed { host: prod(), path: p(SITE), error: Error::NotFound(p(SITE)) });
    s.apply(ServiceEvent::ListFailed { host: prod(), path: p("/var/www"), error: Error::PermissionDenied(p("/var/www")) });
    assert_eq!(s.conn().unwrap().listing, Listing::Failed(ListError::PermissionDenied));
    assert_eq!(toasts(&s), ["/var/www/site was removed"]);
}

#[test]
fn a_missing_folder_navigated_to_and_a_locked_one_keep_their_states() {
    let mut s = demo();
    s.act(Action::GoTo(p("/nope")));
    assert_eq!(s.conn().unwrap().cwd.as_str(), "/nope");
    assert_eq!(s.conn().unwrap().listing, Listing::Failed(ListError::NotFound));
    assert!(toasts(&s).is_empty());

    let mut s = demo();
    s.apply(ServiceEvent::ListFailed { host: prod(), path: p(SITE), error: Error::PermissionDenied(p(SITE)) });
    assert_eq!(s.conn().unwrap().cwd.as_str(), SITE);
    assert_eq!(s.conn().unwrap().listing, Listing::Failed(ListError::PermissionDenied));
    assert!(toasts(&s).is_empty());
}

#[test]
fn the_window_state_goes_to_the_service_once_per_change_and_focus_polls() {
    let (mut s, rec) = recorded(demo());
    s.window_focus(true);
    assert_eq!(rec.sent(), [Command::Window { visible: true, focused: true }, Command::Poll { host: prod() }]);
    s.window_focus(true);
    assert_eq!(rec.sent().len(), 2, "no change, nothing sent");
    s.window_focus(false);
    s.window_visibility(false);
    s.window_focus(true);
    assert_eq!(
        rec.sent()[2..],
        [
            Command::Window { visible: true, focused: false },
            Command::Window { visible: false, focused: false },
            Command::Window { visible: false, focused: true },
        ],
        "no poll while hidden"
    );

    // With no host on screen, focus polls nothing.
    let mut s = fixture::app(fixture::demo());
    let rec = Recorder::default();
    s.backend = Box::new(rec.clone());
    s.window_focus(true);
    assert_eq!(rec.sent(), [Command::Window { visible: true, focused: true }]);
}
