//! The ⌘K palette (§6.8 "⌘K palette", `state::palette`): 560 pt wide, its top a third of the
//! way down the window, on the `sheet` surface with sheet elevation and a 12 pt radius. A search
//! field (autofocused) over the rows grouped as Hosts, Go to folder… and Actions; each row shows
//! its shortcut on the right as the menus do (`widgets::chord`), and the characters the query
//! matched in its title are bold. It scales in from its centre on the `move` spring (§6.6); under
//! Reduce Motion it only fades, in 150 ms. Modal: Escape or a press outside closes it.
//!
//! Keys: `palette`, `palette-input`, `palette-list`, `palette-group-<hosts|folders|actions>`,
//! rows `pal-host-<id>`, `pal-go`, `pal-folder-<path>`, `pal-<action>`; `palette-empty`.

use rune::{
    Anim, AutoComplete, Curve, Cx, Easing, El, Key, KeyEvent, ScrollSpec, Span, TextStyle, Transition, Tx, col, el,
    icon, row, text, text_input,
};

use super::widgets::{chord, kbd};
use super::{Paint, run};
use crate::icons::{self, IconPair};
use crate::state::palette::{Group, Item, Mode};
use crate::state::{Action, App, PaletteAction};
use crate::theme::{motion, radius, size, space, type_};

pub const WIDTH: f32 = 560.0;
/// The most the rows take before they scroll: about nine rows and their headings.
const LIST_MAX: f32 = 340.0;
const ROW: f32 = 32.0;
/// The query: a step up from body text, as a search field that is the whole dialog.
const QUERY: TextStyle = TextStyle::new(15.0, 400.0).lh(20.0);

fn act(s: &mut App, a: PaletteAction, tx: &mut Tx) {
    run(s, Action::Palette(a), tx);
}

/// Keys the field hands to the palette before its editor sees them.
fn field_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let a = match ev.key {
        Key::Escape => PaletteAction::Close,
        Key::Up => PaletteAction::Move(-1),
        Key::Down => PaletteAction::Move(1),
        Key::Enter => PaletteAction::Run,
        _ => return false,
    };
    let moved = matches!(a, PaletteAction::Move(_));
    act(s, a, tx);
    if moved && let Some(item) = s.palette_items().get(s.ui.palette.cursor) {
        tx.scroll_into_view(item.key.clone());
    }
    true
}

/// The palette, while it's open and nothing modal is over it; `top` is [`top`]'s.
pub fn layer(s: &App, p: Paint, top: Option<f32>) -> Option<El> {
    let mode = s.ui.palette.mode()?;
    if s.ui.sheets.is_open() || s.ui.confirm_quit || s.conflict_on_screen().is_some() {
        return None;
    }
    let t = p.t;
    let items = s.palette_items();
    let cursor = s.ui.palette.cursor.min(items.len().saturating_sub(1));
    let (label, placeholder, glyph) = match mode {
        Mode::Commands => ("Command palette", "Search hosts, folders and actions", &icons::SEARCH),
        Mode::Path => ("Go to folder", "A folder's path: /var/www, ~/logs, ../releases", &icons::FOLDER_OPEN),
    };
    let input = text_input(&s.ui.palette.edit, QUERY)
        .key("palette-input")
        .label(placeholder)
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .color(t.text)
        .placeholder(placeholder, t.text_3)
        .auto_complete(AutoComplete::List)
        .controls("palette-list")
        .active_descendant(items.get(cursor).map(|i| i.key.clone()).unwrap_or_default())
        .on_edit(|s: &mut App| &mut s.ui.palette.edit)
        .on_input_change(|s: &mut App, tx: &mut Tx| act(s, PaletteAction::Edited, tx))
        .on_input_key(field_key);
    let field = row()
        .key("palette-field")
        .w_full()
        .h(48.0)
        .px(space::S4)
        .gap(space::S3)
        .center_y()
        // The rule under the field, inside its 48 pt, so the rows below start on a whole point.
        .border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle)
        .child(icon(glyph.at(p.scale), size::ICON).color(t.text_3))
        .child(input);
    let body = if items.is_empty() { empty(s, mode, p) } else { list(&items, cursor, p) };
    let (enter, motion_in, motion_out): (Transition, Anim, Anim) = if p.reduced {
        let fade = Curve::tween(0.15, Easing::EASE_OUT);
        (Transition::opacity(), fade.into(), fade.into())
    } else {
        (Transition::scale(0.96).combined(Transition::opacity()), motion::MOVE.into(), motion::MOVE.into())
    };
    let panel = col()
        .key("palette")
        .role("dialog")
        .label(label)
        .w(WIDTH)
        .min_h(0.0)
        .shrink()
        .radius(radius::SHEET)
        .fill(t.sheet)
        .border(p.hair, t.border)
        .shadow(16.0, 48.0, t.sheet_shadow)
        .transition(enter)
        .transition_motion(motion_in, motion_out)
        .modal_tx(true, |s: &mut App, tx: &mut Tx| act(s, PaletteAction::Close, tx))
        .child(field)
        .child(body);
    // The top third stays clear (to a whole point, so the rows' icons sit on the pixel grid);
    // the panel hangs from there and shrinks to what's left above a 24 pt margin.
    let top = match top {
        Some(y) => el().h(y),
        // Before the window's first layout: a third by flex, 12 + (H − 12 − 24) / 3 = H / 3.
        None => el().pt(space::S6 / 2.0).grow(1.0),
    };
    Some(
        col()
            .key("palette-layer")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .child(top.key("palette-top").w_full().a11y_hidden())
            .child(
                col()
                    .key("palette-room")
                    .w_full()
                    .grow(2.0)
                    .min_h(0.0)
                    .px(space::S6)
                    .pb(space::S6)
                    .items_center()
                    .child(panel),
            ),
    )
}

/// Where the open palette's top goes: a third of the way down the window, at a whole point.
/// Read only while it's open, so window resizes don't re-run the view otherwise.
pub fn top(s: &App, cx: &Cx<App>) -> Option<f32> {
    if !s.ui.palette.is_open() {
        return None;
    }
    cx.layout_with("window", |r| (r[3] / 3.0).round())
}

/// No rows: what to type instead.
fn empty(s: &App, mode: Mode, p: Paint) -> El {
    let line = match mode {
        Mode::Path if s.ui.palette.query().is_empty() => "Type a path on this host".to_owned(),
        _ => format!("Nothing matches “{}”", s.ui.palette.query()),
    };
    row()
        .key("palette-empty")
        .role("status")
        .w_full()
        .h(44.0)
        .px(space::S4)
        .center_y()
        .child(text(line).font(type_::BODY).color(p.t.text_3).truncate())
}

/// The rows by group, scrolling past [`LIST_MAX`].
fn list(items: &[Item], cursor: usize, p: Paint) -> El {
    let mut groups: Vec<(Group, Vec<El>)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let r = item_row(item, i, i == cursor, items.len(), p);
        match groups.last_mut() {
            Some((g, rows)) if *g == item.group => rows.push(r),
            _ => groups.push((item.group, vec![r])),
        }
    }
    let groups = groups.into_iter().map(|(g, rows)| {
        let heading = row()
            .h(24.0)
            .px(space::S2)
            .pt(space::S1)
            .center_y()
            .a11y_hidden()
            .child(text(g.title().to_uppercase()).font(type_::CAPTION).color(p.t.text_3));
        col().key(g.key()).role("group").label(g.title()).w_full().child(heading).children(rows)
    });
    col()
        .key("palette-list")
        .role("listbox")
        .label("Results")
        .w_full()
        .max_h(LIST_MAX)
        .min_h(0.0)
        .shrink()
        .scroll(ScrollSpec::default())
        .px(space::S2)
        .pt(space::S1)
        .pb(space::S2)
        .children(groups)
}

/// The glyph in front of a row.
fn glyph(item: &Item) -> &'static IconPair {
    match item.group {
        Group::Hosts => &icons::SERVER,
        Group::Folders => &icons::FOLDER,
        Group::Actions => match item.key.as_str() {
            "pal-quick-connect" => &icons::PLUS,
            "pal-go-to-folder" => &icons::FOLDER_OPEN,
            "pal-filter" => &icons::SEARCH,
            "pal-refresh" => &icons::RETRY,
            "pal-new-folder" => &icons::FOLDER,
            "pal-download" | "pal-show-transfers" => &icons::DOWNLOAD,
            "pal-rename" => &icons::FILE_TEXT,
            "pal-delete" | "pal-disconnect" => &icons::CLOSE,
            "pal-log" => &icons::LOG,
            "pal-clear-finished" => &icons::CHECK,
            "pal-settings" => &icons::GEAR,
            "pal-report-bug" => &icons::WARNING,
            _ => &icons::FILE,
        },
    }
}

fn item_row(item: &Item, i: usize, current: bool, total: usize, p: Paint) -> El {
    let t = p.t;
    let spans: Vec<Span> = item.hits.iter().map(|&(a, b)| Span::new(a, b).weight(600.0)).collect();
    let title = text(item.title.clone())
        .key("title")
        .font(type_::BODY)
        .color(t.text)
        .spans(spans)
        .truncate()
        .shrink()
        .min_w(0.0);
    let mut r = row()
        .key(item.key.clone())
        .role("option")
        .label(item.title.clone())
        .position_in_set(i + 1, total)
        .w_full()
        .h(ROW)
        .px(space::S2)
        .gap(space::S3)
        .radius(radius::ROW)
        .center_y()
        .fill(if current { t.selection } else { rune::Color::TRANSPARENT })
        .hover(t.hover)
        .press(t.press)
        .motion(motion::SNAP)
        .child(icon(glyph(item).at(p.scale), size::ICON).color(if item.group == Group::Folders {
            t.folder
        } else {
            t.text_2
        }))
        .child(title);
    if let Some(d) = &item.detail {
        r = r
            .description(d.clone())
            .child(text(d.clone()).key("detail").font(type_::META).color(t.text_3).truncate().shrink().min_w(0.0));
    }
    r = r.child(el().grow(1.0));
    if let Some(spec) = item.shortcut {
        let label = chord(spec);
        r = r.shortcut_hint(label.clone()).child(kbd(p, &label).key("shortcut"));
    }
    let key = item.key.clone();
    r.on_press_tx(move |s: &mut App, tx: &mut Tx| act(s, PaletteAction::Pick(key.clone()), tx))
}
