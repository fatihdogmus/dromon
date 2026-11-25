//! Dragging rows within the window (`state::drag`). A row starts the drag once the pointer
//! travels 4 pt from the press; each move hit-tests what's under the pointer, and targets say
//! what they are with `El::data(DragTarget)`: folder rows, breadcrumbs, sidebar hosts. A target
//! that takes the rows lifts with an accent fill (§6.8 "Drop target") and the pill names it.
//! ⌥ (Ctrl on Windows and Linux) copies instead of moving. Escape cancels (Rune ends the
//! gesture cancelled).

use rune::{Drag, El, KeyPlatform, Mods, Phase, Tx};

use super::{Paint, drop, run};
use crate::icons;
use crate::state::drag::DragTarget;
use crate::state::{Action, App, FileAction};

/// How far the pointer travels before a press on a row becomes a drag.
const START: f32 = 4.0;

/// Whether the copy modifier is held: ⌥ on macOS, Ctrl elsewhere.
pub fn copy_held(m: Mods) -> bool {
    if KeyPlatform::current() == KeyPlatform::Mac { m.alt } else { m.ctrl }
}

fn act(s: &mut App, a: FileAction, tx: &mut Tx) {
    run(s, Action::File(a), tx);
}

/// Hit-tests window point (`x`, `y`) for a drag target and reports it; `drop_it` drops there.
fn hover_at(tx: &mut Tx, x: f32, y: f32, copy: bool, drop_it: bool) {
    tx.element_at(x, y, move |s: &mut App, picked, tx| {
        let target = picked.and_then(|p| p.data::<DragTarget>().cloned());
        act(s, FileAction::DragOver { target, copy }, tx);
        if drop_it {
            act(s, FileAction::DragDrop, tx);
        }
    });
}

/// Makes row `name` draggable.
pub fn row(r: El, name: &str) -> El {
    let name = name.to_owned();
    r.on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
        let [x, y] = d.window_point();
        let copy = copy_held(tx.mods());
        match d.phase {
            Phase::Down => {}
            Phase::Move => {
                if s.files.drag.is_none() {
                    if d.dx.hypot(d.dy) < START {
                        return;
                    }
                    act(s, FileAction::DragStart(name.clone()), tx);
                }
                if s.files.drag.is_some() {
                    hover_at(tx, x, y, copy, false);
                }
            }
            Phase::Up => {
                if d.cancelled() || s.files.drag.is_none() {
                    act(s, FileAction::DragCancel, tx);
                } else {
                    hover_at(tx, x, y, copy, true);
                }
            }
        }
    })
}

/// Whether the drag is over `target` and it takes the rows.
pub fn over(s: &App, target: &DragTarget) -> bool {
    s.files.drag.as_ref().and_then(|d| d.over.as_ref()).is_some_and(|o| o.target == *target)
}

/// "Move 3 items to www", at the bottom of the list while the drag is over a target.
pub fn pill(s: &App, p: Paint) -> Option<El> {
    let d = s.files.drag.as_ref()?;
    let label = d.pill()?;
    let glyph = match d.over.as_ref().map(|o| &o.target) {
        Some(DragTarget::Host(_)) => &icons::SERVER,
        _ => &icons::FOLDER,
    };
    // The same pill as a file drag's (`drop-pill`): the two never show at once.
    Some(drop::pill_row("drop", label, glyph, p))
}
