//! The quit sheet (§4.6 "Quitting during active transfers asks"): the window was asked to close
//! while transfers run, or while a file being edited has a change that isn't on the server yet
//! (an upload running, failed or waiting on its conflict sheet). **Cancel** (the default:
//! nothing is lost) or **Quit** (danger). It comes before every other sheet; Escape or a click
//! outside cancels. Update asks the same in the same cases (`state/update.rs`): **Cancel** (the
//! default) or **Update** (danger).

use rune::{El, row, text};

use super::Paint;
use super::conflict::danger;
use super::sheets::{button, buttons, frame};
use crate::state::update::UpdateAction;
use crate::state::{Action, App};
use crate::theme::type_;

const WIDTH: f32 = 440.0;

/// "1 transfer in progress" / "3 transfers in progress"; "1 edited file isn't saved yet" when
/// only edits wait; both together, "2 transfers in progress, 1 edited file not saved".
pub fn title(transfers: usize, edits: usize) -> String {
    let n = |k: usize| crate::fmt::thousands(k as u64);
    let moving = if transfers == 1 {
        format!("{} transfer in progress", n(1))
    } else {
        format!("{} transfers in progress", n(transfers))
    };
    let files = if edits == 1 { "edited file" } else { "edited files" };
    match (transfers, edits) {
        (_, 0) => moving,
        (0, 1) => format!("{} {files} isn't saved yet", n(1)),
        (0, _) => format!("{} {files} aren't saved yet", n(edits)),
        _ => format!("{moving}, {} {files} not saved", n(edits)),
    }
}

/// What stopping the transfers does to them, after "Quit anyway?" or "Update anyway?".
const STOPS: &str = "They stop now. Files that were part-way through keep their partial copies \
                     (.dromon-part), and finished ones stay where they are.";

/// What quitting does to edits whose change isn't on the server yet.
const EDITS: &str = "Changes to files being edited that aren't on the server yet are lost with their local copies.";

/// What stops: the transfers, the edits, or both.
fn stops(transfers: usize, edits: usize) -> String {
    match (transfers, edits) {
        (_, 0) => STOPS.to_owned(),
        (0, _) => EDITS.to_owned(),
        _ => format!("{STOPS} {EDITS}"),
    }
}

pub fn sheet(s: &App, p: Paint) -> Option<El> {
    let (transfers, edits) = (s.transfers.active(), s.edits.unsaved());
    let (body, actions) = if s.ui.confirm_quit {
        let actions = vec![
            danger(p, "quit-anyway", "Quit", Action::QuitAnyway),
            row().grow(1.0),
            button(p, "quit-cancel", "Cancel", true, Action::CancelQuit),
        ];
        (format!("Quit anyway? {}", stops(transfers, edits)), actions)
    } else if s.update.confirming() {
        // Updating quits too (state/update.rs): the same question, as the update's.
        let actions = vec![
            danger(p, "update-anyway", "Update", Action::Update(UpdateAction::Confirm)),
            row().grow(1.0),
            button(p, "update-cancel", "Cancel", true, Action::Update(UpdateAction::Cancel)),
        ];
        (format!("Update and restart Dromon anyway? {}", stops(transfers, edits)), actions)
    } else {
        return None;
    };
    let t = p.t;
    let heading = title(transfers, edits);
    Some(
        frame(p, "quit", &heading, WIDTH)
            .child(text(heading.clone()).key("sheet-title").font(type_::TITLE).color(t.text))
            .child(text(body).key("quit-explain").font(type_::BODY).color(t.text_2).w_full().wrap())
            .child(buttons(actions)),
    )
}
