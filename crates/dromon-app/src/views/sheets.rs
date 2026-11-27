//! Sheets (§6.8): sign in, a new host key, a yes/no question, and the connection log (the
//! delete, conflict, edit conflict, Add to SSH config, Properties and bug report sheets have
//! files of their own). They drop
//! from the toolbar's edge on the `move` spring, 12 pt radius, sheet elevation, over a veil
//! that makes the window behind them inert. Escape cancels (Rune's modal dismissal); Return
//! presses the primary button (`default_button`). Notices show as toasts that block nothing.

use dromon_core::{Prompt, PromptKind};
use rune::{Beat, El, LoopSpec, ScrollSpec, Sequence, Transition, Tx, col, icon, row, text, text_input};

use super::widgets::Keyboard;
use super::{Paint, conflict, quit, run};
use crate::icons;
use crate::state::sheets::takes_text;
use crate::state::{Action, App, Sheet};
use crate::theme::{motion, radius, size, space, type_};

/// Sheet width.
const WIDTH: f32 = 440.0;
/// The log sheet's width and the most its lines take.
const LOG_WIDTH: f32 = 680.0;
const LOG_HEIGHT: f32 = 360.0;

/// One shake of the field after a wrong answer: about a third of a second, side to side.
const SHAKE: &[Beat] = &[
    Beat::new(0.0),
    Beat::new(0.06).translate(-7.0, 0.0),
    Beat::new(0.12).translate(6.0, 0.0),
    Beat::new(0.18).translate(-4.0, 0.0),
    Beat::new(0.24).translate(2.0, 0.0),
    Beat::new(0.32),
];

/// The host's name as the sidebar shows it.
fn host_name(s: &App, host: &dromon_core::HostId) -> String {
    s.hosts.get(host).map_or(host.as_str(), |h| h.display_name()).to_owned()
}

/// The veil and the open sheet, over the whole window below the toolbar band, so the sheet
/// drops from the toolbar's edge.
pub fn layer(s: &App, p: Paint) -> Option<El> {
    // "Quit anyway?" comes first, then a prompt sheet, then the bug report; an upload's name
    // clash waits behind those, and an edited file's conflict behind that (`App::dismiss_sheet`
    // cancels in the same order).
    let panel = match (quit::sheet(s, p), s.ui.sheets.open()) {
        (Some(quit), _) => quit,
        (None, Some(Sheet::Ask { prompt, .. })) => prompt_sheet(s, prompt, false, p),
        (None, Some(Sheet::Sent { prompt })) => prompt_sheet(s, prompt, true, p),
        (None, Some(Sheet::Log { host })) => log_sheet(s, host, p),
        (None, Some(Sheet::Delete { names, .. })) => super::delete::sheet(s, names, p),
        (None, Some(Sheet::AddToConfig(v))) => super::add_config::sheet(s, v, p),
        (None, Some(Sheet::Properties(v))) => super::properties::sheet(s, v, p),
        (None, None) => super::report::sheet(s, p)
            .or_else(|| conflict::sheet(s, p))
            .or_else(|| super::editing::conflict_sheet(s, p))?,
    };
    Some(
        col()
            .key("sheet-layer")
            .pin(Some(0.0), Some(size::TOOLBAR), Some(0.0), Some(0.0))
            .items_center()
            .fill(p.t.scrim)
            .transition(Transition::opacity())
            .transition_motion(motion::MOVE, motion::MOVE)
            .child(panel),
    )
}

/// The panel every sheet shares: drops 16 pt from the toolbar's edge as it fades in.
fn panel(p: Paint, key: &'static str, title: String, width: f32) -> El {
    let t = p.t;
    frame(p, key, &title, width).child(text(title).key("sheet-title").font(type_::TITLE).color(t.text))
}

/// [`panel`] without its title, for a sheet that sets its own.
pub(super) fn frame(p: Paint, key: &'static str, title: &str, width: f32) -> El {
    let t = p.t;
    col()
        .key(key)
        .role("dialog")
        .label(title.to_owned())
        .w(width)
        .padding(space::S6)
        .gap(space::S3)
        .radius(radius::SHEET)
        .fill(t.sheet)
        .border(p.hair, t.elevated_edge())
        .shadow(16.0, 48.0, t.sheet_shadow)
        .transition(Transition::offset(0.0, -16.0).combined(Transition::opacity()))
        .transition_motion(motion::MOVE, motion::MOVE)
        .modal_tx(false, |s: &mut App, tx: &mut Tx| run(s, Action::DismissSheet, tx))
}

pub(super) fn button(p: Paint, key: &'static str, label: &str, primary: bool, action: Action) -> El {
    let t = p.t;
    let b = row()
        .key(key)
        .role("button")
        .label(label.to_owned())
        .h(28.0)
        .px(space::S3)
        .radius(radius::ROW)
        .center()
        .press_scale(0.97)
        .press_motion(rune::Anim::INSTANT, motion::SNAP)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, action.clone(), tx))
        .tab_stop();
    if primary {
        b.fill(t.accent).default_button().child(text(label.to_owned()).font(type_::BODY_STRONG).color(t.on_accent))
    } else {
        b.border(p.hair, t.border)
            .hover(t.hover)
            .press(t.press)
            .child(text(label.to_owned()).font(type_::BODY).color(t.text))
    }
}

/// The primary button once answered: busy until ssh moves on.
fn busy(p: Paint) -> El {
    let t = p.t;
    let spinner = icon(icons::SPINNER.at(p.scale), size::ICON).color(t.on_accent).looping(LoopSpec::rotation(0.9));
    row()
        .key("sheet-busy")
        .role("progressbar")
        .label("Connecting")
        .busy(true)
        .h(28.0)
        .px(space::S3)
        .gap(6.0)
        .radius(radius::ROW)
        .center()
        .fill(t.accent.alpha(0.6))
        .child(spinner)
        .child(text("Connecting…").font(type_::BODY_STRONG).color(t.on_accent))
}

pub(super) fn buttons(children: Vec<El>) -> El {
    row().key("sheet-buttons").w_full().pt(space::S2).gap(space::S2).justify_end().children(children)
}

fn prompt_sheet(s: &App, prompt: &Prompt, sent: bool, p: Paint) -> El {
    let host = host_name(s, &prompt.host);
    match &prompt.kind {
        PromptKind::HostKey { key_type, fingerprint } => {
            host_key(p, &host, prompt, key_type.as_deref(), fingerprint.as_deref(), sent)
        }
        PromptKind::Confirm => confirm(p, &host, prompt, sent),
        _ => sign_in(s, p, &host, prompt, sent),
    }
}

/// Password, key passphrase, or anything else the server asks (2FA, OTP).
fn sign_in(s: &App, p: Paint, host: &str, prompt: &Prompt, sent: bool) -> El {
    let t = p.t;
    let mut panel = panel(p, "sheet", format!("Sign in to {host}"), WIDTH).child(
        text(prompt.text.trim().to_owned())
            .key("sheet-prompt")
            .font(type_::BODY)
            .color(t.text_2)
            .w_full()
            .wrap_anywhere(),
    );
    if takes_text(&prompt.kind) {
        let mut input = text_input(s.ui.sheets.field(), type_::BODY)
            .key("sheet-input")
            .label(prompt.text.trim().to_owned())
            .single_line()
            .grow(1.0)
            .min_w(0.0)
            .color(t.text)
            .on_edit(|s: &mut App| s.ui.sheets.edit_mut());
        if prompt.kind.secret() {
            input = input.secure();
        }
        if sent {
            input = input.read_only(true);
        }
        let error = prompt.error.is_some();
        input = input.invalid(error);
        let mut field = row()
            .key(format!("sheet-field-{}", prompt.id.0))
            .w_full()
            .h(32.0)
            .px(space::S2)
            .center_y()
            .radius(radius::ROW)
            .fill(t.field)
            .border(p.hair, if error { t.danger } else { t.border_strong })
            .focus_visible_within_ring(2.0, 1.0, t.focus)
            .child(input);
        if error {
            field = field.sequence(Sequence::new(SHAKE));
        }
        panel = panel.child(field);
    }
    if let Some(e) = &prompt.error {
        panel = panel.child(text(e.clone()).key("sheet-error").role("alert").font(type_::META).color(t.danger));
    }
    if prompt.kind.rememberable() {
        let note = "Kept in memory until you quit or disconnect. Never saved to disk.";
        panel = panel.child(text(note).key("sheet-note").font(type_::META).color(t.text_3).w_full().wrap_anywhere());
    }
    let primary = if sent { busy(p) } else { button(p, "sheet-submit", "Connect", true, Action::SubmitPrompt) };
    panel.child(buttons(vec![button(p, "sheet-cancel", "Cancel", false, Action::DismissSheet), primary]))
}

fn host_key(
    p: Paint,
    host: &str,
    prompt: &Prompt,
    key_type: Option<&str>,
    fingerprint: Option<&str>,
    sent: bool,
) -> El {
    let t = p.t;
    let explain = format!(
        "This computer hasn't connected to {host} before, so ssh can't yet tell that it's the right server. \
         If the fingerprint below matches the one its owner gave you, trust it: ssh will remember it and \
         warn you if it ever changes."
    );
    let block = match fingerprint {
        Some(fp) => col()
            .gap(space::S1)
            .child(text(format!("{} key fingerprint", key_type.unwrap_or("Host"))).font(type_::META).color(t.text_3))
            .child(
                text(fp.to_owned()).key("sheet-fingerprint").font(type_::MONO).color(t.text).w_full().wrap_anywhere(),
            ),
        // ssh didn't print one we recognise: show what it said.
        None => col().child(
            text(prompt.text.trim().to_owned())
                .key("sheet-fingerprint")
                .font(type_::MONO)
                .color(t.text)
                .w_full()
                .wrap_anywhere(),
        ),
    };
    let block = block
        .key("sheet-key")
        .w_full()
        .padding(space::S3)
        .radius(radius::ROW)
        .fill(t.raised)
        .border(p.hair, t.border_subtle);
    let primary =
        if sent { busy(p) } else { button(p, "sheet-submit", "Trust and connect", true, Action::SubmitPrompt) };
    panel(p, "sheet", format!("First connection to {host}"), WIDTH)
        .child(text(explain).key("sheet-prompt").font(type_::BODY).color(t.text_2).w_full().wrap_anywhere())
        .child(block)
        .child(buttons(vec![button(p, "sheet-cancel", "Cancel", false, Action::DismissSheet), primary]))
}

fn confirm(p: Paint, host: &str, prompt: &Prompt, sent: bool) -> El {
    let t = p.t;
    let primary = if sent { busy(p) } else { button(p, "sheet-submit", "Yes", true, Action::SubmitPrompt) };
    panel(p, "sheet", format!("{host} asks"), WIDTH)
        .child(
            text(prompt.text.trim().to_owned())
                .key("sheet-prompt")
                .font(type_::BODY)
                .color(t.text_2)
                .w_full()
                .wrap_anywhere(),
        )
        .child(buttons(vec![button(p, "sheet-no", "No", false, Action::DeclinePrompt), primary]))
}

fn log_sheet(s: &App, host: &dromon_core::HostId, p: Paint) -> El {
    let t = p.t;
    let name = host_name(s, host);
    let c = s.conns.get(host);
    let body = match c.map(|c| &c.log).filter(|l| !l.is_empty()) {
        Some(log) => text(log.to_text())
            .key("log-lines")
            .font(type_::MONO)
            .color(t.text_2)
            .w_full()
            .wrap_anywhere()
            .selectable(true),
        None => text("Nothing logged yet.").key("log-lines").font(type_::BODY).color(t.text_3),
    };
    let lines = col()
        .key("log-scroll")
        .w_full()
        .max_h(LOG_HEIGHT)
        .padding(space::S3)
        .radius(radius::ROW)
        .fill(t.raised)
        .border(p.hair, t.border_subtle)
        .scroll(ScrollSpec::default())
        .child(body);
    let copy = row()
        .key("log-copy")
        .role("button")
        .label("Copy all")
        .h(28.0)
        .px(space::S3)
        .radius(radius::ROW)
        .center()
        .border(p.hair, t.border)
        .hover(t.hover)
        .press(t.press)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let host = s.ui.sheets.open().map(|sh| sh.host().clone());
            if let Some(c) = host.and_then(|h| s.conns.get(&h)) {
                tx.copy(c.log.to_text());
            }
        })
        .tab_stop()
        .child(text("Copy all").font(type_::BODY).color(t.text));
    let live = c.is_some_and(|c| crate::state::hosts::is_live(&c.status));
    let mut actions = Vec::new();
    if let Some(c) = c {
        actions.push(verbose(p, host, c.log.verbose));
        actions.push(rune::el().key("log-gap").grow(1.0));
    }
    actions.push(copy);
    if live {
        actions.push(button(p, "log-disconnect", "Disconnect", false, Action::Disconnect(host.clone())));
    }
    actions.push(button(p, "log-close", "Done", true, Action::DismissSheet));
    panel(p, "sheet", format!("Connection log · {name}"), LOG_WIDTH).child(lines).child(buttons(actions).center_y())
}

/// The log's "Verbose logging" switch (§6.8), with what changing it does: ssh reconnects with
/// `-v` (or without). The whole row presses.
fn verbose(p: Paint, host: &dromon_core::HostId, on: bool) -> El {
    let t = p.t;
    let knob = rune::el()
        .size(14.0, 14.0)
        .radius(7.0)
        .fill(if on { t.on_accent } else { t.text_2 })
        .offset(if on { 14.0 } else { 0.0 }, 0.0);
    let track = row()
        .key("log-verbose-track")
        .w(32.0)
        .h(18.0)
        .padding(2.0)
        .radius(9.0)
        .center_y()
        .fill(if on { t.accent } else { t.field })
        .border(p.hair, if on { t.accent } else { t.border_strong })
        .motion(motion::SNAP)
        .child(knob);
    let action = Action::Verbose { host: host.clone(), on: !on };
    row()
        .key("log-verbose")
        .role("switch")
        .label("Verbose logging (reconnects)")
        .checked(on)
        .focusable(true)
        .keyboard_focus_only()
        .h(28.0)
        .gap(space::S2)
        .center_y()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, action.clone(), tx))
        .child(track)
        .child(text("Verbose logging").font(type_::BODY).color(t.text))
        .child(text("reconnects").key("log-verbose-note").font(type_::META).color(t.text_3))
}

/// Notices (ssh's "touch your security key"): toasts above the status bar until ssh is done,
/// in the toast stack (`toast::layer`).
pub fn notices(s: &App, p: Paint) -> Option<El> {
    let t = p.t;
    let list = s.ui.sheets.notices();
    if list.is_empty() {
        return None;
    }
    let toasts = list.iter().map(|n| {
        let host = host_name(s, &n.host);
        row()
            .key(format!("notice-{}", n.id.0))
            .role("status")
            .label(n.text.trim().to_owned())
            .max_w(WIDTH + 80.0)
            .px(space::S3)
            .py(space::S2)
            .gap(space::S2)
            .center_y()
            .radius(radius::POPOVER)
            .fill(t.sheet)
            .border(p.hair, t.elevated_edge())
            .shadow(8.0, 24.0, t.shadow)
            .transition(Transition::offset(0.0, 8.0).combined(Transition::opacity()))
            .transition_motion(motion::MOVE, motion::MOVE)
            .child(icon(icons::SERVER.at(p.scale), size::ICON).color(t.text_2))
            .child(text(format!("{host}: {}", n.text.trim())).font(type_::BODY).color(t.text).shrink().wrap())
    });
    Some(col().key("notices").items_center().gap(space::S2).children(toasts))
}
