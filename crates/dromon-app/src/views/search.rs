//! A search in subfolders' results (`state/search.rs`), in the list area while one is on: a
//! line saying how it goes, then what it found as the list's grid with four columns (Name,
//! Folder relative to where it started, Size, Modified), virtualized as the folder's rows are
//! and keyed `hit-<n>` in the order found. A click selects, Enter or a double-click opens, a
//! right-click has Download, Copy path and Properties.
//!
//! The header row is for the eye: results aren't sorted (they come as they're found), so it
//! has nothing to press; readers hear each row's folder instead, in its description.

use std::borrow::Cow;

use rune::{Color, El, GrowthSpec, LoopSpec, Tx, col, el, icon, row, text};

use super::{LIST, Paint, ROWS, list, menu, run, transfers};
use crate::icons;
use crate::state::search::{self, Phase, Search, hit_key};
use crate::state::{Action, App, MenuKind, SearchAction};
use crate::theme::{motion, radius, size, space, type_};

/// The results grid's key.
pub const GRID: &str = "search-grid";
/// How much of the free width the Folder column takes beside Name's 1.
const FOLDER_SHARE: f32 = 0.8;

pub fn view(s: &App, r: &Search, p: Paint) -> El {
    let searching = matches!(r.phase, Phase::Waiting { .. } | Phase::Searching { .. });
    let body = if r.found.is_empty() && !searching { nothing_found(r, p) } else { rows(s, r, p) };
    let grid = col()
        .key(GRID)
        .role("grid")
        .label("Search results")
        .multiselectable(true)
        .row_count(r.found.len())
        .column_count(4)
        .busy(searching)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(header(p))
        .child(body);
    col().key("search").w_full().h_full().child(status_line(r, searching, p)).child(grid)
}

/// "Searching… 1,204 folders" with a spinner, then what it found and where.
fn status_line(r: &Search, searching: bool, p: Paint) -> El {
    let t = p.t;
    let spinner = searching
        .then(|| icon(icons::SPINNER.at(p.scale), size::ICON).color(t.text_3).looping(LoopSpec::rotation(0.9)));
    let unreadable = search::unreadable_text(r)
        .map(|u| text(u).key("search-unreadable").font(type_::META).color(t.text_3).truncate().min_w(0.0));
    row()
        .key("search-status")
        .w_full()
        .h(size::HEADER)
        .px(space::S4)
        .gap(space::S2)
        .center_y()
        .border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle)
        .children(spinner)
        .child(text(search::status_text(r)).key("search-status-text").font(type_::META).color(t.text_2).truncate())
        .children(unreadable)
}

/// The four columns, with the rows' paddings.
fn columns(cells: [El; 4]) -> El {
    row()
        .w_full()
        .h_full()
        .px(space::S1)
        .child(row().w_full().h_full().pl(space::S2).pr(space::S3).gap(space::S2).center_y().children(cells))
}

fn header(p: Paint) -> El {
    let t = p.t;
    let label = |s: &'static str| text(s).font(type_::META_STRONG).color(t.text_3);
    let name = row().grow(1.0).min_w(0.0).pl(size::ICON + space::S2).child(label("Name"));
    let folder = row().grow(FOLDER_SHARE).min_w(0.0).child(label("Folder"));
    let size_cell = row().w(size::COL_SIZE).justify_end().child(label("Size"));
    let modified = row().w(size::COL_MODIFIED).pl(space::S4).child(label("Modified"));
    el().key("search-header")
        .w_full()
        .h(size::HEADER)
        .fill(t.canvas)
        .border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle)
        .a11y_hidden()
        .child(columns([name, folder, size_cell, modified]))
}

/// A search that found nothing.
fn nothing_found(r: &Search, p: Paint) -> El {
    let t = p.t;
    col()
        .key("search-empty")
        .w_full()
        .grow(1.0)
        .center()
        .gap(space::S2)
        .pb(size::TOOLBAR)
        .child(text(format!("No results for “{}”", r.query)).font(type_::DISPLAY).color(t.text))
        .child(text("Nothing in this folder or below it matches").font(type_::BODY).color(t.text_2))
}

fn rows(s: &App, r: &Search, p: Paint) -> El {
    // One Tab stop, as the folder's rows: the cursor's row is the active descendant.
    let mut stack = col().key(ROWS).role("rowgroup").label("Search results").focusable(true);
    stack = match r.selection.cursor() {
        Some(key) => stack.active_descendant(key.to_owned()).no_focus_ring(),
        None => stack.focus_ring(2.0, -2.0, p.t.focus),
    };
    let host = r.origin.as_ref().map_or_else(String::new, |(h, _)| h.as_str().to_owned());
    let stack = stack
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(r.found.len(), s.settings.density.row(), row_key, move |s: &App, i| match s.results() {
            Some(r) if i < r.found.len() => hit_row(s, r, i, p),
            _ => el().h(s.settings.density.row()),
        })
        .virtual_rows_version((host, r.version))
        .virtual_rows_append_only();
    col()
        .key(LIST)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec::default())
        .child(col().w_full().py(space::S1).child(stack))
}

fn row_key(_: &App, i: usize) -> Cow<'_, str> {
    Cow::Owned(hit_key(i))
}

fn hit_row(s: &App, r: &Search, i: usize, p: Paint) -> El {
    let t = p.t;
    let hit = &r.found[i];
    let e = &hit.entry;
    let key = hit_key(i);
    let selected = r.selection.contains(&key);
    let dim = e.hidden();
    let (name_color, meta_color) = if dim { (t.text_3, t.text_3) } else { (t.text, t.text_2) };
    let name = text(e.name.clone())
        .font(if selected { type_::BODY_STRONG } else { type_::BODY })
        .color(name_color)
        .truncate()
        .grow(1.0)
        .min_w(0.0)
        .role("gridcell")
        .label(e.name.clone())
        .column_index(1);
    let folder = text(hit.folder.clone())
        .font(type_::META)
        .color(meta_color)
        .truncate()
        .grow(FOLDER_SHARE)
        .min_w(0.0)
        .role("gridcell")
        .label(hit.folder.clone())
        .column_index(2);
    let size_cell = list::size_cell(e, meta_color).column_index(3);
    let modified = list::modified_cell(e, s.now, meta_color).column_index(4);
    let mut cells = row()
        .w_full()
        .h_full()
        .pl(space::S2)
        .pr(space::S3)
        .gap(space::S2)
        .center_y()
        .radius(radius::ROW)
        .fill(if selected { t.selection } else { Color::TRANSPARENT })
        .hover(t.hover)
        .press(t.press)
        .motion(motion::SNAP);
    if r.selection.cursor() == Some(key.as_str()) {
        cells = cells.focus_ring_for(ROWS, 2.0, -2.0, t.focus);
    }
    let cells = cells.child(list::file_icon(p, e, selected)).child(name).child(folder).child(size_cell).child(modified);
    let where_ = if hit.folder == "." { "in the folder searched".to_owned() } else { format!("in {}", hit.folder) };
    let description = match list::kind_words(e) {
        Some(kind) => format!("{kind}, {where_}"),
        None => where_,
    };
    let (k1, k2, k3) = (key.clone(), key.clone(), key);
    row()
        .role("row")
        .label(e.name.clone())
        .description(description)
        .row_index(i + 1)
        .focus_selected(selected)
        .w_full()
        .h(s.settings.density.row())
        .px(space::S1)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let m = tx.mods();
            run(s, Action::Click { name: k1.clone(), extend: m.shift, toggle: m.cmd }, tx);
        })
        .on_double_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::Open(k2.clone()), tx))
        .on_secondary_press(move |s: &mut App, at, tx: &mut Tx| menu::open(s, MenuKind::Hit(k3.clone()), at, tx))
        .child(cells)
}

/// The status bar's count while results are on screen: "5 results · 2 selected, 14.8 KB".
pub fn summary(r: &Search) -> String {
    let n = r.found.len() as u64;
    let mut s = format!("{} {}", crate::fmt::thousands(n), if n == 1 { "result" } else { "results" });
    let selected: Vec<&dromon_core::Entry> = r.selection.iter().filter_map(|k| r.hit(k)).map(|f| &f.entry).collect();
    if !selected.is_empty() {
        s.push_str(&format!(" · {} selected", crate::fmt::thousands(selected.len() as u64)));
        let bytes: u64 = selected.iter().filter(|e| !e.kind.is_dir_like()).map(|e| e.size).sum();
        if bytes > 0 {
            s.push_str(&format!(", {}", crate::fmt::size(bytes)));
        }
    }
    s
}

/// The transfers' poll (the done flourish, what's announced), then a search waiting for typing
/// to pause starts once it's due; part of the app's poll (`views::poll`).
pub fn poll(s: &mut App, clock: f64, tx: &mut Tx) -> bool {
    let changed = transfers::poll(s, clock, tx);
    match s.ui.search.due() {
        Some(due) if clock >= due => {
            run(s, Action::Search(SearchAction::Tick(clock)), tx);
            true
        }
        Some(due) => {
            tx.wake_at(due);
            changed
        }
        None => changed,
    }
}
