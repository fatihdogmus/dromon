//! The toast stack, bottom centre above the status bar: ssh's notices first, then the app's
//! toasts (`state::toast`), oldest on top. Each rises 8 pt as it fades in and leaves the same
//! way (`move`). Running ones show a spinner; failures a warning glyph and a dismiss button.

use rune::{El, LoopSpec, Transition, Tx, col, el, icon, row, text};

use super::widgets::Keyboard;
use super::{Paint, run, sheets};
use crate::icons;
use crate::state::toast::{Toast, Tone};
use crate::state::{Action, App, FileAction};
use crate::theme::{motion, radius, size, space, type_};

/// Every toast is this wide (longer text wraps): a whole width keeps a centred toast, and the
/// glyph at its start, on whole points.
const WIDTH: f32 = 360.0;

pub fn layer(s: &App, p: Paint) -> Option<El> {
    let notices = sheets::notices(s, p);
    let toasts = s.toasts();
    if notices.is_none() && toasts.is_empty() {
        return None;
    }
    Some(
        col()
            .key("toasts")
            .pin(Some(0.0), None, Some(0.0), Some(size::STATUS + space::S3))
            .items_center()
            .gap(space::S2)
            .children(notices)
            .children(toasts.iter().map(|t| toast(t, p))),
    )
}

/// One toast.
pub fn toast(toast: &Toast, p: Paint) -> El {
    let t = p.t;
    let key = toast.id.key();
    let lead = match toast.tone {
        Tone::Busy => icon(icons::SPINNER.at(p.scale), size::ICON).color(t.text_2).looping(LoopSpec::rotation(0.9)),
        Tone::Error => icon(icons::WARNING.at(p.scale), size::ICON).color(t.danger),
        Tone::Note => icon(icons::FOLDER.at(p.scale), size::ICON).color(t.text_2),
    };
    let mut e = row()
        .key(key.clone())
        .role(if toast.tone == Tone::Error { "alert" } else { "status" })
        .label(toast.text.clone())
        .busy(toast.tone == Tone::Busy)
        .w(WIDTH)
        .pl(space::S3)
        .pr(if toast.dismissible() { space::S1 } else { space::S3 })
        .py(space::S2)
        .gap(space::S2)
        .center_y()
        .radius(radius::POPOVER)
        .fill(t.sheet)
        .border(p.hair, if toast.tone == Tone::Error { t.danger.alpha(0.5) } else { t.elevated_edge() })
        .shadow(8.0, 24.0, t.shadow)
        .transition(Transition::offset(0.0, 8.0).combined(Transition::opacity()))
        .transition_motion(motion::MOVE, motion::MOVE)
        .child(lead)
        .child(
            text(toast.text.clone())
                .key(format!("{key}-text"))
                .font(type_::BODY)
                .color(t.text)
                .grow(1.0)
                .min_w(0.0)
                .wrap(),
        );
    if toast.dismissible() {
        let id = toast.id;
        e = e.child(
            el().key(format!("{key}-dismiss"))
                .role("button")
                .label("Dismiss")
                .size(24.0, 24.0)
                .radius(radius::ROW)
                .center()
                .hover(t.hover)
                .press(t.press)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::File(FileAction::DismissToast(id)), tx))
                .tab_press()
                .child(icon(icons::CLOSE.at(p.scale), size::ICON).color(t.text_3)),
        );
    }
    e
}
