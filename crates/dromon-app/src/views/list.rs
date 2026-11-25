//! The file list: a sortable header and virtualized 28 pt rows (only the rows in the mounted
//! window are built, so a 10,000-entry folder costs what a screenful does).

use std::borrow::Cow;

use dromon_core::{ConnStatus, Entry, SortDir, SortKey};
use rune::{Align, Color, El, GrowthSpec, ScrollSpec, Tx, col, el, icon, oklch, row, text};

use super::{LIST, Paint, ROWS, drop, run, states};
use crate::fmt;
use crate::icons;
use crate::state::{Action, App, ConnView, ListError, Listing};
use crate::theme::{motion, radius, size, space, type_};

/// The list area for one connection, in whatever state it's in.
pub fn view(s: &App, c: &ConnView, p: Paint) -> El {
    match &c.listing {
        Listing::Loading => col().w_full().h_full().child(header(c, p)).child(states::skeleton(p)),
        Listing::Failed(ListError::PermissionDenied) => states::denied(p, c),
        Listing::Failed(ListError::NotFound) => states::message(p, "This folder no longer exists", c.cwd.as_str(), true),
        Listing::Failed(ListError::Other(e)) => states::message(p, "Couldn't open this folder", e, true),
        Listing::Ready(entries) if entries.is_empty() => states::empty(p),
        Listing::Ready(_) if c.row_count() == 0 && c.filter.is_on() => {
            col().w_full().h_full().child(header(c, p)).child(states::no_matches(p, c.filter.query()))
        }
        Listing::Ready(_) => {
            let rows = rows(s, c, p);
            let dimmed = !matches!(c.status, ConnStatus::Ready);
            let mut body = col().w_full().h_full().child(header(c, p)).child(rows);
            if dimmed {
                // Reconnecting: the last listing stays, dimmed and read-only.
                body = body.opacity(0.6).inert();
            }
            body
        }
    }
}

/// The three columns. Name takes what's left; the cell paddings match the rows'.
fn columns(name: El, size_cell: El, modified: El) -> El {
    row()
        .w_full()
        .h_full()
        .px(space::S1)
        .child(row().w_full().h_full().pl(space::S2).pr(space::S3).gap(space::S2).center_y().child(name).child(size_cell).child(modified))
}

fn header(c: &ConnView, p: Paint) -> El {
    let t = p.t;
    let cell = |key: SortKey, label: &'static str, end: bool| {
        let active = c.sort.key == key;
        let caret = if c.sort.dir == SortDir::Ascending { &icons::CARET_UP } else { &icons::CARET_DOWN };
        let txt = text(label).font(type_::META_STRONG).color(if active { t.text_2 } else { t.text_3 });
        let mut r = row()
            .key(format!("col-{label}"))
            .role("columnheader")
            .label(label)
            .sorted(active.then_some(c.sort.dir == SortDir::Ascending))
            .h(22.0)
            .center_y()
            .radius(4.0)
            .hover(t.hover)
            .press(t.press)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::Sort(key), tx));
        if end {
            r = r.justify_end().pl(space::S1);
            if active {
                r = r.child(icon(caret.at(p.scale), size::ICON).color(t.text_3));
            }
            r.child(txt).pr(space::S1)
        } else {
            r = r.pl(space::S1).child(txt);
            if active {
                r = r.child(icon(caret.at(p.scale), size::ICON).color(t.text_3));
            }
            r.pr(space::S1)
        }
    };
    // Name lines up with the row text: past the row inset, its padding and the icon.
    let name = row().grow(1.0).min_w(0.0).pl(size::ICON + space::S2 - space::S1).child(cell(SortKey::Name, "Name", false));
    let size_cell = row().w(size::COL_SIZE).justify_end().child(cell(SortKey::Size, "Size", true)).mr(-space::S1);
    let modified = row().w(size::COL_MODIFIED).pl(space::S4 - space::S1).child(cell(SortKey::Modified, "Modified", false));
    el().key("list-header")
        .role("row")
        .w_full()
        .h(size::HEADER)
        .fill(t.canvas)
        .border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle)
        .child(columns(name, size_cell, modified))
}

fn rows(s: &App, c: &ConnView, p: Paint) -> El {
    let stack = col()
        .key(ROWS)
        .role("grid")
        .label("Files")
        .multiselectable(true)
        .row_count(c.row_count())
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(c.row_count(), size::ROW, row_key, move |s: &App, i| match s.conn() {
            Some(c) if i < c.row_count() => file_row(s, c, i, p),
            _ => el().h(size::ROW),
        })
        .virtual_rows_version((c.host.as_str().to_owned(), c.rows_version));
    let mut viewport = col().key(LIST).w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec::default()).child(col().w_full().py(space::S1).child(stack));
    if let Some(name) = s.ui.center_on_mount.clone() {
        viewport = viewport.on_mount(move |s: &mut App, tx: &mut Tx| {
            s.ui.center_on_mount = None;
            // Lands the row a third of the way down, mounting it first if it isn't.
            tx.scroll_key_to(format!("{ROWS}/{name}"), 200.0);
        });
    }
    viewport
}

fn row_key(s: &App, i: usize) -> Cow<'_, str> {
    match s.conn() {
        Some(c) if i < c.row_count() => Cow::Borrowed(c.row(i).name.as_str()),
        _ => Cow::Owned(format!("#{i}")),
    }
}

fn file_row(s: &App, c: &ConnView, i: usize, p: Paint) -> El {
    let t = p.t;
    let e = c.row(i);
    let selected = c.selection.contains(&e.name);
    let dim = e.hidden();
    let name_color = if dim { t.text_3 } else { t.text };
    let meta_color = if dim { t.text_3 } else { t.text_2 };
    let open = e.kind.is_dir_like();
    // A file drag over this folder row: it lifts with an accent fill.
    let drop_here = open && drop::over_folder(s, &e.name);
    let (n1, n2) = (e.name.clone(), e.name.clone());
    let name = text(e.name.clone()).font(if selected { type_::BODY_STRONG } else { type_::BODY }).color(name_color).truncate().grow(1.0).min_w(0.0);
    let size_cell = text(fmt::entry_size(e)).font(type_::META).color(meta_color).w(size::COL_SIZE).align(Align::End);
    let modified =
        text(fmt::modified(e.mtime, s.now)).font(type_::META).color(meta_color).w(size::COL_MODIFIED).pl(space::S4).truncate();
    let mut cells = row()
        .w_full()
        .h_full()
        .pl(space::S2)
        .pr(space::S3)
        .gap(space::S2)
        .center_y()
        .radius(radius::ROW)
        .fill(if drop_here {
            t.accent.alpha(0.22)
        } else if selected {
            t.selection
        } else {
            Color::TRANSPARENT
        })
        .hover(t.hover)
        .press(t.press)
        .motion(motion::SNAP);
    if drop_here {
        cells = cells.border(1.5, t.accent).shadow(2.0, 8.0, t.shadow);
    }
    let cells = cells
        .child(file_icon(p, e, selected || drop_here))
        .child(name)
        .child(size_cell)
        .child(modified);
    let mut r = row()
        .role("row")
        .label(e.name.clone())
        .row_index(i)
        .w_full()
        .h(size::ROW)
        .px(space::S1)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let m = tx.mods();
            run(s, Action::Click { name: n1.clone(), extend: m.shift, toggle: m.cmd }, tx);
        })
        .child(cells);
    if open {
        r = r.on_double_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::Open(n2.clone()), tx));
        r = drop::folder_row(r, &e.name);
    }
    r
}

/// The glyph for an entry: folders in the accent, file categories in a quiet tint, a link
/// badge on symlinks; selection lifts neutral icons to `text`.
fn file_icon(p: Paint, e: &Entry, selected: bool) -> El {
    let t = p.t;
    let (glyph, color) = if e.kind.is_dir_like() {
        (&icons::FOLDER, t.folder)
    } else {
        let (glyph, hue) = icons::for_file(&e.name);
        let color = match hue {
            Some(h) => category(t.dark, h),
            None if selected => t.text,
            None => t.text_2,
        };
        (glyph, color)
    };
    let color = if e.hidden() { color.alpha(color.a * 0.6) } else { color };
    let mut cell = el().size(size::ICON, size::ICON).child(icon(glyph.at(p.scale), size::ICON).color(color));
    if e.kind.is_symlink() {
        let badge = el()
            .absolute(-3.0, 7.0)
            .size(10.0, 10.0)
            .radius(5.0)
            .fill(t.canvas)
            .center()
            .child(icon(icons::LINK_BADGE.at(p.scale), 10.0).color(t.text_2));
        cell = cell.child(badge);
    }
    cell
}

/// A file category's tint: its hue at low chroma, readable on the canvas.
fn category(dark: bool, hue: f32) -> Color {
    if dark { oklch(0.74, 0.055, hue) } else { oklch(0.55, 0.06, hue) }
}
