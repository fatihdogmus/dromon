//! The inline name field (`state::edit`): a new folder's row at the top of the list, or the
//! name cell of a row being renamed. An accent-outlined box around a single-line field; a
//! problem with the name shows in a small danger bubble under it and the box shakes once (as
//! the sign-in sheet's field does). While the request runs the field is read-only.

use dromon_core::Entry;
use rune::{Beat, El, Key, KeyEvent, Sequence, Tx, el, icon, row, text, text_input};

use super::{Paint, run};
use crate::icons;
use crate::state::edit::{EditKind, Field};
use crate::state::{Action, App, ConnView, FileAction};
use crate::theme::{radius, size, space, type_};

/// The field's element key: list keys skip it, and its blur checks focus went elsewhere.
pub const INPUT: &str = "edit-input";

/// One shake, about a third of a second, side to side (the sign-in sheet's).
const SHAKE: &[Beat] = &[
    Beat::new(0.0),
    Beat::new(0.06).translate(-6.0, 0.0),
    Beat::new(0.12).translate(5.0, 0.0),
    Beat::new(0.18).translate(-3.0, 0.0),
    Beat::new(0.24).translate(2.0, 0.0),
    Beat::new(0.32),
];

fn act(s: &mut App, a: FileAction, tx: &mut Tx) {
    run(s, Action::File(a), tx);
}

fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Escape {
        act(s, FileAction::EditCancel, tx);
        return true;
    }
    false
}

/// The field for `f`, filling the name cell; read-only while its request runs (`sending`).
pub fn field(f: &Field, sending: bool, p: Paint) -> El {
    let t = p.t;
    let input = text_input(&f.edit, type_::BODY)
        .key(INPUT)
        .label(if f.renaming().is_some() { "New name" } else { "Folder name" })
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .color(t.text)
        .on_edit(|s: &mut App| s.files.field_edit())
        .on_input_change(|s: &mut App, tx: &mut Tx| act(s, FileAction::EditChanged, tx))
        .on_submit(|s: &mut App, tx: &mut Tx| act(s, FileAction::EditSubmit, tx))
        .on_input_key(key)
        .on_focus_change(|s: &mut App, focused: bool, tx: &mut Tx| {
            // A remount (the shake) hands focus straight back to the same field: not a blur.
            if !focused && !tx.focus_within(INPUT) {
                act(s, FileAction::EditBlur, tx);
            }
        })
        .read_only(sending);
    let error = f.error.is_some();
    let mut boxed = row()
        .key(format!("edit-field-{}", f.shakes))
        .grow(1.0)
        .min_w(0.0)
        .h(22.0)
        .px(space::S1)
        .ml(-space::S1)
        .radius(radius::ROW - 2.0)
        .center_y()
        .fill(t.canvas)
        .border(1.5, if error { t.danger } else { t.accent })
        .child(input);
    if f.shakes > 0 {
        boxed = boxed.sequence(Sequence::new(SHAKE));
    }
    let Some(message) = &f.error else { return boxed };
    let bubble = row()
        .key("edit-error")
        .role("alert")
        .label(message.clone())
        .absolute(0.0, 25.0)
        .px(space::S2)
        .py(space::S1)
        .radius(radius::ROW)
        .fill(t.sheet)
        .border(p.hair, t.danger.alpha(0.6))
        .shadow(4.0, 12.0, t.shadow)
        .raise()
        .child(text(message.clone()).key("edit-error-text").font(type_::META).color(t.danger));
    // Under the box, over the next row.
    boxed.child(bubble)
}

/// Whether the field is a rename of row `e` on screen.
pub fn renaming<'a>(s: &'a App, c: &ConnView, e: &Entry) -> Option<&'a Field> {
    s.files.edit.field().filter(|f| f.host == c.host && f.folder == c.cwd && f.renaming() == Some(e.name.as_str()))
}

/// The new folder's row at the top of the list, while its field is up for this folder.
pub fn new_folder_row(s: &App, c: &ConnView, p: Paint) -> Option<El> {
    let f = s.files.edit.field().filter(|f| f.host == c.host && f.folder == c.cwd && f.kind == EditKind::NewFolder)?;
    let t = p.t;
    let glyph = el().size(size::ICON, size::ICON).child(icon(icons::FOLDER.at(p.scale), size::ICON).color(t.folder));
    let cells = row()
        .w_full()
        .h_full()
        .pl(space::S2)
        .pr(space::S3)
        .gap(space::S2)
        .center_y()
        .radius(radius::ROW)
        .fill(t.selection)
        .child(glyph)
        .child(field(f, s.files.edit.is_sending(), p))
        .child(text("—").font(type_::META).color(t.text_3).w(size::COL_SIZE).align(rune::Align::End))
        .child(el().w(size::COL_MODIFIED));
    Some(row().key("new-folder").role("row").label("New folder").w_full().h(s.settings.density.row()).px(space::S1).child(cells))
}
