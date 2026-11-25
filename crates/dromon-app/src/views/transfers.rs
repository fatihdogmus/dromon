//! Transfers (§6.8): the toolbar's transfers button (an arrow inside a progress ring over every
//! active job, with a count badge) and the popover it opens (Downloads / Uploads tabs, one row
//! per job, "Clear finished").
//!
//! The ring is two half rings, each turned into view behind a clip as the fraction grows, so
//! it sweeps on the `move` spring and retargets from wherever it is.

use std::f32::consts::PI;

use dromon_transfer::{JobKind, JobView, Reveal, TaskState, format_bytes, progress_line};
use rune::{Align, Color, El, LineCap, ScrollSpec, Shape, Side, Transition, Tx, canvas, col, el, icon, row, text};

use super::{Paint, run};
use crate::fmt;
use crate::icons::{self, IconPair};
use crate::state::{Action, App, Tab, TransferCommand};
use crate::theme::{motion, radius, size, space, type_};

#[cfg(test)]
#[path = "transfers_tests.rs"]
mod tests;

/// The popover's width and the most it grows to (§6.8).
const WIDTH: f32 = 380.0;
const MAX_HEIGHT: f32 = 480.0;
/// The ring's outer diameter and stroke, inside the 28 pt button.
const RING: f32 = 24.0;
const RING_STROKE: f32 = 2.0;
/// How many detail lines a row's tooltip shows.
const DETAIL_LINES: usize = 8;

/// The key of the toolbar button, which the popover hangs from.
pub const BUTTON: &str = "tb-transfers";

/// The toolbar's transfers button. Idle, it's the plain glyph (as quiet as a disabled button
/// until the session has any job); with jobs running, the glyph sits in a ring of their
/// progress, with a badge counting them.
pub fn button(s: &App, p: Paint) -> El {
    let t = p.t;
    let tr = &s.transfers;
    let active = tr.active();
    let glyph_color = if tr.jobs.is_empty() { t.text_3.alpha(0.6) } else { t.text_2 };
    let label = if active > 0 { format!("Transfers, {active} active") } else { "Transfers".to_owned() };
    let mut b = el()
        .key(BUTTON)
        .role("button")
        .label(label)
        .expanded(tr.open)
        .size(28.0, 28.0)
        .radius(radius::ROW)
        .center()
        .hover(t.hover)
        .press(t.press)
        .press_scale(0.97)
        .press_motion(rune::Anim::INSTANT, motion::SNAP)
        .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::ToggleTransfers, tx));
    if tr.open {
        b = b.fill(t.press);
    }
    if let Some(progress) = tr.ring() {
        b = b.child(ring(p, progress.fraction()));
    }
    b = b.child(icon(icons::DOWNLOAD.at(p.scale), size::ICON).color(glyph_color));
    if active > 0 {
        b = b.child(badge(p, active));
    }
    b
}

/// A 2 pt ring of `fraction` in the accent over a quiet track, centred in the button.
fn ring(p: Paint, fraction: f32) -> El {
    let t = p.t;
    let r = (RING - RING_STROKE) / 2.0;
    let c = RING / 2.0;
    let (top, bottom) = (c - r, c + r);
    // Round caps: the clips cut the trailing ends flat, the leading end stays round.
    let half = |sweep: u8, color: Color| {
        let d = format!("M{c} {top} A{r} {r} 0 0 {sweep} {c} {bottom}");
        canvas(Shape::new().stroke_svg(&d, RING_STROKE, LineCap::Round, color)).size(RING, RING)
    };
    let track = format!("M{c} {top} A{r} {r} 0 0 1 {c} {bottom} A{r} {r} 0 0 1 {c} {top}Z");
    let f = fraction.clamp(0.0, 1.0);
    // The left half turned clockwise into the right-hand clip draws 0–50%; the right half
    // turned into the left-hand clip draws the rest. A half not turned at all would still show
    // its leading cap past the clip, so it hides.
    let turned = |e: El, angle: f32| e.rotate(angle).rotate_motion(motion::MOVE).opacity(if angle > 0.0 { 1.0 } else { 0.0 });
    let first = turned(half(0, t.accent).absolute(-c, 0.0), f.min(0.5) * 2.0 * PI);
    let second = turned(half(1, t.accent).absolute(0.0, 0.0), (f - 0.5).max(0.0) * 2.0 * PI);
    // The track is opaque: a translucent stroke darkens where its segments overlap.
    let track_color = flatten(t.border_strong, t.surface);
    el().key("tb-ring")
        .role("progressbar")
        .label(format!("{:.0}%", f * 100.0))
        .absolute((28.0 - RING) / 2.0, (28.0 - RING) / 2.0)
        .size(RING, RING)
        .child(canvas(Shape::new().stroke_svg(&track, RING_STROKE, LineCap::Round, track_color)).absolute(0.0, 0.0).size(RING, RING))
        .child(el().absolute(c, 0.0).size(c, RING).clip_soft(0.0).child(first))
        .child(el().absolute(0.0, 0.0).size(c, RING).clip_soft(0.0).child(second))
}

/// `top` drawn over `bottom` as the screen blends them (in sRGB), as one opaque colour.
fn flatten(top: Color, bottom: Color) -> Color {
    let enc = |x: f32| x.clamp(0.0, 1.0).powf(1.0 / 2.2);
    let dec = |x: f32| x.clamp(0.0, 1.0).powf(2.2);
    let m = |a: f32, b: f32| dec(enc(b) + (enc(a) - enc(b)) * top.a);
    Color { r: m(top.r, bottom.r), g: m(top.g, bottom.g), b: m(top.b, bottom.b), a: 1.0 }
}

/// The count of active jobs, at the button's top-right corner.
fn badge(p: Paint, n: usize) -> El {
    let t = p.t;
    let label = if n > 99 { "99+".to_owned() } else { n.to_string() };
    row()
        .key("tb-badge")
        .absolute(17.0, -4.0)
        .h(15.0)
        .min_w(15.0)
        .px(4.0)
        .radius(7.5)
        .center()
        .fill(t.accent)
        .border(1.5, t.surface)
        .child(text(label).key("tb-badge-count").font(type_::BADGE).color(t.on_accent))
}

/// The popover, hung from the button. Modal: a click outside or Escape closes it. A prompt
/// sheet takes its place while it's up (it comes back after); a name clash waits in its row
/// instead of opening the conflict sheet over it.
pub fn popover(s: &App, p: Paint) -> Option<El> {
    let tr = &s.transfers;
    if !tr.open || s.ui.sheets.is_open() {
        return None;
    }
    let t = p.t;
    let e = col()
        .key("transfers")
        .role("dialog")
        .label("Transfers")
        .w(WIDTH)
        .max_h(MAX_HEIGHT)
        .anchored(BUTTON, Side::Bottom, Align::End, 6.0)
        .anchored_round()
        .radius(radius::POPOVER)
        .fill(t.raised)
        .border(p.hair, t.elevated_edge())
        .shadow(8.0, 24.0, t.shadow)
        .origin(1.0, 0.0)
        .transition(Transition::scale(0.96).combined(Transition::opacity()))
        .transition_motion(motion::MOVE, motion::MOVE)
        .child(tabs(s, p))
        .child(jobs(s, p))
        .child(footer(s, p))
        .modal_tx(true, |s: &mut App, tx: &mut Tx| run(s, Action::Escape, tx));
    Some(e)
}

/// The segmented Downloads / Uploads control, each with its count.
fn tabs(s: &App, p: Paint) -> El {
    let t = p.t;
    let seg = |tab: Tab, key: &'static str, label: &'static str| {
        let on = s.transfers.tab == tab;
        let n = s.transfers.in_tab(tab).count();
        let mut e = row()
            .key(key)
            .role("tab")
            .label(format!("{label}, {n}"))
            .checked(on)
            .grow(1.0)
            .h(24.0)
            .gap(6.0)
            .radius(radius::ROW - 1.0)
            .center()
            .motion(motion::SNAP)
            .child(text(label).font(if on { type_::BODY_STRONG } else { type_::BODY }).color(if on { t.text } else { t.text_2 }))
            .child(text(fmt::thousands(n as u64)).font(type_::META).color(t.text_3));
        if on {
            e = e.fill(t.press).border(p.hair, t.border_subtle);
        } else {
            e = e.hover(t.hover).on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::TransfersTab(tab), tx));
        }
        e
    };
    let control = row()
        .key("tr-tabs")
        .role("tablist")
        .w_full()
        .h(28.0)
        .padding(2.0)
        .gap(2.0)
        .radius(radius::ROW + 1.0)
        .fill(t.field)
        .child(seg(Tab::Downloads, "tr-tab-downloads", "Downloads"))
        .child(seg(Tab::Uploads, "tr-tab-uploads", "Uploads"));
    row().w_full().padding(space::S3).border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle).child(control)
}

/// The open tab's jobs, newest first, scrolling past the popover's height.
fn jobs(s: &App, p: Paint) -> El {
    let t = p.t;
    let tab = s.transfers.tab;
    let rows: Vec<El> = s.transfers.in_tab(tab).map(|j| job_row(p, j)).collect();
    let body = if rows.is_empty() {
        let (headline, hint) = match tab {
            Tab::Downloads => ("No downloads yet", format!("Select files and press {}", super::widgets::chord("mod+d"))),
            Tab::Uploads => ("No uploads yet", "Drop files on the list to upload them".to_owned()),
        };
        col()
            .key("tr-empty")
            .w_full()
            .h(144.0)
            .center()
            .gap(space::S1)
            .child(text(headline).font(type_::BODY_STRONG).color(t.text_2))
            .child(text(hint).font(type_::META).color(t.text_3))
    } else {
        col().w_full().py(space::S1).children(rows)
    };
    col().key("tr-list").w_full().shrink().min_h(0.0).scroll(ScrollSpec::default()).child(body)
}

/// The line under a job's bar: its progress, or what it's doing instead (§6.8).
pub fn status_line(j: &JobView) -> String {
    if let Some(c) = &j.conflict {
        return format!("Waiting for you: ‘{}’ already exists", c.name);
    }
    if j.waiting_for_connection && !matches!(j.state, TaskState::Paused) && j.state.is_active() {
        return "Waiting for connection".to_owned();
    }
    let mut line = match &j.state {
        TaskState::Queued => "Queued".to_owned(),
        TaskState::Preparing { files_found: 0 } => "Preparing…".to_owned(),
        TaskState::Preparing { files_found: 1 } => "Preparing… 1 file".to_owned(),
        TaskState::Preparing { files_found } => format!("Preparing… {} files", fmt::thousands(*files_found)),
        TaskState::Running => progress_line(j.progress.done, j.progress.total, j.speed, j.eta),
        TaskState::Paused => "Paused".to_owned(),
        TaskState::Done if j.progress.total == 0 && j.files_total == 0 => "Done".to_owned(),
        TaskState::Done => format!("Done · {}", format_bytes(j.progress.total)),
        TaskState::Failed { error } => format!("Failed: {error}"),
        TaskState::Cancelled => "Cancelled".to_owned(),
    };
    if j.state.is_finished() && !j.skipped.is_empty() {
        line.push_str(&format!(" · {} skipped", fmt::thousands(j.skipped.len() as u64)));
    }
    line
}

/// What a row's tooltip lists: the files that failed, then what was left out.
pub fn details(j: &JobView) -> Option<String> {
    let lines: Vec<&String> = j.errors.iter().chain(&j.skipped).collect();
    if lines.is_empty() {
        return None;
    }
    let mut out: Vec<String> = lines.iter().take(DETAIL_LINES).map(|l| (*l).clone()).collect();
    if lines.len() > DETAIL_LINES {
        out.push(format!("and {} more", fmt::thousands((lines.len() - DETAIL_LINES) as u64)));
    }
    Some(out.join("\n"))
}

/// The buttons a job shows on hover, by state.
fn controls(j: &JobView) -> Vec<(&'static str, &'static str, &'static IconPair, TransferCommand)> {
    let id = j.id;
    match j.state {
        TaskState::Queued => vec![("cancel", "Cancel", &icons::CLOSE, TransferCommand::Cancel(id))],
        TaskState::Preparing { .. } | TaskState::Running => vec![
            ("pause", "Pause", &icons::PAUSE, TransferCommand::Pause(id)),
            ("cancel", "Cancel", &icons::CLOSE, TransferCommand::Cancel(id)),
        ],
        TaskState::Paused => vec![
            ("resume", "Resume", &icons::RESUME, TransferCommand::Resume(id)),
            ("cancel", "Cancel", &icons::CLOSE, TransferCommand::Cancel(id)),
        ],
        TaskState::Failed { .. } => vec![("retry", "Retry", &icons::RETRY, TransferCommand::Retry(id))],
        TaskState::Done | TaskState::Cancelled => Vec::new(),
    }
}

fn job_row(p: Paint, j: &JobView) -> El {
    let t = p.t;
    let id = j.id;
    let failed = matches!(j.state, TaskState::Failed { .. });
    let (glyph, tint): (&IconPair, Color) = match &j.state {
        TaskState::Done => (&icons::CHECK, t.accent),
        TaskState::Failed { .. } => (&icons::WARNING, t.danger),
        _ if j.kind == JobKind::Download => (&icons::DOWNLOAD, t.text_2),
        _ => (&icons::UPLOAD, t.text_2),
    };
    let tile = el().size(28.0, 28.0).radius(radius::ROW).center().fill(t.field).child(icon(glyph.at(p.scale), size::ICON).color(tint));

    let buttons = controls(j).into_iter().map(|(name, label, glyph, cmd)| {
        el().key(format!("tr-{name}-{}", id.0))
            .role("button")
            .label(label)
            .size(22.0, 22.0)
            .radius(radius::ROW - 2.0)
            .center()
            .hover(t.hover)
            .press(t.press)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::Transfer(cmd.clone()), tx))
            .child(icon(glyph.at(p.scale), size::ICON).color(t.text_2))
    });
    // A name clash waits for an answer: closing the popover brings up its sheet.
    let resolve = j.conflict.as_ref().map(|_| {
        row()
            .key(format!("tr-resolve-{}", id.0))
            .role("button")
            .label("Resolve the name clash")
            .h(22.0)
            .px(space::S2)
            .radius(radius::ROW - 2.0)
            .center()
            .border(p.hair, t.accent.alpha(0.5))
            .hover(t.accent.alpha(0.1))
            .press(t.accent.alpha(0.16))
            .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::ToggleTransfers, tx))
            .child(text("Resolve…").font(type_::META_STRONG).color(t.accent))
    });
    let title = row()
        .w_full()
        .h(22.0)
        .gap(space::S1)
        .center_y()
        .child(text(j.title.clone()).key(format!("tr-title-{}", id.0)).font(type_::BODY).color(t.text).truncate().grow(1.0).min_w(0.0))
        .children(resolve)
        .child(row().gap(2.0).group_opacity(0.0, 1.0).children(buttons));

    let (fraction, bar) = match &j.state {
        TaskState::Done => (1.0, t.accent),
        TaskState::Failed { .. } => (j.progress.fraction().max(0.02), t.danger),
        TaskState::Paused | TaskState::Cancelled => (j.progress.fraction(), t.text_3),
        _ if j.waiting_for_connection || j.conflict.is_some() => (j.progress.fraction(), t.text_3),
        _ => (j.progress.fraction(), t.accent),
    };
    let progress = el()
        .key(format!("tr-bar-{}", id.0))
        .role("progressbar")
        .label(format!("{:.0}%", fraction * 100.0))
        .w_full()
        .h(3.0)
        .mt(3.0)
        .radius(1.5)
        .fill(t.border)
        .child(el().w_frac(fraction).h_full().radius(1.5).fill(bar).motion(motion::MOVE));
    let line = text(status_line(j))
        .key(format!("tr-line-{}", id.0))
        .font(type_::META)
        .color(if failed { t.danger } else { t.text_2 })
        .mt(5.0)
        .truncate();

    let mut r = row()
        .key(format!("tr-{}", id.0))
        .role("listitem")
        .label(format!("{}, {}", j.title, status_line(j)))
        .w_full()
        .px(space::S1)
        .child(
            row()
                .w_full()
                .px(space::S2)
                .py(space::S2)
                .gap(space::S3)
                .radius(radius::ROW)
                .hover(t.hover)
                .hover_group()
                .motion(motion::SNAP)
                .child(tile)
                .child(col().grow(1.0).min_w(0.0).child(title).child(progress).child(line)),
        );
    // A finished download shows in Finder or Explorer; a finished upload in its folder.
    let reveal = match (&j.state, &j.reveal) {
        (TaskState::Done, Some(Reveal::Local(_))) => Some("Show in folder"),
        (TaskState::Done, Some(Reveal::Remote(..))) => Some("Show on the server"),
        _ => None,
    };
    if let Some(label) = reveal {
        r = r.on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::RevealTransfer(id), tx)).help(label);
    }
    // What failed or was left out says so on hover, over the reveal hint.
    if let Some(d) = details(j) {
        r = r.help(d);
    }
    r
}

/// "3 active" and Clear finished.
fn footer(s: &App, p: Paint) -> El {
    let t = p.t;
    let active = s.transfers.active();
    let summary = if active == 0 { "Nothing running".to_owned() } else { format!("{} active", fmt::thousands(active as u64)) };
    let can_clear = s.transfers.has_finished();
    let mut clear = row()
        .key("tr-clear")
        .role("button")
        .label("Clear finished")
        .h(24.0)
        .px(space::S2)
        .radius(radius::ROW)
        .center()
        .child(text("Clear finished").font(type_::META_STRONG).color(if can_clear { t.text_2 } else { t.text_3.alpha(0.6) }));
    clear = if can_clear {
        clear
            .hover(t.hover)
            .press(t.press)
            .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::Transfer(TransferCommand::ClearFinished), tx))
    } else {
        clear.disabled(true)
    };
    row()
        .key("tr-footer")
        .w_full()
        .h(40.0)
        .pl(space::S4)
        .pr(space::S2)
        .center_y()
        .border_sides([p.hair, 0.0, 0.0, 0.0], t.border_subtle)
        .child(text(summary).key("tr-summary").font(type_::META).color(t.text_3).grow(1.0))
        .child(clear)
}
