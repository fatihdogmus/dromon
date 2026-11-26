//! The palette's state machine, its groups and rows, what it hides, path mode and its keys, on
//! the demo fixture.

use dromon_core::{ConnStatus, HostId, RemotePath};

use super::*;
use crate::fixture;
use crate::state::{ServiceEvent, Shortcut};

fn host(alias: &str) -> HostId {
    HostId(alias.into())
}

fn demo() -> App {
    fixture::app(fixture::demo())
}

/// prod on screen, having been in releases and assets (now back in the site).
fn on_prod() -> App {
    let mut s = demo();
    s.act(Action::SelectHost(host("prod")));
    s.act(Action::Open("releases".into()));
    s.act(Action::GoUp);
    s.act(Action::Open("assets".into()));
    s.act(Action::GoUp);
    // Coming up selected assets: nothing selected.
    s.act(Action::Escape);
    s
}

fn pal(s: &mut App, a: PaletteAction) -> Vec<Effect> {
    s.act(Action::Palette(a))
}

/// Types `text` into the palette's field (replacing it), as the field's change handler does.
fn type_in(s: &mut App, text: &str) {
    s.ui.palette.edit = TextEdit::new(text);
    s.ui.palette.edit.focused = true;
    pal(s, PaletteAction::Edited);
}

fn keys(s: &App) -> Vec<String> {
    s.palette_items().into_iter().map(|i| i.key).collect()
}

fn groups(s: &App) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for i in s.palette_items() {
        if out.last() != Some(&i.group) {
            out.push(i.group);
        }
    }
    out
}

fn actions(s: &App) -> Vec<String> {
    s.palette_items().into_iter().filter(|i| i.group == Group::Actions).map(|i| i.key).collect()
}

#[test]
fn the_state_machine_opens_moves_wraps_and_closes() {
    let mut p = Palette::default();
    step(&mut p, Event::Move { delta: 1, count: 3 });
    step(&mut p, Event::Edited);
    assert_eq!((p.phase, p.cursor), (Phase::Closed, 0), "closed: nothing but Open moves it");
    step(&mut p, Event::Open { mode: Mode::Path, text: "/var/www".into() });
    assert_eq!(p.phase, Phase::Open(Mode::Path));
    assert!(p.focused(), "the field takes the keyboard");
    assert_eq!((p.edit.anchor, p.edit.focus), (0, 8), "all of it selected");
    assert!(p.untouched());
    step(&mut p, Event::Move { delta: 1, count: 3 });
    step(&mut p, Event::Move { delta: 1, count: 3 });
    assert_eq!(p.cursor, 2);
    step(&mut p, Event::Move { delta: 1, count: 3 });
    assert_eq!(p.cursor, 0, "↓ at the end wraps to the top");
    step(&mut p, Event::Move { delta: -1, count: 3 });
    assert_eq!(p.cursor, 2, "↑ at the top wraps to the end");
    step(&mut p, Event::Move { delta: 1, count: 0 });
    assert_eq!(p.cursor, 0, "no rows: the cursor rests at 0");
    p.cursor = 2;
    step(&mut p, Event::Edited);
    assert_eq!(p.cursor, 0, "a new query starts at the top");
    step(&mut p, Event::Close);
    assert!(!p.is_open() && !p.focused() && p.edit.text.is_empty());
}

#[test]
fn an_empty_query_lists_hosts_then_actions() {
    let mut s = demo();
    pal(&mut s, PaletteAction::Toggle);
    assert_eq!(groups(&s), [Group::Hosts, Group::Actions]);
    let items = s.palette_items();
    let hosts: Vec<&str> = items.iter().filter(|i| i.group == Group::Hosts).map(|i| i.title.as_str()).collect();
    assert_eq!(hosts, ["prod", "staging", "nas", "pi", "build-box", "box"], "sidebar order, capped at {MAX_HOSTS}");
    assert_eq!(items[0].shortcut, Some("mod+1"));
    assert_eq!(items[5].shortcut, Some("mod+6"));
    assert_eq!(items[0].detail.as_deref(), Some("deploy@prod.example.com"));
    assert_eq!(items[0].action, Action::SelectHost(host("prod")));
    // Nothing on screen: only what works without a host.
    assert_eq!(actions(&s), ["pal-quick-connect", "pal-show-transfers", "pal-hidden", "pal-settings"]);
    let qc = items.iter().find(|i| i.key == "pal-quick-connect").unwrap();
    assert_eq!((qc.title.as_str(), qc.shortcut), ("Quick connect…", Some("mod+shift+k")));
}

#[test]
fn actions_that_cant_run_now_are_hidden() {
    let mut s = on_prod();
    pal(&mut s, PaletteAction::Toggle);
    assert_eq!(
        actions(&s),
        [
            "pal-quick-connect",
            "pal-go-to-folder",
            "pal-filter",
            "pal-refresh",
            "pal-new-folder",
            "pal-upload",
            "pal-copy-path",
            "pal-show-transfers",
            "pal-hidden",
            "pal-log"
        ],
        "nothing selected: no Download, Rename or Delete (and the cap of {MAX_ACTIONS} trims Disconnect, listed last)"
    );
    s.act(Action::Click { name: "index.html".into(), extend: false, toggle: false });
    let a = actions(&s);
    assert!(["pal-download", "pal-rename", "pal-delete"].iter().all(|k| a.iter().any(|x| x == k)), "{a:?}");
    assert_eq!(a.len(), MAX_ACTIONS, "capped");
    // Two selected: no Rename.
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: true });
    type_in(&mut s, "ren");
    assert!(!actions(&s).contains(&"pal-rename".to_owned()));
    // Reconnecting: nothing that touches the server, and no paths to go to.
    s.apply(ServiceEvent::Status { host: host("prod"), status: ConnStatus::Reconnecting { attempt: 1 } });
    type_in(&mut s, "");
    let a = actions(&s);
    for gone in
        ["pal-go-to-folder", "pal-filter", "pal-refresh", "pal-new-folder", "pal-download", "pal-rename", "pal-delete"]
    {
        assert!(!a.iter().any(|x| x == gone), "{gone} in {a:?}");
    }
    assert!(a.iter().any(|x| x == "pal-copy-path") && a.iter().any(|x| x == "pal-disconnect"), "{a:?}");
    type_in(&mut s, "/var");
    assert!(!groups(&s).contains(&Group::Folders), "no Go to while it reconnects");
    // And a hidden row doesn't run, even by its key.
    type_in(&mut s, "");
    pal(&mut s, PaletteAction::Pick("pal-new-folder".into()));
    assert!(s.files.edit.field().is_none());
    // Disconnect names the host it closes.
    let disconnect = s.palette_items().into_iter().find(|i| i.key == "pal-disconnect").unwrap();
    assert_eq!((disconnect.detail.as_deref(), disconnect.action), (Some("prod"), Action::Disconnect(host("prod"))));
}

#[test]
fn typing_ranks_within_groups() {
    let mut s = on_prod();
    pal(&mut s, PaletteAction::Toggle);
    type_in(&mut s, "nf");
    let items = s.palette_items();
    assert_eq!(items[0].key, "pal-new-folder", "{:?}", keys(&s));
    assert_eq!(items[0].hits, [(0, 1), (4, 5)]);
    type_in(&mut s, "sta");
    assert_eq!(keys(&s)[0], "pal-host-staging");
    assert_eq!(
        s.palette_items()[0].shortcut,
        Some("mod+2"),
        "its ⌘ number is the sidebar's (prod is Connected, first)"
    );
    type_in(&mut s, "zzz");
    assert!(s.palette_items().is_empty());
    pal(&mut s, PaletteAction::Run);
    assert!(s.ui.palette.is_open(), "Return on nothing keeps it open");
}

#[test]
fn a_typed_path_offers_go_to_and_recent_folders() {
    let mut s = on_prod();
    pal(&mut s, PaletteAction::Toggle);
    type_in(&mut s, "/var/www/site/rel");
    assert_eq!(groups(&s), [Group::Folders], "{:?}", keys(&s));
    let items = s.palette_items();
    assert_eq!(items[0].key, "pal-go");
    assert_eq!(items[0].title, "Go to /var/www/site/rel");
    assert_eq!(items[0].detail.as_deref(), Some("on prod"));
    assert_eq!(items[1].key, "pal-folder-/var/www/site/releases");
    // Recent folders match without a slash too, after the hosts.
    type_in(&mut s, "assets");
    let k = keys(&s);
    assert!(k.contains(&"pal-folder-/var/www/site/assets".to_owned()) && !k.contains(&"pal-go".to_owned()), "{k:?}");
    // ~ is the session's home.
    type_in(&mut s, "~/uploads");
    assert_eq!(s.palette_items()[0].action, Action::GoTo(RemotePath::new("/var/www/site/uploads")));
    let fx = pal(&mut s, PaletteAction::Run);
    assert!(!s.ui.palette.is_open());
    assert!(fx.contains(&Effect::Blur), "the keyboard goes back to the list");
    assert_eq!(s.conn().unwrap().cwd, RemotePath::new("/var/www/site/uploads"));
}

#[test]
fn path_mode_opens_on_the_folder_on_screen() {
    let mut s = on_prod();
    pal(&mut s, PaletteAction::OpenPath);
    assert_eq!(s.ui.palette.mode(), Some(Mode::Path));
    assert_eq!(s.ui.palette.edit.text, "/var/www/site");
    assert_eq!((s.ui.palette.edit.anchor, s.ui.palette.edit.focus), (0, 13), "all selected");
    // Untouched: Go to the folder itself, then every recent folder, most recent first.
    assert_eq!(keys(&s), ["pal-go", "pal-folder-/var/www/site/assets", "pal-folder-/var/www/site/releases"]);
    // A relative path from the folder on screen; ".." climbs.
    type_in(&mut s, "../staging-site");
    assert_eq!(s.palette_items()[0].action, Action::GoTo(RemotePath::new("/var/www/staging-site")));
    type_in(&mut s, "releases");
    assert_eq!(keys(&s), ["pal-go"], "the recent folder it names isn't listed twice");
    assert_eq!(s.palette_items()[0].title, "Go to /var/www/site/releases");
    type_in(&mut s, "rel");
    assert_eq!(keys(&s), ["pal-go", "pal-folder-/var/www/site/releases"]);
    // ↓ then Return runs the recent folder.
    pal(&mut s, PaletteAction::Move(1));
    pal(&mut s, PaletteAction::Run);
    assert_eq!(s.conn().unwrap().cwd, RemotePath::new("/var/www/site/releases"));
    // A folder that isn't there shows the not-found state.
    pal(&mut s, PaletteAction::OpenPath);
    type_in(&mut s, "/nope");
    pal(&mut s, PaletteAction::Run);
    assert_eq!(s.conn().unwrap().listing, Listing::Failed(super::super::ListError::NotFound));
    // No host on screen: ⌘L does nothing.
    let mut w = demo();
    pal(&mut w, PaletteAction::OpenPath);
    assert!(!w.ui.palette.is_open());
}

#[test]
fn go_to_folder_turns_the_palette_to_a_path() {
    let mut s = on_prod();
    pal(&mut s, PaletteAction::Toggle);
    type_in(&mut s, "go to");
    assert_eq!(keys(&s)[0], "pal-go-to-folder");
    pal(&mut s, PaletteAction::Run);
    assert_eq!(s.ui.palette.mode(), Some(Mode::Path), "it stays open, on the path");
    assert_eq!(s.ui.palette.edit.text, "/var/www/site");
}

#[test]
fn return_connects_a_host_and_escape_closes() {
    let mut s = demo();
    pal(&mut s, PaletteAction::Toggle);
    type_in(&mut s, "nas");
    let fx = pal(&mut s, PaletteAction::Run);
    assert_eq!(s.active, Some(host("nas")));
    assert_eq!(s.conn().unwrap().status, ConnStatus::Ready);
    assert!(!s.ui.palette.is_open() && fx.contains(&Effect::Blur));
    pal(&mut s, PaletteAction::Toggle);
    assert!(s.ui.palette.is_open());
    assert_eq!(s.shortcut(Shortcut::Escape, 0.0), [Effect::Blur]);
    assert!(!s.ui.palette.is_open());
    // ⌘K toggles.
    s.shortcut(Shortcut::Palette, 0.0);
    assert!(s.ui.palette.is_open());
    s.shortcut(Shortcut::Palette, 0.0);
    assert!(!s.ui.palette.is_open());
}

#[test]
fn it_is_modal_and_opening_it_closes_the_popovers() {
    let mut s = on_prod();
    s.act(Action::ToggleTransfers);
    s.act(Action::OpenFilter);
    assert!(s.ui.filter_edit.focused);
    s.shortcut(Shortcut::Palette, 0.0);
    assert!(s.ui.palette.is_open(), "⌘K works from the filter field");
    assert!(!s.transfers.open && !s.ui.filter_edit.focused);
    assert!(s.text_focused(), "its field counts as a text field");
    s.act(Action::Click { name: "index.html".into(), extend: false, toggle: false });
    s.shortcut(Shortcut::File(super::super::keys::FileKey::Delete), 0.0);
    s.shortcut(Shortcut::Host(2), 0.0);
    assert!(s.ui.sheets.open().is_none(), "the list's keys don't run behind it");
    assert_eq!(s.active, Some(host("prod")));
    // ↓ and Return reach it when its field doesn't have the keyboard.
    s.ui.palette.edit.focused = false;
    s.shortcut(Shortcut::Move { delta: 1, extend: false }, 0.0);
    assert_eq!(s.ui.palette.cursor, 1);
    s.shortcut(Shortcut::Open, 0.0);
    assert!(!s.ui.palette.is_open());
    assert_eq!(s.active, Some(host("staging")), "the second row ran");
}

#[test]
fn running_an_action_closes_it_first() {
    let mut s = on_prod();
    pal(&mut s, PaletteAction::Toggle);
    type_in(&mut s, "filter");
    pal(&mut s, PaletteAction::Run);
    assert!(!s.ui.palette.is_open());
    assert!(s.conn().unwrap().filter.is_on() && s.ui.filter_edit.focused, "the filter field has the keyboard");
    pal(&mut s, PaletteAction::Toggle);
    type_in(&mut s, "show transfers");
    pal(&mut s, PaletteAction::Run);
    assert!(s.transfers.open);
}

#[test]
fn cmd_digits_follow_the_sidebar_order() {
    let mut s = demo();
    let order = |s: &App| s.sidebar_order().into_iter().map(|h| h.0).collect::<Vec<_>>();
    let sidebar = |s: &App| -> Vec<String> {
        s.hosts.sections(&s.conns).into_iter().flat_map(|(_, hs)| hs.into_iter().map(|h| h.id.0.clone())).collect()
    };
    assert_eq!(order(&s), ["prod", "staging", "nas", "pi", "build-box", "box", "media", "203.0.113.7", "10.0.0.12"]);
    s.shortcut(Shortcut::Host(3), 0.0);
    assert_eq!(s.active, Some(host("nas")), "⌘3: the third host");
    // nas is Connected now: first in the sidebar, so first for ⌘1.
    assert_eq!(order(&s), sidebar(&s));
    assert_eq!(order(&s)[..3], ["nas", "prod", "staging"]);
    s.shortcut(Shortcut::Host(2), 0.0);
    assert_eq!(s.active, Some(host("prod")));
    assert_eq!(order(&s)[..3], ["prod", "nas", "staging"], "Connected keeps the directory's order");
    s.shortcut(Shortcut::Host(9), 0.0);
    assert_eq!(s.active, Some(host("10.0.0.12")), "⌘9: the last, a recent host");
    // A focused field keeps ⌘1–9, ⌘L and ⌘⇧J.
    s.act(Action::SelectHost(host("prod")));
    s.act(Action::OpenFilter);
    s.shortcut(Shortcut::Host(1), 0.0);
    s.shortcut(Shortcut::GoToPath, 0.0);
    s.shortcut(Shortcut::ShowTransfers, 0.0);
    assert_eq!(s.active, Some(host("prod")));
    assert!(!s.ui.palette.is_open() && !s.transfers.open);
}

#[test]
fn cmd_shift_j_toggles_the_transfers_popover() {
    let mut s = on_prod();
    s.shortcut(Shortcut::ShowTransfers, 0.0);
    assert!(s.transfers.open);
    s.shortcut(Shortcut::ShowTransfers, 0.0);
    assert!(!s.transfers.open);
    s.shortcut(Shortcut::GoToPath, 0.0);
    assert_eq!(s.ui.palette.mode(), Some(Mode::Path), "⌘L opens the path");
}

#[test]
fn paths_resolve_from_the_folder_and_home() {
    let (cwd, home) = (RemotePath::new("/var/www/site"), RemotePath::new("/home/deploy"));
    let r = |t: &str| resolve(t, &cwd, Some(&home)).map(|p| p.to_string());
    assert_eq!(r("/etc/nginx/").as_deref(), Some("/etc/nginx"));
    assert_eq!(r("assets").as_deref(), Some("/var/www/site/assets"));
    assert_eq!(r("../..").as_deref(), Some("/var"));
    assert_eq!(r("~").as_deref(), Some("/home/deploy"));
    assert_eq!(r("~/").as_deref(), Some("/home/deploy"));
    assert_eq!(r(" ~/logs/../www ").as_deref(), Some("/home/deploy/www"));
    assert_eq!(r("~root"), None, "another user's home isn't known");
    assert_eq!(r("  "), None);
    assert_eq!(resolve("~/x", &cwd, None), None, "no home yet");
}
