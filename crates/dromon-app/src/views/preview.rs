//! The quick preview panel (`state::preview`): a card over the right of the list (12 pt radius,
//! sheet elevation) that leaves the rows on its left in reach, so a click on another row, or
//! ↑/↓, moves the preview along. It springs in from the right on `snap` and leaves on `move`;
//! under reduce motion it comes and goes at once.
//!
//! The header has the file's icon and name, what it is (an image's size in pixels, the file's
//! size), **Download** (⌘D) and **Close**. Below it: the image fitted to the panel, labelled
//! with the file's name; or the text in the app's mono face, in blocks of 16 lines with their
//! numbers beside them (the numbers are hidden from screen readers, which read the lines of a
//! region named for the file), only the blocks on screen laid out, and notes when it was cut
//! (to 256 KB, or a line to 2,000 characters); or the file's kind, size and date when there's
//! nothing to show.
//!
//! It's a dialog that keeps Tab among its controls while it has the keyboard; Space and Escape
//! close it from there as from the list, and letters still type-select the rows.

use std::borrow::Cow;

use rune::{
    Align, El, GrowthSpec, Key, KeyEvent, KeyPlatform, LoopSpec, ScrollSpec, SurfaceFit, Transition, Tx, col, el, icon,
    row, text,
};

use super::list::file_icon;
use super::widgets::{Keyboard, chord, icon_button, kbd};
use super::{Paint, effect, run};
use crate::fmt;
use crate::icons;
use crate::state::preview::{
    BLOCK, Body, Cut, IMAGE_LIMIT, LONG_LINE, Phase, Picture, PreviewAction, TEXT_LIMIT, Target, Text,
};
use crate::state::properties::kind_text;
use crate::state::{Action, App, Shortcut, keys};
use crate::theme::{motion, radius, size, space, type_};

/// The panel's key, and its buttons'.
pub const PANEL: &str = "preview";
pub const CLOSE: &str = "preview-close";
pub const DOWNLOAD: &str = "preview-download";

/// The panel's width: a share of the list's, within these.
const MIN_W: f32 = 320.0;
const MAX_W: f32 = 720.0;
/// The labels' column of the details.
const LABEL_W: f32 = 88.0;
/// The text's blocks, each keyed by its index: `preview-lines/<b>`, its numbers `n` and lines
/// `lines`.
pub const LINES: &str = "preview-lines";
/// A line of the mono face (`type_::MONO`): its height, and one column's advance (Geist Mono's
/// 600/1000 em at 12 pt).
pub const LINE_H: f32 = 16.0;
pub const ADVANCE: f32 = 7.2;
/// A whole block's height.
const BLOCK_H: f32 = BLOCK as f32 * LINE_H;

/// The panel over the list, while it's open.
pub fn layer(s: &App, p: Paint) -> Option<El> {
    let phase = &s.ui.preview.phase;
    let target = phase.target()?;
    let t = p.t;
    let transition =
        if p.reduced { Transition::none() } else { Transition::offset(24.0, 0.0).combined(Transition::opacity()) };
    Some(
        col()
            .key(PANEL)
            .role("dialog")
            .label(format!("Preview of {}", target.entry.name))
            // Below the list's column header, which stays whole above it.
            .pin(None, Some(size::HEADER + space::S2), Some(space::S3), Some(space::S3))
            .w_frac(0.6)
            .min_w(MIN_W)
            .max_w(MAX_W)
            .radius(radius::SHEET)
            .fill(t.sheet)
            .border(p.hair, t.elevated_edge())
            .shadow(16.0, 48.0, t.sheet_shadow)
            .transition(transition)
            .transition_motion(motion::SNAP, motion::MOVE)
            .on_key(panel_key)
            .child(header(s, target, phase, p))
            .child(body(phase, p)),
    )
}

/// The controls Tab walks while the panel has the keyboard, in order.
fn controls(s: &App) -> Vec<&'static str> {
    if can_download(s) { vec![DOWNLOAD, CLOSE] } else { vec![CLOSE] }
}

fn can_download(s: &App) -> bool {
    s.conn().is_some_and(|c| c.interactive())
}

/// Keys while the panel has the keyboard: Tab stays among its controls; Space, Escape and
/// typing do what they do from the list.
fn panel_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = ev.mods;
    if ev.key == Key::Tab && !m.cmd && !m.ctrl && !m.alt {
        let controls = controls(s);
        let n = controls.len();
        let at = tx.focused().and_then(|k| controls.iter().position(|c| *c == k));
        let next = match at {
            Some(i) if m.shift => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
            None if m.shift => n - 1,
            None => 0,
        };
        tx.focus(controls[next], true);
        return true;
    }
    match keys::shortcut(ev, KeyPlatform::current()) {
        Some(sc @ (Shortcut::Preview | Shortcut::Escape | Shortcut::Type(_))) => {
            for fx in s.shortcut(sc, tx.clock()) {
                effect(fx, tx);
            }
            true
        }
        _ => false,
    }
}

fn close(s: &mut App, tx: &mut Tx) {
    run(s, Action::Preview(PreviewAction::Close), tx);
}

fn header(s: &App, target: &Target, phase: &Phase, p: Paint) -> El {
    let t = p.t;
    let e = &target.entry;
    let names = col()
        .key("preview-heading")
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .child(text(e.name.clone()).key("preview-title").font(type_::TITLE).color(t.text).truncate())
        .child(text(meta(phase, target)).key("preview-meta").font(type_::META).color(t.text_2).truncate());
    row()
        .key("preview-header")
        .w_full()
        .px(space::S4)
        .py(space::S3)
        .gap(space::S3)
        .center_y()
        .border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle)
        .child(file_icon(p, e, false))
        .child(names)
        .child(download(s, p))
        .child(icon_button(p, CLOSE, "Close", &icons::CLOSE, Some(close)))
}

/// What the file is: an image's size in pixels (once decoded) or its kind, then its size.
fn meta(phase: &Phase, target: &Target) -> String {
    let e = &target.entry;
    let what = match phase {
        Phase::Shown { body: Body::Image(pic), .. } => pic
            .size()
            .map(|(w, h)| format!("{} × {} pixels", fmt::thousands(u64::from(w)), fmt::thousands(u64::from(h)))),
        _ => None,
    };
    let what = what.unwrap_or_else(|| kind_text(e).to_owned());
    if e.kind.is_dir_like() { what } else { format!("{what} · {}", fmt::size(e.size)) }
}

/// Download (⌘D): the file on the panel. Dimmed while the host isn't connected.
fn download(s: &App, p: Paint) -> El {
    let t = p.t;
    let b = row()
        .key(DOWNLOAD)
        .role("button")
        .label("Download")
        .h(28.0)
        .px(space::S2)
        .gap(6.0)
        .radius(radius::ROW)
        .center()
        .border(p.hair, t.border);
    if !can_download(s) {
        return b.disabled(true).child(text("Download").font(type_::BODY).color(t.text_3));
    }
    b.hover(t.hover)
        .press(t.press)
        .press_scale(0.97)
        .press_motion(rune::Anim::INSTANT, motion::SNAP)
        .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::Preview(PreviewAction::Download), tx))
        .tab_press()
        .child(text("Download").font(type_::BODY).color(t.text))
        .child(kbd(p, &chord("mod+d")))
}

fn body(phase: &Phase, p: Paint) -> El {
    match phase {
        Phase::Closed => el(),
        // A text that came while the cursor moved shows once it rests: loading until then.
        Phase::Loading { .. } | Phase::Waiting { .. } => centred(busy(p, "Loading preview…")),
        Phase::Shown { target, body: Body::Text(text) } => text_body(target, text, p),
        Phase::Shown { target, body: Body::Image(pic) } if pic.failed() => {
            details(target, note(p, "This image can't be shown.", false), p)
        }
        Phase::Shown { target, body: Body::Image(pic) } => image_body(target, pic, p),
        Phase::TooLarge { target } => {
            let why = format!("Images larger than {} aren't previewed.", fmt::size(IMAGE_LIMIT));
            details(target, note(p, &why, false), p)
        }
        Phase::Unsupported { target } if target.entry.kind.is_dir_like() => {
            details(target, note(p, "Folders have no preview.", false), p)
        }
        Phase::Unsupported { target } => {
            details(target, note(p, "There's no preview for this kind of file.", false), p)
        }
        Phase::Failed { target, error } => details(target, note(p, &format!("Couldn't preview it: {error}"), true), p),
    }
}

/// The rest of the panel, `child` in its middle.
fn centred(child: El) -> El {
    col().key("preview-body").w_full().grow(1.0).min_h(0.0).padding(space::S6).center().child(child)
}

/// A spinner and what it waits for.
fn busy(p: Paint, words: &str) -> El {
    let spinner = icon(icons::SPINNER.at(p.scale), size::ICON).color(p.t.text_2).looping(LoopSpec::rotation(0.9));
    row()
        .key("preview-loading")
        .role("status")
        .label(words.to_owned())
        .busy(true)
        .gap(space::S2)
        .center_y()
        .child(spinner)
        .child(text(words.to_owned()).font(type_::BODY).color(p.t.text_2))
}

/// Why there's nothing to show; an alert when something went wrong.
fn note(p: Paint, words: &str, alert: bool) -> El {
    let t = text(words.to_owned()).key("preview-note").font(type_::BODY).w_full().wrap();
    if alert { t.role("alert").color(p.t.danger) } else { t.color(p.t.text_2) }
}

/// The text, in blocks of [`BLOCK`] lines with their numbers beside them, and notes when it
/// was cut. Only the blocks on screen (and a little either side) are laid out, so a 256 KB text
/// costs what a screenful does. The lines don't wrap: the content is as wide as the widest
/// line, whether it's laid out or not. A log's opens at its end.
fn text_body(target: &Target, body: &Text, p: Paint) -> El {
    let t = p.t;
    let cut = body.cut.map(|cut| {
        let which = if cut == Cut::Head { "first" } else { "last" };
        format!("Showing the {which} {} of {}", fmt::size(TEXT_LIMIT), fmt::size(body.total))
    });
    let long = body.long.then(|| format!("Long lines are cut at {} characters", fmt::thousands(LONG_LINE as u64)));
    let notes = [("preview-cut", cut), ("preview-long", long)]
        .into_iter()
        .filter_map(|(key, words)| Some(text(words?).key(key).font(type_::META).color(t.text_2)))
        .collect::<Vec<_>>();
    let notes = (!notes.is_empty()).then(|| col().key("preview-notes").px(space::S4).pt(space::S2).children(notes));
    if body.text.is_empty() {
        return centred(note(p, "This file is empty.", false));
    }
    let n = body.lines();
    let gutter = body.numbered.then(|| n.to_string().len() as f32 * ADVANCE);
    let width = 2.0 * space::S3 + gutter.map_or(0.0, |w| w + space::S3) + body.columns as f32 * ADVANCE;
    let said = if n == 1 { "1 line".to_owned() } else { format!("{} lines", fmt::thousands(n as u64)) };
    let blocks = body.blocks();
    let lines = col()
        .key(LINES)
        .role("region")
        .label(format!("Contents of {}, {said}", target.entry.name))
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(blocks, BLOCK_H, |_: &App, b| Cow::Owned(b.to_string()), move |s: &App, b| block(s, b, gutter, p))
        .virtual_rows_version((&target.folder, &target.entry.name, body.total, n))
        // Half a screen either side: what scrolling needs, without laying out screens unseen.
        .virtual_overscan(0.5, 0.25);
    let mut scroll = col().key(format!("preview-scroll-{}", target.entry.name)).w_full().grow(1.0).min_h(0.0);
    if body.cut == Some(Cut::Tail) {
        scroll = scroll.scroll(ScrollSpec::stick_bottom());
    }
    let content = col().key("preview-content").w(width).padding(space::S3).child(lines);
    col().key("preview-body").w_full().grow(1.0).min_h(0.0).children(notes).child(scroll.scroll_xy().child(content))
}

/// Block `b` of the text on the panel: its lines, and their numbers in a `gutter` wide column
/// (none for a cut tail), hidden from screen readers, which read the lines. As tall as its
/// lines: the last block may be shorter.
fn block(s: &App, b: usize, gutter: Option<f32>, p: Paint) -> El {
    let Phase::Shown { body: Body::Text(body), .. } = &s.ui.preview.phase else { return el().h(BLOCK_H) };
    if b >= body.blocks() {
        return el().h(BLOCK_H);
    }
    let h = body.block_lines(b).len() as f32 * LINE_H;
    let numbers = gutter.map(|w| {
        text(body.numbers(b)).key("n").font(type_::MONO).color(p.t.text_3).w(w).align(Align::End).a11y_hidden()
    });
    let lines = text(body.block(b).to_owned()).key("lines").font(type_::MONO).color(p.t.text).selectable(true);
    row().h(h).gap(space::S3).items_start().children(numbers).child(lines)
}

/// The image fitted to the panel; a spinner over it until it's decoded.
fn image_body(target: &Target, pic: &Picture, p: Paint) -> El {
    let mut frame = col().key("preview-body").w_full().grow(1.0).min_h(0.0).padding(space::S4);
    if let Some(image) = &pic.image {
        frame = frame.child(
            el().key("preview-image")
                .role("img")
                .label(target.entry.name.clone())
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .pixels_fit(image.pixels(), SurfaceFit::Fit),
        );
    }
    if !pic.image.as_ref().is_some_and(|i| i.is_ready()) {
        frame = frame.child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center().child(busy(p, "Decoding…")));
    }
    frame
}

/// A file with nothing to show: its kind, size and date, and why.
fn details(target: &Target, why: El, p: Paint) -> El {
    let e = &target.entry;
    let modified = e.mtime.map_or_else(|| "—".to_owned(), |m| fmt::date_time(m, 0));
    let size = if e.kind.is_dir_like() { "—".to_owned() } else { fmt::size_exact(e.size) };
    let fields = col()
        .key("preview-fields")
        .w_full()
        .max_w(400.0)
        .gap(space::S2)
        .padding(space::S3)
        .radius(radius::ROW)
        .fill(p.t.raised)
        .border(p.hair, p.t.border_subtle)
        .child(field(p, "kind", "Kind", kind_text(e).to_owned()))
        .child(field(p, "size", "Size", size))
        .child(field(p, "modified", "Modified", modified));
    col()
        .key("preview-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(space::S6)
        .gap(space::S4)
        .items_center()
        .child(fields)
        .child(col().w_full().max_w(400.0).child(why))
}

fn field(p: Paint, key: &str, label: &'static str, value: String) -> El {
    row()
        .key(format!("preview-{key}"))
        .w_full()
        .gap(space::S3)
        .child(text(label).font(type_::META).color(p.t.text_3).w(LABEL_W).pt(1.0))
        .child(text(value).key(format!("preview-{key}-value")).font(type_::BODY).color(p.t.text).grow(1.0).min_w(0.0))
}
