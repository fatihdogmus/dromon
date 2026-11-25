//! The file list: a sortable header and virtualized rows, 28 pt or 24 pt by Settings' density
//! (only the rows in the mounted window are built, so a 10,000-entry folder costs what a
//! screenful does).

use std::borrow::Cow;

use dromon_core::{ConnStatus, Entry, SortDir, SortKey};
use rune::{Align, Anim, AnimExt, Color, Curve, Easing, El, GrowthSpec, ScrollSpec, Transition, Tx, col, el, icon, oklch, row, text};

use super::{LIST, Paint, ROWS, drag, drop, edit, menu, run, states};
use crate::fmt;
use crate::icons;
use crate::state::refresh::HIGHLIGHT;
use crate::state::{Action, App, ConnView, FileAction, ListError, Listing, MenuKind, RowTarget};
use crate::theme::{motion, radius, size, space, type_};

/// The list area for one connection, in whatever state it's in.
pub fn view(s: &App, c: &ConnView, p: Paint) -> El {
    match &c.listing {
        Listing::Loading => col().w_full().h_full().child(header(c, p)).child(states::skeleton(p, s.settings.density.row())),
        Listing::Failed(ListError::PermissionDenied) => states::denied(p, c),
        Listing::Failed(ListError::NotFound) => states::message(p, "This folder no longer exists", c.cwd.as_str(), true),
        Listing::Failed(ListError::Other(e)) => states::message(p, "Couldn't open this folder", e, true),
        // A new folder in an empty one: its row, under the header.
        Listing::Ready(entries) if entries.is_empty() => match edit::new_folder_row(s, c, p) {
            Some(new) => col().w_full().h_full().child(header(c, p)).child(col().w_full().py(space::S1).child(new)),
            None => empty_area(states::empty(p)),
        },
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
        .virtual_rows(c.row_count(), s.settings.density.row(), row_key, move |s: &App, i| match s.conn() {
            Some(c) if i < c.row_count() => file_row(s, c, i, p),
            _ => el().h(s.settings.density.row()),
        })
        .virtual_rows_version((c.host.as_str().to_owned(), c.rows_version));
    let mut content = col().w_full().py(space::S1).children(edit::new_folder_row(s, c, p)).child(stack);
    if let Some((n, name)) = s.files.scroll.clone() {
        // The folder just made or the item just renamed: brought into view once it lists.
        content = content.child(el().key(format!("scroll-req-{n}")).absolute(0.0, 0.0).size(0.0, 0.0).on_mount(move |s: &mut App, tx: &mut Tx| {
            tx.scroll_into_view_in(ROWS, name.clone());
            run(s, Action::File(FileAction::Scrolled(n)), tx);
        }));
    }
    let mut viewport = empty_area(col().key(LIST).w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec::default()).child(content));
    if let Some(name) = s.ui.center_on_mount.clone() {
        viewport = viewport.on_mount(move |s: &mut App, tx: &mut Tx| {
            s.ui.center_on_mount = None;
            // Lands the row a third of the way down, mounting it first if it isn't.
            tx.scroll_key_to(format!("{ROWS}/{name}"), 200.0);
        });
    }
    viewport
}

/// The list where there's no row: right-click opens New folder, Paste, Refresh, Show hidden.
fn empty_area(e: El) -> El {
    e.on_secondary_press(|s: &mut App, at, tx: &mut Tx| menu::open(s, MenuKind::Empty, at, tx))
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
    let target = RowTarget::Folder(e.name.clone());
    // A file drag (or a row drag) over this folder row: it lifts with an accent fill.
    let drop_here = open && (drop::over_folder(s, &e.name) || drag::over(s, &target));
    let (n1, n2, n3) = (e.name.clone(), e.name.clone(), e.name.clone());
    let name = match edit::renaming(s, c, e) {
        Some(f) => edit::field(f, s.files.edit.is_sending(), p),
        None => text(e.name.clone()).font(if selected { type_::BODY_STRONG } else { type_::BODY }).color(name_color).truncate().grow(1.0).min_w(0.0),
    };
    // Cut rows stay dimmed until they're pasted or the clip changes.
    let cut = s.files.clip.as_ref().is_some_and(|k| k.is_cut(&c.host, &c.cwd.join(&e.name)));
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
    let fresh = c.fresh.is_new(&e.name);
    let cells = cells
        .children(fresh.then(|| highlight(c.fresh.seq, c.fresh.seen, p)))
        .child(file_icon(p, e, selected || drop_here))
        .child(name)
        .child(size_cell)
        .child(modified);
    let mut r = row()
        .role("row")
        .label(e.name.clone())
        .row_index(i)
        .w_full()
        .h(s.settings.density.row())
        .px(space::S1)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let m = tx.mods();
            run(s, Action::Click { name: n1.clone(), extend: m.shift, toggle: m.cmd }, tx);
        })
        .on_secondary_press(move |s: &mut App, at, tx: &mut Tx| menu::open(s, MenuKind::Row(n3.clone()), at, tx))
        .child(cells);
    if cut {
        r = r.opacity(0.5);
    }
    if fresh {
        // A row a refresh brought slides and fades in as its space opens (§4.5); under
        // Reduce Motion Rune makes that a short fade.
        r = r.transition(Transition::offset(0.0, 8.0).combined(Transition::opacity())).transition_motion(motion::MOVE, motion::MOVE);
    }
    if edit::renaming(s, c, e).is_none() {
        r = drag::row(r, &e.name);
    }
    if open {
        r = r.on_double_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::Open(n2.clone()), tx));
        r = drop::folder_row(r, &e.name).data(target);
    }
    r
}

/// The mint wash under a row the last refresh brought (§4.5): it shows whole as the row
/// mounts, and once on screen (its `on_mount` marks the refresh seen) decays over
/// [`HIGHLIGHT`] seconds. Under Reduce Motion it's colour, not motion (§6.6): it holds, then
/// fades quickly at the end. Pointer input passes through it.
fn highlight(seq: u64, seen: bool, p: Paint) -> El {
    let fade: Anim = if p.reduced {
        Curve::tween(0.15, Easing::EASE_OUT).delay(HIGHLIGHT - 0.15)
    } else {
        Curve::tween(HIGHLIGHT, Easing::EASE_OUT).into()
    };
    el().key(format!("fresh-{seq}"))
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .radius(radius::ROW)
        // Lighter than the selection's fill, with an edge, so a new row never reads as selected.
        .fill(p.t.accent.alpha(0.1))
        .border(1.0, p.t.accent.alpha(0.55))
        .opacity(if seen { 0.0 } else { 1.0 })
        .opacity_motion(fade)
        .transition(Transition::none())
        .inert()
        .on_mount(move |s: &mut App, tx: &mut Tx| {
            if s.conn_mut().is_some_and(|c| c.fresh.shown(seq)) {
                tx.animated();
            }
        })
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
