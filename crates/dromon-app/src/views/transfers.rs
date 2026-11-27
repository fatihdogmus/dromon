//! Transfers (§6.8): the toolbar's transfers button (an arrow inside a progress ring over every
//! active job, with a count badge) and the popover it opens (Downloads / Uploads tabs, one row
//! per job, "Clear finished").
//!
//! The ring is two half rings, each turned into view behind a clip as the fraction grows, so
//! it sweeps on the `move` spring and retargets from wherever it is.
//!
//! The done flourish (§6.6): a job that finishes while the app watches fills its bar, which
//! then collapses into a mint check (on `settle`) while the ring sends out one pulse. Its
//! phases are `state::transfers::Flourish`, stepped by [`poll`]. Under reduce motion the check
//! is there at once and nothing pulses.
//!
//! The download-started animation (Chrome's, [`started`]): a download the user starts in this
//! window sends an accent circle with the button's glyph from the middle of the content area
//! up into the button, fading in, then out. Its phases are `state::transfers::Started`,
//! stepped by [`poll`] with the flourishes. Under reduce motion it doesn't play.
//!
//! Outside the window ([`connect_os`], [`connect_window`]): the taskbar button's bar and badge
//! (Windows), the Dock badge (macOS), and system notifications, from the effects of the leading
//! main window's `state::transfers::Watch`. The flourish plays in every window.

use std::f32::consts::PI;
use std::sync::mpsc;

use dromon_transfer::{JobId, JobKind, JobView, Reveal, TaskState, format_bytes, progress_line};
use rune::{
    Align, AppHandle, Beat, Color, Cx, Easing, El, LineCap, LoopSpec, MotionIntent, ScrollSpec, Sequence, Shape, Side,
    Taskbar, TaskbarProgress, Transition, Tx, WindowHandle, canvas, col, el, icon, row, text,
};

use super::widgets::Keyboard;
use super::{Paint, run};
use crate::fmt;
use crate::icons::{self, IconPair};
use crate::service::RuneSink;
use crate::state::transfers::{Bar, Dock, Flourish, Notice, OsEffect, PULSE, STARTED};
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
/// The toolbar button's width and height.
const BUTTON_SIZE: f32 = 28.0;

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
        .size(BUTTON_SIZE, BUTTON_SIZE)
        .radius(radius::ROW)
        .center()
        .hover(t.hover)
        .press(t.press)
        .press_scale(0.97)
        .press_motion(rune::Anim::INSTANT, motion::SNAP)
        .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::ToggleTransfers, tx))
        .has_popup(rune::HasPopup::Dialog)
        .tab_press();
    if tr.open {
        b = b.fill(t.press);
    }
    // A job that just finished keeps the ring (full, once nothing else runs) while its bar
    // fills and the ring pulses; under reduce motion the ring goes with the last active job.
    let fraction = if p.reduced { tr.ring().map(|r| r.fraction()) } else { tr.ring_fraction() };
    if let Some(f) = fraction {
        b = b.child(ring(p, f));
    }
    if let Some(n) = tr.flourish.pulse().filter(|_| !p.reduced) {
        b = b.child(pulse(p, n));
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
    let turned =
        |e: El, angle: f32| e.rotate(angle).rotate_motion(motion::MOVE).opacity(if angle > 0.0 { 1.0 } else { 0.0 });
    let first = turned(half(0, t.accent).absolute(-c, 0.0), f.min(0.5) * 2.0 * PI);
    let second = turned(half(1, t.accent).absolute(0.0, 0.0), (f - 0.5).max(0.0) * 2.0 * PI);
    // The track is opaque: a translucent stroke darkens where its segments overlap.
    let track_color = flatten(t.border_strong, t.surface);
    // The ring's drawing says nothing the progress bar's value doesn't.
    el().key("tb-ring")
        .role("progressbar")
        .label(format!("{:.0}%", f * 100.0))
        .absolute((28.0 - RING) / 2.0, (28.0 - RING) / 2.0)
        .size(RING, RING)
        .child(
            canvas(Shape::new().stroke_svg(&track, RING_STROKE, LineCap::Round, track_color))
                .absolute(0.0, 0.0)
                .size(RING, RING)
                .a11y_hidden(),
        )
        .child(el().absolute(c, 0.0).size(c, RING).clip_soft(0.0).child(first.a11y_hidden()))
        .child(el().absolute(0.0, 0.0).size(c, RING).clip_soft(0.0).child(second.a11y_hidden()))
}

/// The ring's pulse: a copy of it in the accent that grows out of it and fades, once.
const PULSE_BEATS: &[Beat] =
    &[Beat::new(0.0).opacity(0.8), Beat::new(PULSE).opacity(0.0).scale(1.4).easing(Easing::EASE_OUT)];

/// Pulse `n` of the ring (each one a new element, so each plays), over the ring.
fn pulse(p: Paint, n: u32) -> El {
    let r = (RING - RING_STROKE) / 2.0;
    let c = RING / 2.0;
    let (top, bottom) = (c - r, c + r);
    let circle = format!("M{c} {top} A{r} {r} 0 0 1 {c} {bottom} A{r} {r} 0 0 1 {c} {top}Z");
    canvas(Shape::new().stroke_svg(&circle, RING_STROKE, LineCap::Round, p.t.accent))
        .key(format!("tb-pulse-{n}"))
        .absolute((28.0 - RING) / 2.0, (28.0 - RING) / 2.0)
        .size(RING, RING)
        .sequence(Sequence::new(PULSE_BEATS))
        .motion_intent(MotionIntent::PULSE)
        .a11y_hidden()
}

/// The download-started circle's diameter and its glyph's size.
const STARTED_SIZE: f32 = 48.0;
const STARTED_GLYPH: f32 = 20.0;
/// How far below the button the rise ends, and how much room the content area needs for the
/// animation to play at all (the circle and this much), as Chrome has it.
const STARTED_GAP: f32 = 40.0;
/// Where the rise ends: the circle centred [`STARTED_GAP`] below the button's bottom, this far
/// below its centre.
const STARTED_TURN: f32 = BUTTON_SIZE / 2.0 + STARTED_GAP;
/// The rise's share of [`STARTED`] (Chrome's 60 %), in seconds.
const STARTED_RISE: f64 = 0.3;

/// The download-started animation after its rise ([`started`] carries the rise): it fades in
/// on the way up, then goes into the button as it fades out, at an even speed throughout.
const STARTED_BEATS: &[Beat] = &[
    Beat::new(0.0).opacity(0.0),
    Beat::new(STARTED_RISE).opacity(1.0).easing(Easing::LINEAR),
    Beat::new(STARTED).opacity(0.0).translate(0.0, -STARTED_TURN).easing(Easing::LINEAR),
];

/// Where the download-started animation flies, from the laid-out window (in window points):
/// the button's centre, and the middle of the content area between the toolbar and the
/// status bar.
#[derive(Clone, Copy, Debug)]
pub struct Flight {
    x: f32,
    button: f32,
    from: f32,
}

/// The [`Flight`] of the download-started animation while one plays, `None` when the content
/// area hasn't room for it. Read only then, so layout changes don't re-run the view otherwise.
pub fn flight(s: &App, cx: &Cx<App>) -> Option<Flight> {
    s.transfers.flourish.started()?;
    // All three every time: the first run doesn't know them yet, and a commit re-runs the view
    // only twice more for reads it hasn't seen, so stopping at the first unknown one would
    // leave the last one unread.
    let (button, top, bottom) =
        (cx.layout(BUTTON), cx.layout_with("toolbar", |r| r[1] + r[3]), cx.layout_with("status", |r| r[1]));
    let (b, top, bottom) = (button?, top?, bottom?);
    let roomy = bottom - top >= STARTED_SIZE + STARTED_GAP;
    roomy.then_some(Flight { x: b[0] + b[2] / 2.0, button: b[1] + b[3] / 2.0, from: (top + bottom) / 2.0 })
}

/// Download-started animation `n` (each one a new element, so each plays), on its `flight`
/// over the main column, x fixed on the button's centre. Under reduce motion nothing plays.
/// Decoration: never focused, never read, and the pointer passes through it.
pub fn started(s: &App, p: Paint, flight: Option<Flight>) -> Option<El> {
    let n = s.transfers.flourish.started().filter(|_| !p.reduced)?;
    let f = flight?;
    let half = STARTED_SIZE / 2.0;
    let turn = f.button + STARTED_TURN;
    // Beats are fixed, and how far the rise goes is the window's: a slide, in the carrier's
    // own heights, takes it from the content area's middle up to the turn; the beats do the
    // rest.
    let rise = LoopSpec::slide(STARTED_RISE, [0.0, (f.from - turn) / STARTED_SIZE], [0.0, 0.0]).easing(Easing::LINEAR);
    let circle = el()
        .size(STARTED_SIZE, STARTED_SIZE)
        .radius(half)
        .center()
        .fill(p.t.accent)
        .sequence(Sequence::new(STARTED_BEATS))
        .child(icon(icons::DOWNLOAD.at(p.scale), STARTED_GLYPH).color(p.t.on_accent));
    let carrier = el()
        .key(format!("tb-started-{n}"))
        // At rest on whole points, its glyph on the pixel grid where it turns.
        .absolute((f.x - half).round(), (turn - half).round())
        .size(STARTED_SIZE, STARTED_SIZE)
        .looping(rise)
        .loop_times(1)
        // It shows whole as it mounts (the beats fade it) and is gone the frame it ends.
        .transition(Transition::none())
        // An even speed that changes at the turn, and a fade in and out.
        .motion_intent(MotionIntent::IMPULSE | MotionIntent::PULSE)
        .inert()
        .a11y_hidden()
        .child(circle);
    Some(carrier)
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
        // The button's name says the count.
        .a11y_hidden()
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
            // One Tab stop, the open tab; ← → switch (tabs follow focus).
            .tab_stop()
            .focus_selected(on)
            .grow(1.0)
            .h(24.0)
            .gap(6.0)
            .radius(radius::ROW - 1.0)
            .center()
            .motion(motion::SNAP)
            .child(text(label).font(if on { type_::BODY_STRONG } else { type_::BODY }).color(if on {
                t.text
            } else {
                t.text_2
            }))
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
        .label("Transfers")
        .focus_group()
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
    let rows: Vec<El> = s.transfers.in_tab(tab).map(|j| job_row(p, j, s.transfers.flourish.get(j.id))).collect();
    let body = if rows.is_empty() {
        let (headline, hint) = match tab {
            Tab::Downloads => {
                ("No downloads yet", format!("Select files and press {}", super::widgets::chord("mod+d")))
            }
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

/// The tile left of a row: the job's arrow, a warning once it failed, the mint check once it's
/// done. While a done job's bar still fills (`flourishing`) the arrow stays and the check
/// waits, small and clear; then the arrow goes and the check lands on `settle`.
fn tile(p: Paint, j: &JobView, flourishing: bool) -> El {
    let t = p.t;
    let arrow: &IconPair = if j.kind == JobKind::Download { &icons::DOWNLOAD } else { &icons::UPLOAD };
    let tile = el().size(28.0, 28.0).radius(radius::ROW).fill(t.field);
    let inset = (28.0 - size::ICON) / 2.0;
    match &j.state {
        TaskState::Done => {
            // The row's name says "Done": the check is its picture.
            let check = icon(icons::CHECK.at(p.scale), size::ICON)
                .key(format!("tr-check-{}", j.id.0))
                .a11y_hidden()
                .color(t.accent)
                .absolute(inset, inset)
                .scale(if flourishing { 0.5 } else { 1.0 })
                .opacity(if flourishing { 0.0 } else { 1.0 })
                .scale_motion(motion::settle(p.reduced))
                .opacity_motion(motion::MOVE);
            let tile = if flourishing {
                tile.child(icon(arrow.at(p.scale), size::ICON).color(t.text_2).absolute(inset, inset))
            } else {
                tile
            };
            // Reduce motion: the check is just there (no cross-fade either).
            let tile = if p.reduced { tile.instant_changes() } else { tile };
            tile.child(check)
        }
        TaskState::Failed { .. } => tile.center().child(icon(icons::WARNING.at(p.scale), size::ICON).color(t.danger)),
        _ => tile.center().child(icon(arrow.at(p.scale), size::ICON).color(t.text_2)),
    }
}

/// One job's row; `flourish` is its done flourish, if one plays (none under reduce motion).
fn job_row(p: Paint, j: &JobView, flourish: Option<Flourish>) -> El {
    let t = p.t;
    let id = j.id;
    let failed = matches!(j.state, TaskState::Failed { .. });
    let flourish = flourish.filter(|_| !p.reduced);
    // A done job's bar has collapsed into the check: at rest, or as its flourish ends.
    let collapsed = j.state == TaskState::Done && flourish.is_none();
    let tile = tile(p, j, flourish.is_some());

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
            .tab_press()
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
            .tab_press()
            .child(text("Resolve…").font(type_::META_STRONG).color(t.accent))
    });
    let title = row()
        .w_full()
        .h(22.0)
        .gap(space::S1)
        .center_y()
        .child(
            text(j.title.clone())
                .key(format!("tr-title-{}", id.0))
                .font(type_::BODY)
                .color(t.text)
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .children(resolve)
        .child(row().gap(2.0).group_opacity(0.0, 1.0).children(buttons));
    let progress = progress_bar(p, j, flourish, collapsed);
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
        r = r
            .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::RevealTransfer(id), tx))
            .help(label)
            .tab_press()
            .focus_ring(2.0, -1.0, t.focus);
    }
    // What failed or was left out says so on hover, over the reveal hint.
    if let Some(d) = details(j) {
        r = r.help(d);
    }
    r
}

/// A job's progress bar, in the state's colour; `collapsed` (a done job at rest, or as its
/// flourish ends) shrinks it into the tile.
fn progress_bar(p: Paint, j: &JobView, flourish: Option<Flourish>, collapsed: bool) -> El {
    let t = p.t;
    let (fraction, bar) = match &j.state {
        TaskState::Done => (if let Some(Flourish::Finished { from }) = flourish { from } else { 1.0 }, t.accent),
        TaskState::Failed { .. } => (j.progress.fraction().max(0.02), t.danger),
        TaskState::Paused | TaskState::Cancelled => (j.progress.fraction(), t.text_3),
        _ if j.waiting_for_connection || j.conflict.is_some() => (j.progress.fraction(), t.text_3),
        _ => (j.progress.fraction(), t.accent),
    };
    // Collapsed, the bar keeps its room (the row doesn't change height), drawn shrunk into its
    // left end, by the tile, and clear.
    let mut progress = el()
        .key(format!("tr-bar-{}", j.id.0))
        .w_full()
        .h(3.0)
        .mt(3.0)
        .radius(1.5)
        .fill(t.border)
        .origin(0.0, 0.5)
        .scale(if collapsed { 0.0 } else { 1.0 })
        .opacity(if collapsed { 0.0 } else { 1.0 })
        .scale_motion(motion::MOVE)
        .opacity_motion(motion::MOVE)
        .child(el().w_frac(fraction).h_full().radius(1.5).fill(bar).motion(motion::MOVE));
    if !collapsed {
        progress = progress.role("progressbar").label(format!("{:.0}%", fraction * 100.0));
    } else if p.reduced {
        // Reduce motion: no collapse, the bar is just gone.
        progress = progress.instant_changes();
    }
    progress
}

/// "3 active" and Clear finished.
fn footer(s: &App, p: Paint) -> El {
    let t = p.t;
    let active = s.transfers.active();
    let summary =
        if active == 0 { "Nothing running".to_owned() } else { format!("{} active", fmt::thousands(active as u64)) };
    let can_clear = s.transfers.has_finished();
    let mut clear = row()
        .key("tr-clear")
        .role("button")
        .label("Clear finished")
        .h(24.0)
        .px(space::S2)
        .radius(radius::ROW)
        .center()
        .child(text("Clear finished").font(type_::META_STRONG).color(if can_clear {
            t.text_2
        } else {
            t.text_3.alpha(0.6)
        }));
    clear = if can_clear {
        clear
            .hover(t.hover)
            .press(t.press)
            .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::Transfer(TransferCommand::ClearFinished), tx))
            .tab_press()
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

/// The app's poll (`AppSpec::poll`, at the start of each frame): steps the done flourishes and
/// the ring's pulse on the window's clock. A step that changes what shows animates: it isn't
/// input's doing, and Rune would land it at once otherwise.
pub fn poll(s: &mut App, clock: f64, tx: &mut Tx) -> bool {
    // What finished, failed, dropped or came back since the last frame, for screen readers
    // (§6.9); it changes nothing on screen.
    let mut announcer = std::mem::take(&mut s.ui.announcer);
    if let Some(news) = announcer.news(s) {
        tx.announce(news, rune::LiveRegion::Polite);
    }
    s.ui.announcer = announcer;
    let step = s.transfers.flourish.step(clock);
    if let Some(due) = step.wake {
        tx.wake_at(due);
    }
    if step.changed {
        tx.animated();
    }
    step.changed
}

// Outside the window.

/// The app's identity for notifications: the bundle id on macOS (which reads it from the
/// bundle, `packaging/macos/Info.plist`), the AppUserModelID on Windows, the desktop entry
/// (`dromon.desktop`) on Linux.
const NOTIFY_ID: &str = if cfg!(target_os = "linux") { "dromon" } else { "dev.dromon.Dromon" };
/// The user-info key a notification carries its job under.
const NOTIFY_JOB: &str = "job";

/// The app's way to notifications once it's hooked up to them ([`connect_os`]): the poster
/// thread. Every main window's sink holds it ([`connect_window`]).
#[derive(Clone)]
pub struct OsLink {
    notices: mpsc::Sender<Notice>,
}

/// Hooks the app up to notifications, once per run: registers its identity, starts the
/// `dromon-notify` thread that posts them (a first post on Linux waits for the session bus),
/// and has a click on one bring the app forward with the popover open on its job, in the
/// focused main window or else the leading one. `windows` are the main windows. Call it
/// before the first window opens (Windows takes the app's identity then). A headless run
/// (`--snap`, `--script`, tests) gets none of it (`None`): nothing it does reaches the OS.
pub fn connect_os(windows: RuneSink) -> Option<OsLink> {
    if rune::tooling::headless_run().is_some() {
        return None;
    }
    if let Err(e) = rune_notify::register(&rune_notify::Identity::new(NOTIFY_ID, "Dromon")) {
        eprintln!("dromon: no notifications: {e}");
    }
    rune_notify::on_click(move |click| clicked(&windows, &click));
    Some(OsLink { notices: spawn_poster() })
}

/// From now on `s`'s taskbar effects go to `window` (its native window, each time it's made)
/// and its notifications to the poster, and the taskbar shows again what it showed. Every main
/// window has the sink; only the leading one's `Watch` uses it (`App::leads`), so there's one
/// badge and one notification per job for the whole app.
pub fn connect_window(s: &mut App, window: WindowHandle, os: OsLink) {
    let notices = os.notices;
    s.transfers.watch.os.connect(move |fx| match fx {
        OsEffect::Taskbar(dock) => show_taskbar(&window, &dock),
        // The OS's own word on focus as well (macOS, Windows), in case a focus change hasn't
        // reached the app yet: a banner while the user looks at the app is noise.
        OsEffect::Notify(n) => {
            if !rune_notify::app_focused() {
                let _ = notices.send(n);
            }
        }
    });
    s.transfers.watch.reapply();
}

/// Windows: the bar under the taskbar button and the count over it; macOS: the count on the
/// Dock icon (the Dock has no bar). Linux shows neither (Rune has no badge there). On the UI
/// thread.
fn show_taskbar(window: &WindowHandle, dock: &Dock) {
    if cfg!(not(any(target_os = "macos", target_os = "windows"))) {
        return;
    }
    let progress = match dock.bar {
        Bar::None => TaskbarProgress::None,
        Bar::Busy => TaskbarProgress::Busy,
        Bar::Paused => TaskbarProgress::Paused,
        Bar::Error => TaskbarProgress::Error,
    };
    window.set_taskbar(Taskbar::new().progress(progress).badge(dock.count, dock.description()));
}

/// A thread that posts notifications as they come, so posting never waits on the UI thread.
/// It ends with the app.
fn spawn_poster() -> mpsc::Sender<Notice> {
    let (tx, rx) = mpsc::channel::<Notice>();
    let posting = std::thread::Builder::new().name("dromon-notify".into()).spawn(move || {
        while let Ok(n) = rx.recv() {
            let note = rune_notify::Notification::new(n.id, n.title).body(n.body).info(NOTIFY_JOB, n.job.0.to_string());
            rune_notify::post(&note);
        }
    });
    if let Err(e) = posting {
        eprintln!("dromon: no notifications: {e}");
    }
    tx
}

/// A notification was clicked (on any thread). Which main window shows it is the windows'
/// state's to say, on the UI thread (`App::notice_window`): the focused one, else the leading
/// one. Any window asks, then that one alone comes forward with the popover open on the job.
fn clicked(windows: &RuneSink, click: &rune_notify::Click) {
    let token = click.activation_token().map(str::to_owned);
    let job = click.get(NOTIFY_JOB).and_then(|v| v.parse().ok()).map(JobId);
    let Some((_, any)) = windows.handles().into_iter().next() else { return };
    let windows = windows.clone();
    any.update_if(move |s, _| {
        let to = s.notice_window();
        if let Some((_, handle)) = windows.handles().into_iter().find(|(w, _)| Some(*w) == to) {
            show_clicked(&handle, token, job);
        }
        false
    });
}

/// The clicked notification's window comes forward (Wayland with the server's activation
/// token) and its popover opens on the job.
fn show_clicked(handle: &AppHandle<App>, token: Option<String>, job: Option<JobId>) {
    match token {
        Some(token) => handle.activate(token),
        None => handle.open(),
    }
    handle.update_if(move |s, _| {
        // Windows keeps the foreground from a process that doesn't hold it; this asks the way
        // a clicked toast's app may.
        rune_notify::bring_to_front();
        if let Some(job) = job {
            s.show_transfer(job);
        }
        true
    });
}
