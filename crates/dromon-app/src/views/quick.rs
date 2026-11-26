//! The quick-connect popover (§6.8 "Quick connect", `state::quick`): hung from the sidebar's +,
//! 320 pt on `raised` with popover elevation, growing from its trigger. Host (mono), Username
//! and Port (placeholder `22`); **More** shows a name, an identity file (with a file picker)
//! and a jump host. **Save** and **Connect** (the default: Return in any field). A field's
//! problem shows under it in the danger colour and its box shakes once. Modal: Escape or a
//! press outside closes it.

use dromon_ssh::{QuickField, split_address};
use rune::{
    Align, Beat, El, FileDialogOptions, Key, KeyEvent, Sequence, Side, Transition, Tx, col, icon, row, text, text_input,
};

use super::widgets::Keyboard;
use super::{Paint, run, sidebar};
use crate::icons;
use crate::state::{Action, App, HostAction};
use crate::theme::{motion, radius, size, space, type_};

/// The popover's width.
const WIDTH: f32 = 320.0;
/// The Port field's width.
const PORT_WIDTH: f32 = 76.0;

/// One shake of a field with a problem (the sign-in sheet's).
const SHAKE: &[Beat] = &[
    Beat::new(0.0),
    Beat::new(0.06).translate(-6.0, 0.0),
    Beat::new(0.12).translate(5.0, 0.0),
    Beat::new(0.18).translate(-3.0, 0.0),
    Beat::new(0.24).translate(2.0, 0.0),
    Beat::new(0.32),
];

/// The field's name in element keys: `quick-<name>-input`, `quick-<name>-error`.
pub fn key_name(f: QuickField) -> &'static str {
    match f {
        QuickField::Host => "host",
        QuickField::User => "user",
        QuickField::Port => "port",
        QuickField::Alias => "alias",
        QuickField::IdentityFile => "identity",
        QuickField::ProxyJump => "jump",
    }
}

fn act(s: &mut App, a: HostAction, tx: &mut Tx) {
    run(s, Action::HostBook(a), tx);
}

fn escape(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Escape {
        act(s, HostAction::CloseQuick, tx);
        return true;
    }
    false
}

pub fn popover(s: &App, p: Paint) -> Option<El> {
    let q = &s.ui.quick;
    if !q.open || s.ui.sheets.is_open() {
        return None;
    }
    let t = p.t;
    let title = match q.editing.as_ref().and_then(|id| s.hosts.get(id)) {
        Some(h) => format!("Edit {}", h.display_name()),
        None => "Quick connect".to_owned(),
    };
    // Username and Port side by side; their problems under both, full width.
    let pair = row()
        .w_full()
        .gap(space::S2)
        .child(field(s, p, QuickField::User, "optional", false).grow(1.0).min_w(0.0))
        .child(field(s, p, QuickField::Port, "22", false).w(PORT_WIDTH));
    let names = col()
        .w_full()
        .gap(space::S1)
        .child(pair)
        .children([QuickField::User, QuickField::Port].into_iter().filter_map(|f| error_text(s, p, f)));
    let mut panel = col()
        .key("quick")
        .role("dialog")
        .label(title.clone())
        .w(WIDTH)
        .padding(space::S4)
        .gap(space::S3)
        .anchored(sidebar::QUICK_CONNECT, Side::Top, Align::Start, 6.0)
        .anchored_round()
        .radius(radius::POPOVER)
        .fill(t.raised)
        .border(p.hair, if t.dark { t.border } else { t.border_subtle })
        .shadow(8.0, 24.0, t.shadow)
        .origin(0.0, 1.0)
        .transition(Transition::scale(0.96).combined(Transition::opacity()))
        .transition_motion(motion::MOVE, motion::MOVE)
        .modal_tx(true, |s: &mut App, tx: &mut Tx| act(s, HostAction::CloseQuick, tx))
        .child(text(title).key("quick-title").font(type_::TITLE).color(t.text))
        .child(field(s, p, QuickField::Host, "host, or user@host:port", true))
        .child(names)
        .child(more_toggle(s, p));
    if q.more {
        panel = panel
            .child(field(s, p, QuickField::Alias, "the host's name", true))
            .child(field(s, p, QuickField::IdentityFile, "your default keys", true))
            .child(field(s, p, QuickField::ProxyJump, "user@bastion:22", true));
    }
    Some(panel.child(buttons(p, q.editing.is_some())))
}

/// `f`'s problem, in the danger colour.
fn error_text(s: &App, p: Paint, f: QuickField) -> Option<El> {
    let e = s.ui.quick.errors.get(&f)?;
    Some(
        text(e.clone())
            .key(format!("quick-{}-error", key_name(f)))
            .role("alert")
            .font(type_::META)
            .color(p.t.danger)
            .w_full()
            .wrap(),
    )
}

/// A label and the field's box, with its problem under it when `with_error`.
fn field(s: &App, p: Paint, f: QuickField, placeholder: &str, with_error: bool) -> El {
    let t = p.t;
    let q = &s.ui.quick;
    let name = key_name(f);
    let error = q.errors.get(&f);
    let font = if f == QuickField::Host { type_::MONO } else { type_::BODY };
    let mut input = text_input(q.edit(f), font)
        .key(format!("quick-{name}-input"))
        .label(f.label())
        .invalid(error.is_some())
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .color(t.text)
        .placeholder(placeholder.to_owned(), t.text_3)
        .on_edit(move |s: &mut App| s.ui.quick.edit_mut(f))
        .on_input_change(move |s: &mut App, tx: &mut Tx| act(s, HostAction::Edited(f), tx))
        // Return is the primary button's: Save while editing a saved host, else Connect.
        .on_submit(|s: &mut App, tx: &mut Tx| {
            let a = if s.ui.quick.editing.is_some() { HostAction::Save } else { HostAction::Connect };
            act(s, a, tx)
        })
        .on_input_key(escape);
    if f == QuickField::Host {
        // `user@host:port` pasted into Host fills Username and Port too.
        input = input.on_input_paste(|s: &mut App, content: &rune::ClipboardContent, tx: &mut Tx| {
            let text = content.text().to_owned();
            if split_address(&text).is_none() {
                return false;
            }
            act(s, HostAction::PasteHost(text), tx);
            true
        });
    }
    let mut boxed = row()
        .key(format!("quick-{name}-box-{}", if error.is_some() { q.shakes } else { 0 }))
        .w_full()
        .h(28.0)
        .pl(space::S2)
        .pr(if f == QuickField::IdentityFile { 2.0 } else { space::S2 })
        .gap(space::S1)
        .radius(radius::ROW)
        .center_y()
        .fill(t.hover)
        .border(p.hair, if error.is_some() { t.danger } else { t.border })
        .focus_visible_within_ring(2.0, 1.0, t.focus)
        .child(input);
    if f == QuickField::IdentityFile {
        boxed = boxed.child(pick_button(p));
    }
    if error.is_some() && q.shakes > 0 {
        boxed = boxed.sequence(Sequence::new(SHAKE));
    }
    col()
        .key(format!("quick-{name}"))
        .w_full()
        .gap(space::S1)
        .child(text(f.label()).font(type_::META_STRONG).color(t.text_2))
        .child(boxed)
        .children(with_error.then(|| error_text(s, p, f)).flatten())
}

/// "Choose…" inside the identity file's box: the platform's file picker.
fn pick_button(p: Paint) -> El {
    let t = p.t;
    row()
        .key("quick-pick-identity")
        .role("button")
        .label("Choose an identity file")
        .h(22.0)
        .px(space::S2)
        .radius(radius::ROW - 2.0)
        .center()
        .border(p.hair, t.border)
        .hover(t.hover)
        .press(t.press)
        .on_press_tx(|_: &mut App, tx: &mut Tx| {
            let options = FileDialogOptions {
                title: "Choose an identity file".into(),
                directory: rune::dirs::home().map(|h| h.join(".ssh")),
                ..FileDialogOptions::default()
            };
            tx.open_file(options, |s: &mut App, picked, tx| {
                act(s, HostAction::PickedIdentity(picked.map_err(|e| e.to_string())), tx)
            });
        })
        .tab_press()
        .child(text("Choose…").font(type_::META_STRONG).color(t.text_2))
}

/// More / Less, with a caret.
fn more_toggle(s: &App, p: Paint) -> El {
    let t = p.t;
    let more = s.ui.quick.more;
    let glyph = if more { &icons::CARET_DOWN } else { &icons::CHEVRON_RIGHT };
    row()
        .key("quick-more")
        .role("button")
        .label(if more { "Fewer options" } else { "More options" })
        .expanded(more)
        .h(22.0)
        .pl(2.0)
        .pr(space::S2)
        .gap(2.0)
        .ml(-2.0)
        .radius(radius::ROW)
        .center_y()
        .hover(t.hover)
        .press(t.press)
        .on_press_tx(|s: &mut App, tx: &mut Tx| act(s, HostAction::ToggleMore, tx))
        .tab_press()
        .child(icon(glyph.at(p.scale), size::ICON).color(t.text_3))
        .child(text("More").font(type_::META_STRONG).color(t.text_2))
}

/// Save and Connect; the primary one (accent, Return) is Connect, or Save while editing.
fn buttons(p: Paint, editing: bool) -> El {
    let t = p.t;
    let button = |key: &'static str, label: &'static str, action: HostAction, primary: bool| {
        let b = row()
            .key(key)
            .role("button")
            .label(label)
            .h(28.0)
            .px(space::S3)
            .radius(radius::ROW)
            .center()
            .press_scale(0.97)
            .press_motion(rune::Anim::INSTANT, motion::SNAP)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| act(s, action.clone(), tx))
            .tab_press();
        if primary {
            b.fill(t.accent).child(text(label).font(type_::BODY_STRONG).color(t.on_accent))
        } else {
            b.border(p.hair, t.border).hover(t.hover).press(t.press).child(text(label).font(type_::BODY).color(t.text))
        }
    };
    let save = button("quick-save", "Save", HostAction::Save, editing);
    let connect = button("quick-connect", "Connect", HostAction::Connect, !editing);
    row().key("quick-buttons").w_full().pt(space::S1).gap(space::S2).justify_end().child(save).child(connect)
}
