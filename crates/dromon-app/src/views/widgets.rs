//! Small shared pieces: icon buttons, keyboard hints, status dots.

use rune::{Color, El, Tx, el, icon, row, text};

use super::Paint;
use crate::icons::IconPair;
use crate::state::App;
use crate::theme::{motion, radius, size, type_};

/// A 28 pt square toolbar button around a 16 pt glyph. Fills on press in the same frame and
/// scales to 0.97 (§6.6); without an action it draws dimmed and takes nothing.
pub fn icon_button(
    p: Paint,
    key: &'static str,
    label: &'static str,
    glyph: &'static IconPair,
    action: Option<fn(&mut App, &mut Tx)>,
) -> El {
    let t = p.t;
    let b = el()
        .key(key)
        .role("button")
        .label(label)
        .size(28.0, 28.0)
        .radius(radius::ROW)
        .center()
        .child(icon(glyph.at(p.scale), size::ICON).color(if action.is_some() { t.text_2 } else { t.text_3.alpha(0.6) }));
    match action {
        Some(f) => b.hover(t.hover).press(t.press).press_scale(0.97).press_motion(rune::Anim::INSTANT, motion::SNAP).on_press_tx(f),
        None => b.disabled(true),
    }
}

/// A keyboard hint chip: `⌘K`, `⌘F`.
pub fn kbd(p: Paint, label: &str) -> El {
    row()
        .h(18.0)
        .px(5.0)
        .radius(4.0)
        .center()
        .border(p.hair, p.t.border)
        .child(text(label.to_owned()).font(type_::KBD).color(p.t.text_3))
}

/// A shortcut as this platform writes it: "⌘K" on macOS, "Ctrl+K" elsewhere.
pub fn chord(spec: &str) -> String {
    rune::Chord::parse(spec).map(|c| c.label()).unwrap_or_else(|_| spec.to_owned())
}

/// A `size` pt status dot.
pub fn dot(color: Color, size: f32) -> El {
    el().size(size, size).radius(size / 2.0).fill(color)
}
