//! The toast stack, bottom centre above the status bar: ssh's notices first, then the app's
//! toasts (`state::toast`), oldest on top. Each rises 8 pt as it fades in and leaves the same
//! way (`move`). Running ones show a spinner; failures a warning glyph and a dismiss button.
//! The update toast (`state::update`) comes last, with its buttons under its text.

use rune::{El, LoopSpec, Transition, Tx, col, el, icon, row, text};

use super::widgets::Keyboard;
use super::{Paint, run, sheets};
use crate::icons;
use crate::state::toast::{Toast, Tone};
use crate::state::update::{Button, UpdateToast};
use crate::state::{Action, App, FileAction};
use crate::theme::{motion, radius, size, space, type_};

/// Every toast is this wide (longer text wraps): a whole width keeps a centred toast, and the
/// glyph at its start, on whole points.
const WIDTH: f32 = 360.0;

pub fn layer(s: &App, p: Paint) -> Option<El> {
    let notices = sheets::notices(s, p);
    let toasts = s.toasts();
    let update = s.update.toast();
    if notices.is_none() && toasts.is_empty() && update.is_none() {
        return None;
    }
    Some(
        col()
            .key("toasts")
            .pin(Some(0.0), None, Some(0.0), Some(size::STATUS + space::S3))
            .items_center()
            .gap(space::S2)
            .children(notices)
            .children(toasts.iter().map(|t| toast(t, p)))
            .children(update.map(|u| update_toast(&u, p))),
    )
}

/// The update toast's key.
const UPDATE: &str = "toast-update";

/// "Dromon 0.3.0 is available", its buttons in a row under the text (Update or Copy command,
/// Later, Release notes); "Updating to Dromon 0.3.0…" with a spinner while it installs.
fn update_toast(u: &UpdateToast, p: Paint) -> El {
    let t = p.t;
    let lead = match u.tone {
        Tone::Busy => icon(icons::SPINNER.at(p.scale), size::ICON).color(t.text_2).looping(LoopSpec::rotation(0.9)),
        Tone::Error => icon(icons::WARNING.at(p.scale), size::ICON).color(t.danger),
        Tone::Note => icon(icons::DOWNLOAD.at(p.scale), size::ICON).color(t.text_2),
    };
    let words = text(u.text.clone()).key(format!("{UPDATE}-text")).font(type_::BODY).color(t.text).w_full().wrap();
    let buttons = row().gap(space::S1).children(u.buttons.iter().map(|&b| {
        row()
            .key(b.key(UPDATE))
            .role("button")
            .label(b.label())
            .h(24.0)
            .px(space::S2)
            .radius(radius::ROW)
            .center()
            .fill(if b == Button::Update { t.press } else { rune::Color::TRANSPARENT })
            .hover(t.hover)
            .press(t.press)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::Update(b.action()), tx))
            .tab_press()
            .child(text(b.label()).font(type_::BODY_STRONG).color(t.text))
    }));
    row()
        .key(UPDATE)
        .role(if u.tone == Tone::Error { "alert" } else { "status" })
        .label(u.text.clone())
        .busy(u.tone == Tone::Busy)
        .w(WIDTH)
        .px(space::S3)
        .py(space::S2)
        .gap(space::S2)
        .radius(radius::POPOVER)
        .fill(t.sheet)
        .border(p.hair, if u.tone == Tone::Error { t.danger.alpha(0.5) } else { t.elevated_edge() })
        .shadow(8.0, 24.0, t.shadow)
        .transition(Transition::offset(0.0, 8.0).combined(Transition::opacity()))
        .transition_motion(motion::MOVE, motion::MOVE)
        .child(lead)
        .child(
            col().grow(1.0).min_w(0.0).gap(space::S1).child(words).children((!u.buttons.is_empty()).then_some(buttons)),
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
