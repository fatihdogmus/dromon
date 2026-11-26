//! The Properties sheet (`state::properties`): the row's icon and name as the title, then its
//! Kind, Location (the folder it's in, in mono), Size, Modified, Permissions and Owner in two
//! columns, labels in `text-3`. Read-only: one **Close** (the default button, so Return closes
//! it as Escape does).
//!
//! A folder's Size is a **Calculate** button until it's measured: then "Calculating… N items"
//! with a spinner while the walk runs, and the total after it (human size and exact bytes,
//! "N files, M folders", a note for folders that couldn't be read, **Recalculate**). A walk
//! that stopped says why, as an alert. Calculate is dimmed while the host isn't connected.

use rune::{El, LoopSpec, Tx, col, icon, row, text};

use super::list::file_icon;
use super::sheets::{button, buttons, frame};
use super::widgets::Keyboard;
use super::{Paint, run};
use crate::icons;
use crate::state::properties::{PropertiesView, SizePhase, calculating_text, counts_text, kind_text, owner_text, permissions_text, unreadable_text};
use crate::state::{Action, App, FileAction};
use crate::theme::{motion, radius, size, space, type_};

const WIDTH: f32 = 440.0;
/// The labels' column.
const LABEL_W: f32 = 96.0;

pub fn sheet(s: &App, v: &PropertiesView, p: Paint) -> El {
    let t = p.t;
    let e = &v.entry;
    let title = row()
        .key("prop-heading")
        .w_full()
        .gap(space::S2)
        .center_y()
        .child(file_icon(p, e, false))
        .child(text(e.name.clone()).key("sheet-title").font(type_::TITLE).color(t.text).grow(1.0).min_w(0.0).wrap_anywhere());
    let modified = e.mtime.map_or_else(|| "—".to_owned(), |m| crate::fmt::date_time(m, 0));
    let fields = col()
        .key("prop-fields")
        .w_full()
        .gap(space::S2)
        .padding(space::S3)
        .radius(radius::ROW)
        .fill(t.raised)
        .border(p.hair, t.border_subtle)
        .child(field(p, "kind", "Kind", value(p, "kind", kind_text(e).to_owned(), false)))
        .child(field(p, "location", "Location", value(p, "location", v.folder.to_string(), true)))
        .child(field(p, "size", "Size", size_value(s, v, p)))
        .child(field(p, "modified", "Modified", value(p, "modified", modified, false)))
        .child(field(p, "permissions", "Permissions", value(p, "permissions", permissions_text(e.mode), true)))
        .child(field(p, "owner", "Owner", value(p, "owner", owner_text(e), false)));
    frame(p, "properties", &format!("{} properties", e.name), WIDTH)
        .child(title)
        .child(fields)
        .child(buttons(vec![button(p, "prop-close", "Close", true, Action::DismissSheet)]))
}

/// One line: its label, then what it says.
fn field(p: Paint, key: &str, label: &'static str, value: El) -> El {
    row()
        .key(format!("prop-{key}"))
        .w_full()
        .gap(space::S3)
        .items_start()
        .child(text(label).key("label").font(type_::META).color(p.t.text_3).w(LABEL_W).pt(1.0))
        .child(col().key("value").grow(1.0).min_w(0.0).gap(space::S1).child(value))
}

fn value(p: Paint, key: &str, s: String, mono: bool) -> El {
    text(s).key(format!("prop-{key}-value")).font(if mono { type_::MONO } else { type_::BODY }).color(p.t.text).w_full().wrap_anywhere().selectable(true)
}

/// A file's size from the listing; a folder's as far as it's measured.
fn size_value(s: &App, v: &PropertiesView, p: Paint) -> El {
    let t = p.t;
    if !v.is_folder() {
        return value(p, "size", crate::fmt::size_exact(v.entry.size), false);
    }
    let calculate = |label: &'static str| calc_button(p, label, s.can_calculate());
    let lines = col().key("prop-size-lines").w_full().gap(space::S1).items_start();
    match &v.size {
        SizePhase::Idle => lines.child(calculate("Calculate")),
        SizePhase::Calculating { found, .. } => {
            let spinner = icon(icons::SPINNER.at(p.scale), size::ICON).color(t.text_2).looping(LoopSpec::rotation(0.9));
            let words = calculating_text(found);
            lines.child(
                row()
                    .key("prop-size-progress")
                    .role("status")
                    .label(words.clone())
                    .busy(true)
                    .gap(space::S2)
                    .center_y()
                    .child(spinner)
                    .child(text(words).font(type_::BODY).color(t.text_2)),
            )
        }
        SizePhase::Done { total } => {
            let mut lines = lines
                .child(value(p, "size", crate::fmt::size_exact(total.bytes), false))
                .child(text(counts_text(total)).key("prop-size-counts").font(type_::META).color(t.text_2));
            if let Some(note) = unreadable_text(total) {
                lines = lines.child(text(note).key("prop-size-note").role("status").font(type_::META).color(t.warning));
            }
            lines.child(calculate("Recalculate"))
        }
        SizePhase::Failed { error, .. } => lines
            .child(text(format!("Couldn't calculate: {error}")).key("prop-size-error").role("alert").font(type_::META).color(t.danger).w_full().wrap())
            .child(calculate("Recalculate")),
    }
}

/// Calculate or Recalculate: a small secondary button, dimmed while it can't start.
fn calc_button(p: Paint, label: &'static str, enabled: bool) -> El {
    let t = p.t;
    let b = row()
        .key("prop-calculate")
        .role("button")
        .label(label)
        .h(24.0)
        .px(space::S2)
        .mt(space::S1)
        .radius(radius::ROW)
        .center()
        .border(p.hair, t.border);
    if !enabled {
        return b.disabled(true).child(text(label).font(type_::META).color(t.text_3));
    }
    b.hover(t.hover)
        .press(t.press)
        .press_scale(0.97)
        .press_motion(rune::Anim::INSTANT, motion::SNAP)
        .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::File(FileAction::Calculate), tx))
        .tab_stop()
        .child(text(label).font(type_::META_STRONG).color(t.text))
}
