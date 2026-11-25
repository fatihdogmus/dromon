//! The quit sheet (§4.6 "Quitting during active transfers asks"): the window was asked to close
//! while transfers run. **Cancel** (the default: nothing is lost) or **Quit** (danger). It comes
//! before every other sheet; Escape or a click outside cancels.

use rune::{El, row, text};

use super::Paint;
use super::conflict::danger;
use super::sheets::{button, buttons, frame};
use crate::state::{Action, App};
use crate::theme::type_;

const WIDTH: f32 = 440.0;

/// "1 transfer in progress" / "3 transfers in progress".
pub fn title(active: usize) -> String {
    let n = crate::fmt::thousands(active as u64);
    if active == 1 { format!("{n} transfer in progress") } else { format!("{n} transfers in progress") }
}

pub fn sheet(s: &App, p: Paint) -> Option<El> {
    if !s.ui.confirm_quit {
        return None;
    }
    let t = p.t;
    let heading = title(s.transfers.active());
    let body = "Quit anyway? They stop now. Files that were part-way through keep their partial \
                copies (.dromon-part), and finished ones stay where they are.";
    let actions = vec![
        danger(p, "quit-anyway", "Quit", Action::QuitAnyway),
        row().grow(1.0),
        button(p, "quit-cancel", "Cancel", true, Action::CancelQuit),
    ];
    Some(
        frame(p, "quit", &heading, WIDTH)
            .child(text(heading.clone()).key("sheet-title").font(type_::TITLE).color(t.text))
            .child(text(body).key("quit-explain").font(type_::BODY).color(t.text_2).w_full().wrap())
            .child(buttons(actions)),
    )
}
