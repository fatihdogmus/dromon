//! The delete sheet (§6.8 "Delete confirm"): "Delete 3 items permanently?", the first five
//! names (then "and 12 more"), "This can't be undone on the server.", **Delete** (danger) and
//! Cancel. Delete isn't the default button: Return doesn't delete, Escape cancels.

use rune::{El, col, icon, row, text};

use super::conflict::danger;
use super::sheets::{button, buttons, frame};
use super::Paint;
use crate::icons;
use crate::state::files::{DELETE_NAMES, delete_more, delete_title};
use crate::state::{Action, App, FileAction};
use crate::theme::{radius, size, space, type_};

const WIDTH: f32 = 440.0;

pub fn sheet(s: &App, names: &[String], p: Paint) -> El {
    let t = p.t;
    let title = delete_title(names);
    let rows = names.iter().take(DELETE_NAMES).enumerate().map(|(i, name)| {
        let glyph = s.conn().and_then(|c| c.find(name)).map_or(&icons::FILE, |e| if e.kind.is_dir_like() { &icons::FOLDER } else { icons::for_file(name).0 });
        row()
            .key(format!("delete-name-{i}"))
            .gap(space::S2)
            .center_y()
            .child(icon(glyph.at(p.scale), size::ICON).color(t.text_2))
            .child(text(name.clone()).font(type_::BODY).color(t.text).truncate().shrink())
    });
    let mut list = col().key("delete-names").w_full().gap(space::S1).padding(space::S3).radius(radius::ROW).fill(t.raised).border(p.hair, t.border_subtle).children(rows);
    if let Some(more) = delete_more(names) {
        list = list.child(text(more).key("delete-more").font(type_::META).color(t.text_3));
    }
    frame(p, "delete", &title, WIDTH)
        .child(text(title.clone()).key("sheet-title").font(type_::TITLE).color(t.text).w_full().wrap_anywhere())
        .child(list)
        .child(text("This can't be undone on the server.").key("delete-note").font(type_::BODY).color(t.text_2))
        .child(buttons(vec![
            button(p, "delete-cancel", "Cancel", false, Action::DismissSheet),
            danger(p, "delete-confirm", "Delete", Action::File(FileAction::ConfirmDelete)),
        ]))
}
