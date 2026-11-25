//! The Add to SSH config sheet (§3.8, §6.8 "Add to SSH config", `state::hostbook`): the target
//! file's path, the exact `Host` block in mono on `raised`, the backup note, **Add to config**
//! (the default: Return) and Cancel. A problem shows in the sheet; while the service works the
//! button is busy.

use rune::{El, LoopSpec, col, icon, row, text};

use super::Paint;
use super::sheets::{button, buttons, frame};
use crate::icons;
use crate::state::hostbook::AddToConfigView;
use crate::state::{Action, App, HostAction};
use crate::theme::{radius, size, space, type_};

const WIDTH: f32 = 480.0;

pub fn sheet(_s: &App, v: &AddToConfigView, p: Paint) -> El {
    let t = p.t;
    let title = format!("Add {} to your SSH config", v.name);
    let mut panel = frame(p, "add-config", &title, WIDTH).child(text(title.clone()).key("sheet-title").font(type_::TITLE).color(t.text).w_full().wrap_anywhere());
    let block = match &v.preview {
        Some(preview) => {
            let lead = row()
                .w_full()
                .gap(space::S1)
                .child(text("These lines go at the end of").font(type_::BODY).color(t.text_2))
                .child(text(preview.target.clone()).key("add-config-target").font(type_::MONO).color(t.text));
            panel = panel.child(lead);
            text(preview.block.trim_end().to_owned()).key("add-config-block-text").font(type_::MONO).color(t.text).w_full().wrap_anywhere().selectable(true)
        }
        None => text("Reading your ssh config…").key("add-config-loading").font(type_::BODY).color(t.text_3),
    };
    panel = panel.child(
        col().key("add-config-block").w_full().padding(space::S3).radius(radius::ROW).fill(t.raised).border(p.hair, t.border_subtle).child(block),
    );
    if let Some(preview) = &v.preview {
        let note = match &preview.backup {
            Some(b) => col()
                .w_full()
                .gap(2.0)
                .child(text("A backup will be saved to").font(type_::META).color(t.text_3))
                .child(text(b.clone()).key("add-config-backup").font(type_::MONO).color(t.text_2).w_full().wrap_anywhere()),
            None => col().child(text("The file will be created.").key("add-config-backup").font(type_::META).color(t.text_3)),
        };
        panel = panel.child(note.key("add-config-note"));
    }
    if let Some(e) = &v.error {
        panel = panel.child(text(e.clone()).key("add-config-error").role("alert").font(type_::META).color(t.danger).w_full().wrap_anywhere());
    }
    let primary = if v.busy {
        busy(p)
    } else if v.preview.is_some() {
        button(p, "add-config-confirm", "Add to config", true, Action::HostBook(HostAction::ConfirmAddToConfig))
    } else {
        button(p, "add-config-confirm", "Add to config", false, Action::HostBook(HostAction::ConfirmAddToConfig)).disabled(true)
    };
    panel.child(buttons(vec![button(p, "add-config-cancel", "Cancel", false, Action::DismissSheet), primary]))
}

/// The primary button while the service writes.
fn busy(p: Paint) -> El {
    let t = p.t;
    let spinner = icon(icons::SPINNER.at(p.scale), size::ICON).color(t.on_accent).looping(LoopSpec::rotation(0.9));
    row()
        .key("add-config-busy")
        .role("progressbar")
        .label("Adding")
        .busy(true)
        .h(28.0)
        .px(space::S3)
        .gap(6.0)
        .radius(radius::ROW)
        .center()
        .fill(t.accent.alpha(0.6))
        .child(spinner)
        .child(text("Adding…").font(type_::BODY_STRONG).color(t.on_accent))
}
