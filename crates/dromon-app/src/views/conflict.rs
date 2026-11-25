//! The conflict sheet (§3.6, §6.8): an upload's name is taken in its folder. **Replace**
//! (danger), **Skip**, or **Keep both** (the default: nothing is lost), and "Apply to all N
//! conflicts" when the job has more. Shown while the oldest clash waits, no prompt sheet is up
//! and the transfers popover is closed (`App::conflict_on_screen`); it's modal like the others,
//! but there's nothing to cancel, so Escape does nothing.
//!
//! The sizes and dates of both items aren't shown yet: the engine's `ConflictView` doesn't
//! carry them.

use dromon_transfer::{ConflictChoice, ConflictView, unique_name};
use rune::{El, Tx, el, icon, row, text};

use super::sheets::{button, buttons, frame};
use super::{Paint, run};
use crate::icons;
use crate::state::{Action, App};
use crate::theme::{motion, radius, size, space, type_};

/// Sheet width, as the other sheets.
const WIDTH: f32 = 440.0;

/// "‘index.html’ already exists in /var/www".
pub fn title(c: &ConflictView) -> String {
    format!("‘{}’ already exists in {}", c.name, c.folder)
}

/// What each answer does, with the name Keep both would pick if nothing else took it.
pub fn explain(c: &ConflictView) -> String {
    let copy = unique_name(&c.name, |n| n == c.name);
    if c.existing_is_dir {
        format!(
            "Replace merges the folders: files with the same names inside are overwritten. \
             Keep both uploads yours under a new name, like “{copy}”."
        )
    } else {
        format!("Replace overwrites it. Keep both uploads yours under a new name, like “{copy}”.")
    }
}

/// The checkbox's label: N is this clash and the ones after it.
pub fn apply_all_label(c: &ConflictView) -> Option<String> {
    (c.more > 0).then(|| format!("Apply to all {} conflicts", crate::fmt::thousands(c.more as u64 + 1)))
}

pub fn sheet(s: &App, p: Paint) -> Option<El> {
    let (_, c) = s.conflict_on_screen()?;
    let t = p.t;
    let heading = title(c);
    let mut panel = frame(p, "conflict", &heading, WIDTH)
        .child(text(heading.clone()).key("sheet-title").font(type_::TITLE).color(t.text).w_full().wrap_anywhere())
        .child(text(explain(c)).key("conflict-explain").font(type_::BODY).color(t.text_2).w_full().wrap());
    if let Some(label) = apply_all_label(c) {
        panel = panel.child(checkbox(p, s.transfers.apply_all, label));
    }
    let replace = danger(p, "conflict-replace", "Replace", Action::ResolveConflict(ConflictChoice::Replace));
    let actions = vec![
        replace,
        row().grow(1.0),
        button(p, "conflict-skip", "Skip", false, Action::ResolveConflict(ConflictChoice::Skip)),
        button(p, "conflict-keep-both", "Keep both", true, Action::ResolveConflict(ConflictChoice::KeepBoth)),
    ];
    Some(panel.child(buttons(actions)))
}

/// A button for what can't be undone: the danger colour on its label and outline.
fn danger(p: Paint, key: &'static str, label: &'static str, action: Action) -> El {
    let t = p.t;
    row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .px(space::S3)
        .radius(radius::ROW)
        .center()
        .border(p.hair, t.danger.alpha(0.55))
        .hover(t.danger.alpha(0.08))
        .press(t.danger.alpha(0.14))
        .press_scale(0.97)
        .press_motion(rune::Anim::INSTANT, motion::SNAP)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, action.clone(), tx))
        .child(text(label).font(type_::BODY_STRONG).color(t.danger))
}

fn checkbox(p: Paint, on: bool, label: String) -> El {
    let t = p.t;
    let mut bx = el().size(size::ICON, size::ICON).radius(4.0).center().motion(motion::SNAP);
    bx = if on {
        bx.fill(t.accent).child(icon(icons::CHECK.at(p.scale), size::ICON).color(t.on_accent))
    } else {
        bx.border(1.0, t.border_strong)
    };
    row()
        .key("conflict-apply-all")
        .role("checkbox")
        .label(label.clone())
        .checked(on)
        .gap(space::S2)
        .center_y()
        .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::ToggleApplyAll, tx))
        .child(bx)
        .child(text(label).font(type_::BODY).color(t.text))
}
