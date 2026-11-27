//! The accessibility audit (§6.9): every scene's AccessKit tree, as a screen reader gets it
//! (`Runtime::accessibility_json`), checked for controls without a role or a name, two controls
//! of one container that say the same, unnamed images, controls that do nothing, and controls
//! the keyboard can't reach. Also the file list's grid semantics and what says a state in words
//! or shape rather than colour alone. Keyboard walks are `ui_keyboard_tests.rs`.

use rune::{Env, Runtime};
use serde_json::Value;

use crate::state::{Action, App, ServiceEvent};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

pub fn scene_runtime(scene: fn(&mut App)) -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(move |s, _| scene(s));
    settle(&mut rt);
    rt
}

/// The scene named `name` in [`scenes::SCENES`].
pub fn scene(name: &str) -> fn(&mut App) {
    scenes::SCENES.iter().find(|(n, _)| *n == name).map(|(_, f)| *f).unwrap_or_else(|| panic!("no scene {name:?}"))
}

pub fn settle<S: 'static>(rt: &mut Runtime<S>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

/// One node of the tree, flattened.
#[derive(Clone, Debug)]
pub struct Node {
    pub role: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub states: Vec<String>,
    pub actions: Vec<String>,
    pub parent: Option<usize>,
    pub raw: Value,
}

impl Node {
    pub fn has_state(&self, s: &str) -> bool {
        self.states.iter().any(|x| x == s)
    }
    fn acts(&self, a: &str) -> bool {
        self.actions.iter().any(|x| x == a)
    }
}

pub fn flatten(root: &Value) -> Vec<Node> {
    fn strs(v: &Value, k: &str) -> Vec<String> {
        v[k].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    }
    fn walk(v: &Value, parent: Option<usize>, out: &mut Vec<Node>) {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
        let mut raw = v.clone();
        raw.as_object_mut().map(|o| o.remove("children"));
        out.push(Node {
            role: s("role"),
            key: s("key"),
            name: s("name"),
            description: s("description"),
            states: strs(v, "states"),
            actions: strs(v, "actions"),
            parent,
            raw,
        });
        let me = out.len() - 1;
        for c in v["children"].as_array().into_iter().flatten() {
            walk(c, Some(me), out);
        }
    }
    let mut out = Vec::new();
    walk(root, None, &mut out);
    out
}

pub fn tree<S: 'static>(rt: &Runtime<S>) -> Vec<Node> {
    flatten(&rt.accessibility_json())
}

/// Roles of controls: a reader offers to press, type in, toggle or pick them.
const CONTROLS: &[&str] = &[
    "button",
    "default-button",
    "link",
    "check-box",
    "switch",
    "radio-button",
    "tab",
    "menu-item",
    "menu-item-check-box",
    "menu-item-radio",
    "list-box-option",
    "text-input",
    "password-input",
    "search-input",
    "multiline-text-input",
    "combo-box",
    "slider",
    "spin-button",
    "column-header",
];

/// Items of a composite: the container takes the keyboard (arrows), and many of them may say the
/// same (two transfers named "logs").
pub const ITEMS: &[&str] = &[
    "row",
    "list-box-option",
    "list-item",
    "menu-item",
    "menu-item-check-box",
    "menu-item-radio",
    "tab",
    "radio-button",
];

/// Roles that say nothing of what a control is.
const GENERIC: &[&str] = &["group", "generic-container", "unknown", "label", ""];

/// Containers a control's name has to be unique in: the nearest one with a role of its own.
const PLAIN: &[&str] = &["group", "generic-container", "scroll-view"];

pub fn interactive(n: &Node) -> bool {
    !window_chrome(n)
        && (["click", "focus", "set-value", "expand", "collapse"].iter().any(|a| n.acts(a))
            || CONTROLS.contains(&n.role.as_str()))
}

/// Rune's own window chrome: the caption buttons it draws for the custom title bar on Windows
/// (`rune-caption-minimize`, `-maximize`, `-close`). Not the app's controls: named by Rune, and,
/// as native caption buttons, not Tab stops (keyboard users have Alt+Space and Alt+F4).
fn window_chrome(n: &Node) -> bool {
    n.key.starts_with("rune-caption-")
}

fn container(nodes: &[Node], i: usize) -> Option<usize> {
    let mut p = nodes[i].parent;
    while let Some(j) = p {
        if !PLAIN.contains(&nodes[j].role.as_str()) {
            return Some(j);
        }
        p = nodes[j].parent;
    }
    None
}

/// Whether node `i` is inside a composite that holds the keyboard for it: a focusable ancestor
/// (a grid or a field naming its rows as the active descendant), or a focus group one of whose
/// items is focusable.
fn reached_through_container(nodes: &[Node], i: usize) -> bool {
    let mut p = nodes[i].parent;
    while let Some(j) = p {
        if nodes[j].acts("focus") {
            return true;
        }
        if nodes[j].raw.get("activeDescendant").is_some() {
            return true;
        }
        p = nodes[j].parent;
    }
    // A field elsewhere that controls this list (the palette's).
    let list = {
        let mut p = nodes[i].parent;
        let mut found = None;
        while let Some(j) = p {
            if !nodes[j].key.is_empty() && matches!(nodes[j].role.as_str(), "list-box" | "grid" | "menu" | "tab-list") {
                found = Some(nodes[j].key.clone());
                break;
            }
            p = nodes[j].parent;
        }
        found
    };
    list.is_some_and(|k| {
        nodes.iter().any(|n| {
            n.acts("focus") && n.raw["controls"].as_array().is_some_and(|c| c.iter().any(|x| x.as_str() == Some(&k)))
        })
    })
}

#[derive(Debug)]
pub struct Offence {
    pub kind: &'static str,
    pub role: String,
    pub key: String,
    pub name: String,
}

/// What's wrong with a tree.
pub fn audit(nodes: &[Node]) -> Vec<Offence> {
    let mut out = Vec::new();
    let mut flag =
        |kind, n: &Node| out.push(Offence { kind, role: n.role.clone(), key: n.key.clone(), name: n.name.clone() });
    for (i, n) in nodes.iter().enumerate() {
        if n.role == "image" && n.name.trim().is_empty() {
            flag("unnamed image", n);
        }
        if !interactive(n) {
            continue;
        }
        let disabled = n.has_state("disabled");
        if GENERIC.contains(&n.role.as_str()) {
            flag("no role", n);
        }
        if n.name.trim().is_empty() {
            flag("no name", n);
        }
        let item = ITEMS.contains(&n.role.as_str());
        if !disabled && !n.acts("click") && !n.acts("focus") && !n.role.contains("input") {
            flag("does nothing", n);
        }
        if !disabled && n.acts("click") && !n.acts("focus") && !reached_through_container(nodes, i) {
            flag("no keyboard", n);
        }
        if !item && !n.name.is_empty() {
            let c = container(nodes, i);
            let twin = nodes.iter().enumerate().any(|(j, m)| {
                j < i
                    && interactive(m)
                    && !ITEMS.contains(&m.role.as_str())
                    && m.name == n.name
                    && container(nodes, j) == c
            });
            if twin {
                flag("same name", n);
            }
        }
    }
    out
}

fn table(rows: &[(String, Offence)]) -> String {
    let mut s = format!("{:<22} {:<14} {:<20} {:<32} {}\n", "scene", "offence", "role", "key", "name");
    for (scene, o) in rows {
        s.push_str(&format!("{:<22} {:<14} {:<20} {:<32} {:?}\n", scene, o.kind, o.role, o.key, o.name));
    }
    s
}

/// Every scene's tree (the Settings page's among them), and the Settings page's last section.
fn trees() -> Vec<(String, Vec<Node>)> {
    let mut out: Vec<(String, Vec<Node>)> =
        scenes::SCENES.iter().map(|(name, scene)| ((*name).to_owned(), tree(&scene_runtime(*scene)))).collect();
    // The reconnecting banner's Sign in is a state of its own (not a golden scene).
    out.push(("reconnect-sign-in".into(), tree(&scene_runtime(needs_sign_in))));
    out.push(("settings-browsing".into(), tree(&scene_runtime(settings_browsing))));
    out
}

/// The Settings page on Browsing (not a golden scene).
pub(crate) fn settings_browsing(s: &mut App) {
    scenes::welcome(s);
    s.act(Action::OpenSettings);
    s.act(Action::SettingsSection(crate::state::SettingsSection::Browsing));
}

/// The Settings page (§6.9): its sections are a tablist whose chosen tab says so and names the
/// panel it controls; the panel is that tab's; the folder view under the page is out of the
/// tree, the sidebar isn't.
#[test]
fn the_settings_page_is_a_tablist_beside_the_sidebar() {
    let nodes = tree(&scene_runtime(scene("settings-transfers")));
    let list = node(&nodes, "settings-sections");
    assert_eq!((list.role.as_str(), list.name.as_str()), ("tab-list", "Settings sections"));
    assert_eq!(list.raw["orientation"].as_str(), Some("vertical"), "{list:?}");
    let tabs: Vec<_> = children(&nodes, "settings-sections")
        .into_iter()
        .map(|n| (n.role.clone(), n.name.clone(), n.has_state("selected")))
        .collect();
    let tab = |name: &str, on| ("tab".to_owned(), name.to_owned(), on);
    assert_eq!(tabs, [tab("Appearance", false), tab("Transfers", true), tab("Browsing", false), tab("General", false)]);
    let chosen = node(&nodes, "settings-tab-transfers");
    assert!(chosen.acts("click") && chosen.acts("focus"), "{chosen:?}");
    assert!(
        chosen.raw["controls"].as_array().is_some_and(|c| c.iter().any(|k| k.as_str() == Some("settings-panel"))),
        "{chosen:?}"
    );
    let panel = node(&nodes, "settings-panel");
    assert_eq!((panel.role.as_str(), panel.name.as_str()), ("tab-panel", "Transfers"));
    let back = node(&nodes, "settings-back");
    assert_eq!((back.role.as_str(), back.name.as_str()), ("button", "Back"));
    assert!(back.acts("click") && back.acts("focus"));
    let gear = node(&nodes, "sidebar-settings");
    assert_eq!((gear.role.as_str(), gear.name.as_str()), ("button", "Settings"));
    assert!(nodes.iter().any(|n| n.key == "host-prod"), "the sidebar stays");
    for hidden in ["toolbar", "file-grid", "rows", "status-conn"] {
        assert!(!nodes.iter().any(|n| n.key == hidden), "{hidden} is under the page");
    }
}

#[test]
fn every_control_in_every_scene_has_a_role_a_name_and_the_keyboard() {
    let mut offences = Vec::new();
    println!("{:<24} {:>11} {:>9}", "scene", "interactive", "offences");
    for (scene, nodes) in trees() {
        let found = audit(&nodes);
        println!("{:<24} {:>11} {:>9}", scene, nodes.iter().filter(|n| interactive(n)).count(), found.len());
        offences.extend(found.into_iter().map(|o| (scene.clone(), o)));
    }
    assert!(offences.is_empty(), "{} accessibility offences:\n{}", offences.len(), table(&offences));
}

fn node<'a>(nodes: &'a [Node], key: &str) -> &'a Node {
    nodes.iter().find(|n| n.key == key).unwrap_or_else(|| panic!("no node {key:?}"))
}

fn children<'a>(nodes: &'a [Node], parent: &str) -> Vec<&'a Node> {
    let p = nodes.iter().position(|n| n.key == parent).unwrap_or_else(|| panic!("no node {parent:?}"));
    nodes.iter().filter(|n| n.parent == Some(p)).collect()
}

fn int(n: &Node, k: &str) -> Option<i64> {
    n.raw[k].as_i64()
}

#[test]
fn the_file_list_is_a_grid_with_sortable_column_headers() {
    let rt = scene_runtime(scenes::listing);
    let nodes = tree(&rt);
    let grid = node(&nodes, "file-grid");
    assert_eq!(grid.role, "grid");
    assert_eq!(grid.name, "Files");
    assert!(grid.has_state("multiselectable"));
    // The header row and every entry, whatever is mounted; three columns.
    assert_eq!((int(grid, "rowCount"), int(grid, "columnCount")), (Some(18), Some(3)));

    let header = node(&nodes, "list-header");
    assert_eq!((header.role.as_str(), int(header, "rowIndex")), ("row", Some(1)));
    let heads: Vec<_> = children(&nodes, "list-header");
    let names: Vec<_> = heads.iter().map(|h| (h.role.as_str(), h.name.as_str(), int(h, "columnIndex"))).collect();
    assert_eq!(
        names,
        [
            ("column-header", "Name", Some(1)),
            ("column-header", "Size", Some(2)),
            ("column-header", "Modified", Some(3))
        ]
    );
    let sort: Vec<_> = heads.iter().map(|h| h.raw["sort"].as_str().unwrap_or("none")).collect();
    assert_eq!(sort, ["ascending", "none", "none"]);
    assert!(heads.iter().all(|h| h.acts("click") && h.acts("focus")), "the headers are pressed and focused: {heads:?}");

    // Rows: their place among all rows (the header is row 1), selected or not, and a cell per
    // column as the user sees it, with the full date said too.
    let row = node(&nodes, "index.html");
    assert_eq!(row.role, "row");
    assert_eq!(int(row, "rowIndex"), Some(13), "index.html is the 12th entry, after the header");
    assert!(row.has_state("selected"), "{row:?}");
    assert!(!node(&nodes, "assets").has_state("selected"));
    let cells: Vec<_> = children(&nodes, "index.html").into_iter().filter(|c| c.role == "grid-cell").collect();
    let said: Vec<_> = cells.iter().map(|c| (c.name.as_str(), int(c, "columnIndex"))).collect();
    assert_eq!(said, [("index.html", Some(1)), ("14.8 KB", Some(2)), ("Yesterday", Some(3))]);
    assert!(cells[2].description.contains("2026"), "the full date: {:?}", cells[2].description);
    assert!(cells[1].description.contains("bytes"), "the exact size: {:?}", cells[1].description);
    // The rows' container keeps the keyboard and names the cursor's row.
    let rows = node(&nodes, crate::views::ROWS);
    assert!(rows.acts("focus"), "{rows:?}");
    assert_eq!(rows.raw["activeDescendant"].as_str(), Some("index.html"));

    // Sorted by size, descending: only Size says so.
    let nodes = tree(&scene_runtime(scenes::listing_sorted_size));
    let sort: Vec<_> =
        children(&nodes, "list-header").iter().map(|h| h.raw["sort"].as_str().unwrap_or("none").to_owned()).collect();
    assert_eq!(sort, ["none", "descending", "none"]);
}

#[test]
fn a_virtualized_folder_reports_every_row_and_where_each_is() {
    let rt = scene_runtime(scenes::big_folder);
    let nodes = tree(&rt);
    assert_eq!(int(node(&nodes, "file-grid"), "rowCount"), Some(10_001));
    let row = node(&nodes, "IMG_05000.jpg");
    let n: i64 = rt.state.conn().unwrap().row_names().iter().position(|n| *n == "IMG_05000.jpg").unwrap() as i64;
    assert_eq!(int(row, "rowIndex"), Some(n + 2));
    let mounted = nodes.iter().filter(|n| n.role == "row").count();
    assert!(mounted < 400, "only a window of rows is built ({mounted})");
}

/// prod was up and reconnecting needs the user (a rotated password, a 2FA code): the banner
/// offers Sign in.
pub(crate) fn needs_sign_in(s: &mut App) {
    scenes::reconnecting(s);
    s.apply(ServiceEvent::Status {
        host: dromon_core::HostId("prod".into()),
        status: dromon_core::ConnStatus::NeedsInput,
    });
}

/// Status and errors in words or shape, not colour alone (§6.9): the connection's status.
#[test]
fn connection_states_are_said_in_words_not_colour_alone() {
    // The connection dot: the status bar says it, and the sidebar's rows too.
    let nodes = tree(&scene_runtime(scenes::listing));
    assert!(node(&nodes, "status-conn").name.starts_with("Connected"));
    assert!(node(&nodes, "host-prod").description.contains("Connected"), "{:?}", node(&nodes, "host-prod"));
    assert!(!node(&nodes, "host-staging").description.contains("Connected"));
    let nodes = tree(&scene_runtime(scenes::reconnecting));
    assert!(node(&nodes, "status-conn").name.starts_with("Reconnecting"));
    assert!(node(&nodes, "host-prod").description.contains("Reconnecting"));
    assert_eq!(node(&nodes, "banner").role, "status");
    assert!(!nodes.iter().any(|n| n.key == "banner-sign-in"), "Sign in is for a reconnect that needs the user");
    // That one offers Sign in as a live button, pressed and focused like any.
    let nodes = tree(&scene_runtime(needs_sign_in));
    let sign_in = node(&nodes, "banner-sign-in");
    assert_eq!((sign_in.role.as_str(), sign_in.name.as_str()), ("button", "Sign in"));
    assert!(!sign_in.has_state("disabled"), "{sign_in:?}");
    assert!(sign_in.acts("click") && sign_in.acts("focus"), "{sign_in:?}");
}

/// What the icons and dimming show, in words too (§6.9).
#[test]
fn kinds_and_cut_rows_are_said_in_words() {
    // Kinds the icons show: a folder, a link.
    let nodes = tree(&scene_runtime(scenes::listing));
    assert!(node(&nodes, "assets").description.contains("Folder"), "{:?}", node(&nodes, "assets"));
    assert!(node(&nodes, "current").description.contains("link"), "{:?}", node(&nodes, "current"));
    assert!(!node(&nodes, "index.html").description.contains("Folder"));

    // Cut rows are dimmed, and say so.
    let nodes = tree(&scene_runtime(scenes::cut_rows));
    assert!(node(&nodes, "robots.txt").name.ends_with("(cut)"), "{:?}", node(&nodes, "robots.txt"));
    assert!(!node(&nodes, "assets").name.contains("cut"));
}

/// Errors in words, not a red outline alone (§6.9).
#[test]
fn errors_are_said_in_words_as_alerts() {
    // Errors: in words, as alerts.
    let nodes = tree(&scene_runtime(scenes::auth_password_error));
    assert_eq!(node(&nodes, "sheet-error").role, "alert");
    assert!(node(&nodes, "sheet-input").has_state("invalid"));
    let nodes = tree(&scene_runtime(scenes::quick_connect_error));
    assert!(node(&nodes, "quick-host-input").has_state("invalid"));
    assert_eq!(node(&nodes, "quick-host-error").role, "alert");
    let nodes = tree(&scene_runtime(scenes::rename_error));
    assert_eq!(node(&nodes, "edit-error").role, "alert");
    assert!(node(&nodes, "edit-input").has_state("invalid"));
    let nodes = tree(&scene_runtime(scenes::delete_failed));
    assert!(nodes.iter().any(|n| n.role == "alert" && n.name.starts_with("Couldn't delete")));
}

/// A transfer's state in words, not its bar's colour alone (§6.9).
#[test]
fn transfer_states_are_said_in_words() {
    // Transfers: failed and done in words; the newer date says "newer".
    let nodes = tree(&scene_runtime(scenes::transfers));
    assert!(node(&nodes, "tr-3").name.contains("Failed"));
    assert!(node(&nodes, "tr-1").name.contains("Done"));
    let nodes = tree(&scene_runtime(scenes::transfer_conflict));
    assert!(node(&nodes, "conflict-incoming-date").name.ends_with("newer"));
}

/// The selection is a shape (the inset fill), not only the accent: it shows in increased
/// contrast and grayscale alike, and the row says it's selected.
#[test]
fn a_selected_row_has_a_fill_and_says_so() {
    let rt = scene_runtime(scenes::listing);
    let fill = |name: &str| {
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(name)).unwrap();
        rt.elements()
            .into_iter()
            .filter(|e| e.parent.is_some() && e.path.starts_with(&row.path) && e.path != row.path)
            .map(|e| e.fill.a)
            .fold(0.0, f32::max)
    };
    assert!(fill("index.html") > 0.05, "the selected row is filled");
    assert!(fill("assets") < 0.01, "an unselected one isn't");
}

fn announced(rt: &Runtime<App>) -> String {
    let root = rt.accessibility_json();
    // The window's own live region: the last child of the root.
    root["children"]
        .as_array()
        .and_then(|c| c.last())
        .and_then(|n| n["name"].as_str())
        .unwrap_or_default()
        .trim_end_matches('\u{200B}')
        .to_owned()
}

fn after(rt: &mut Runtime<App>, f: impl FnOnce(&mut App) + 'static) -> String {
    rt.update(move |s, _| f(s));
    settle(rt);
    announced(rt)
}

#[test]
fn finished_work_and_lost_connections_are_announced() {
    use dromon_core::{ConnStatus, HostId};
    use dromon_transfer::TaskState;
    let prod = || HostId("prod".into());
    let mut rt = scene_runtime(scenes::toolbar_ring);
    assert_eq!(announced(&rt), "", "nothing is said for what was there");

    // A transfer finishes, another fails.
    let done = after(&mut rt, |s| {
        let mut jobs = s.transfers.jobs.clone();
        jobs[0].state = TaskState::Done;
        s.apply(ServiceEvent::Transfers(jobs));
    });
    assert_eq!(done, "Download of site finished");
    let failed = after(&mut rt, |s| {
        let mut jobs = s.transfers.jobs.clone();
        jobs[1].state = TaskState::Failed { error: "1 file failed".into() };
        s.apply(ServiceEvent::Transfers(jobs));
    });
    assert_eq!(failed, "Upload of logs failed: 1 file failed");

    // The connection drops and comes back.
    let lost = after(&mut rt, move |s| {
        s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } })
    });
    assert_eq!(lost, "Connection to prod lost. Reconnecting…");
    let back = after(&mut rt, move |s| s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Ready }));
    assert_eq!(back, "Reconnected to prod");

    // A delete: done, and one that fails.
    let mut rt = scene_runtime(scenes::listing);
    let gone = after(&mut rt, |s| {
        s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
        s.act(Action::File(crate::state::FileAction::Delete));
        s.act(Action::File(crate::state::FileAction::ConfirmDelete));
    });
    assert_eq!(gone, "Deleted 1 item");
    let mut rt = scene_runtime(scenes::delete_progress);
    let failed = after(&mut rt, |s| {
        let op = s.ops.ops.last().unwrap();
        let (id, host) = (op.id, op.host.clone());
        let error = dromon_core::Error::PermissionDenied(dromon_core::RemotePath::new("/var/www/site/assets"));
        s.apply(ServiceEvent::FileOpFailed { id, host, error });
    });
    assert!(failed.starts_with("Couldn't delete ‘assets’"), "{failed:?}");

    // The folder on screen vanished (as `scenes::folder_vanished` has it): the toast's words.
    let mut rt = scene_runtime(scenes::listing);
    after(&mut rt, |s| drop(s.act(Action::Open("releases".into()))));
    let vanished = after(&mut rt, move |s| {
        use dromon_core::RemotePath;
        s.backend = Box::new(crate::state::Silent::default());
        let gone = RemotePath::new("/var/www/site/releases");
        s.apply(ServiceEvent::ListFailed {
            host: prod(),
            path: gone.clone(),
            error: dromon_core::Error::NotFound(gone),
        });
        let mut entries = crate::fixture::active()
            .server("prod")
            .and_then(|srv| srv.list(&RemotePath::new("/var/www/site")).ok())
            .unwrap_or_default();
        entries.retain(|e| e.name != "releases");
        s.apply(ServiceEvent::Listing { host: prod(), path: RemotePath::new("/var/www/site"), entries });
    });
    assert_eq!(vanished, "/var/www/site/releases was removed");
}
