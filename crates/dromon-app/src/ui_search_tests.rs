//! Search in subfolders driven headless through the real view, handlers, poll and `on_key`, on
//! the demo fixture (which searches at once): ⌘⇧F, typing until the pause starts the search,
//! results with their folders in the list's place, Enter on a file opening its folder with it
//! selected, Escape back to the folder view, and the scope switch. The machine's every step and
//! ending are `state/search_tests.rs`'s.

use dromon_core::HostId;
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::state::search::DEBOUNCE;
use crate::state::{Action, App, Scope};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

fn runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| {
        *s = scenes::base();
        s.act(Action::SelectHost(HostId("prod".into())));
    });
    rt.frame(0.0);
    rt
}

fn key(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

/// ⌘ on macOS, Ctrl elsewhere, as the platform reports the primary modifier.
fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

fn text_of(rt: &Runtime<App>, k: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).and_then(|e| e.text).unwrap_or_default()
}

fn click(rt: &mut Runtime<App>, k: &str) {
    let (x, y) = rt.find(k).unwrap_or_else(|| panic!("no element {k:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

/// The results on screen: each row's name and folder cells, in order.
fn results(rt: &Runtime<App>) -> Vec<(String, String)> {
    let r = rt.state.results().expect("results on screen");
    (0..r.found.len())
        .filter(|i| rt.find(&format!("rows/hit-{i}")).is_some())
        .map(|i| (r.found[i].entry.name.clone(), r.found[i].folder.clone()))
        .collect()
}

#[test]
fn typing_searches_subfolders_enter_opens_a_files_folder_and_escape_restores_the_list() {
    let mut rt = runtime();
    key(&mut rt, Key::Command('f'), Mods { shift: true, ..primary() });
    assert!(rt.find("tb-filter-input").is_some(), "⌘⇧F opens the field");
    assert_eq!(rt.state.ui.search.scope, Scope::Subfolders);
    key(&mut rt, Key::Text("app".into()), Mods::default());
    key(&mut rt, Key::Text(".min".into()), Mods::default());
    assert!(rt.state.ui.search.found.is_empty(), "nothing is searched while typing goes on");
    rt.frame(DEBOUNCE);
    rt.frame(1.0 / 60.0);
    let mut got = results(&rt);
    got.sort();
    assert_eq!(
        got,
        [("app.min.js".into(), ".".into()), ("app.min.js".into(), "releases/20261125-1348".into())],
        "every match below the folder, each with its folder"
    );
    assert!(rt.find("rows/index.html").is_none(), "the results take the list's place");
    assert_eq!(text_of(&rt, "search-status-text"), "2 results in 10 folders");

    // ↓ from the field moves among the results; Return opens the deep one's folder with it selected.
    let deep = rt.state.ui.search.found.iter().position(|f| f.folder != ".").expect("the release's file");
    key(&mut rt, Key::Down, Mods::default());
    if rt.state.ui.search.selection.cursor() != Some(format!("hit-{deep}").as_str()) {
        key(&mut rt, Key::Down, Mods::default());
    }
    key(&mut rt, Key::Enter, Mods::default());
    let c = rt.state.conn().unwrap();
    assert_eq!(c.cwd.as_str(), "/var/www/site/releases/20261125-1348");
    assert_eq!(c.selection.cursor(), Some("app.min.js"));
    assert!(rt.state.results().is_none(), "opening ended the search");
    assert!(rt.find("rows/app.min.js").is_some());

    // Back, search again, and Escape: the folder view as it was.
    key(&mut rt, Key::Command('['), primary());
    click(&mut rt, "rows/README.md");
    key(&mut rt, Key::Command('f'), Mods { shift: true, ..primary() });
    key(&mut rt, Key::Text("log".into()), Mods::default());
    rt.frame(DEBOUNCE);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.results().is_some());
    key(&mut rt, Key::Escape, Mods::default());
    assert!(rt.state.results().is_none());
    assert!(rt.find("tb-filter-input").is_none());
    assert!(rt.find("rows/index.html").is_some());
    let c = rt.state.conn().unwrap();
    assert_eq!((c.cwd.as_str(), c.row_count()), ("/var/www/site", 17));
    assert!(c.selection.contains("README.md"), "the folder's selection is kept");
}

#[test]
fn the_scope_switch_turns_the_filter_into_a_search_and_back() {
    let mut rt = runtime();
    key(&mut rt, Key::Command('f'), primary());
    key(&mut rt, Key::Text("json".into()), Mods::default());
    assert_eq!(rt.state.conn().unwrap().row_count(), 2, "This folder filters the rows");
    click(&mut rt, "tb-scope-subfolders");
    assert_eq!(rt.state.ui.search.scope, Scope::Subfolders);
    assert_eq!(results(&rt).len(), 3, "the text there is searched for at once");
    click(&mut rt, "tb-scope-folder");
    assert!(rt.state.results().is_none());
    assert_eq!(rt.state.conn().unwrap().row_count(), 2);
}

fn rect_of(rt: &Runtime<App>, k: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(k) && !e.exiting)
        .unwrap_or_else(|| panic!("no element {k:?}"))
        .rect
}

#[test]
fn shift_f10_and_the_menu_key_open_the_cursor_results_menu_below_its_row() {
    let shift = Mods { shift: true, ..Mods::default() };
    for (k, mods) in [(Key::Function(10), shift), (Key::ContextMenu, Mods::default())] {
        let mut rt = runtime();
        rt.update(|s, _| {
            scenes::search_results(s);
            // The keyboard is on the results, not in the field.
            s.ui.filter_edit.focused = false;
        });
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        // The first result, near the top, so the whole menu fits below it.
        click(&mut rt, "rows/hit-0");
        assert_eq!(rt.state.ui.search.selection.cursor(), Some("hit-0"));
        key(&mut rt, k, mods);
        rt.frame(0.25);
        assert!(rt.find("ctx-menu").is_some(), "the menu opens");
        assert!(rt.find("ctx-properties").is_some() && rt.find("ctx-rename").is_none(), "the result's menu");
        // Anchored below the result row's bottom-left, as a folder row's is.
        let (list, menu, row) = (rect_of(&rt, "list"), rect_of(&rt, "ctx-menu"), rect_of(&rt, "hit-0"));
        let bottom = row[1] + row[3];
        assert!((menu[0] - list[0]).abs() < 1.5, "menu x {} at the row's left {}", menu[0], list[0]);
        assert!((menu[1] - (bottom + 2.0)).abs() < 1.5, "menu y {} below the row's bottom {bottom}", menu[1]);
        key(&mut rt, Key::Escape, Mods::default());
        assert!(rt.find("ctx-menu").is_none());
    }
}

#[test]
fn a_results_row_menu_offers_download_copy_path_and_properties() {
    let mut rt = runtime();
    rt.update(|s, _| scenes::search_results(s));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let (x, y) = rt.find("rows/hit-0").expect("a result");
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(1.0 / 60.0);
    for item in ["ctx-download", "ctx-copy-path", "ctx-properties"] {
        assert!(rt.find(item).is_some(), "{item}");
    }
    for item in ["ctx-rename", "ctx-delete", "ctx-open"] {
        assert!(rt.find(item).is_none(), "{item} is the folder view's");
    }
    click(&mut rt, "ctx-properties");
    let v = rt.state.ui.sheets.properties().expect("the result's Properties");
    let hit = &rt.state.ui.search.found[0];
    let folder = hit.dir(&dromon_core::RemotePath::new("/var/www/site"));
    assert_eq!((&v.entry.name, &v.folder), (&hit.entry.name, &folder));
}
