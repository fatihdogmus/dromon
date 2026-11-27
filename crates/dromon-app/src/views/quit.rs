//! The quit sheet (§4.6 "Quitting during active transfers asks"): the window was asked to close
//! while transfers run. **Cancel** (the default: nothing is lost) or **Quit** (danger). It comes
//! before every other sheet; Escape or a click outside cancels. Update with transfers running
//! asks the same (`state/update.rs`): **Cancel** (the default) or **Update** (danger).

use rune::{El, row, text};

use super::Paint;
use super::conflict::danger;
use super::sheets::{button, buttons, frame};
use crate::state::update::UpdateAction;
use crate::state::{Action, App};
use crate::theme::type_;

const WIDTH: f32 = 440.0;

/// "1 transfer in progress" / "3 transfers in progress".
pub fn title(active: usize) -> String {
    let n = crate::fmt::thousands(active as u64);
    if active == 1 { format!("{n} transfer in progress") } else { format!("{n} transfers in progress") }
}

/// What stopping the transfers does to them, after "Quit anyway?" or "Update anyway?".
const STOPS: &str = "They stop now. Files that were part-way through keep their partial copies \
                     (.dromon-part), and finished ones stay where they are.";

pub fn sheet(s: &App, p: Paint) -> Option<El> {
    let (body, actions) = if s.ui.confirm_quit {
        let actions = vec![
            danger(p, "quit-anyway", "Quit", Action::QuitAnyway),
            row().grow(1.0),
            button(p, "quit-cancel", "Cancel", true, Action::CancelQuit),
        ];
        (format!("Quit anyway? {STOPS}"), actions)
    } else if s.update.confirming() {
        // Updating quits too (state/update.rs): the same question, as the update's.
        let actions = vec![
            danger(p, "update-anyway", "Update", Action::Update(UpdateAction::Confirm)),
            row().grow(1.0),
            button(p, "update-cancel", "Cancel", true, Action::Update(UpdateAction::Cancel)),
        ];
        (format!("Update and restart Dromon anyway? {STOPS}"), actions)
    } else {
        return None;
    };
    let t = p.t;
    let heading = title(s.transfers.active());
    Some(
        frame(p, "quit", &heading, WIDTH)
            .child(text(heading.clone()).key("sheet-title").font(type_::TITLE).color(t.text))
            .child(text(body).key("quit-explain").font(type_::BODY).color(t.text_2).w_full().wrap())
            .child(buttons(actions)),
    )
}
