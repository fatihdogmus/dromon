//! Window frames (§4.7 "window frames"): where a main window was, remembered between launches,
//! and where a window opens. Pure geometry in desktop pixels; `crate::windows` reads the real
//! windows and displays.
//!
//! - At launch the first window reopens on its last frame ([`Saved::frames`]'s first), moved
//!   onto a display that's there now and shrunk to fit it ([`fit`]); only that one reopens.
//! - ⌘N's window opens on its opener's frame, a step right and down ([`cascade`]).
//!
//! [`Saved::frames`]: super::Saved::frames

use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "frames_tests.rs"]
mod tests;

/// A window's place: its frame's top-left (`WindowOptions::position`) and its content's size
/// (`WindowOptions::physical_size`), in desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// It was maximized; `x`..`height` are the frame it returns to.
    #[serde(default)]
    pub maximized: bool,
}

/// A display's usable area, in the same desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// How far each new window steps from its opener, in points (× the display's scale).
pub const CASCADE: f64 = 28.0;

/// Pixels of a frame each way that must be on a display for it to count as on it.
const ON: i64 = 48;

impl Frame {
    /// How many pixels of the frame `a` holds.
    fn overlap(&self, a: &Area) -> i64 {
        let w = span(self.x, self.width, a.x, a.width);
        let h = span(self.y, self.height, a.y, a.height);
        if w >= ON.min(i64::from(self.width)) && h >= ON.min(i64::from(self.height)) { w * h } else { 0 }
    }
}

/// The length two ranges share.
fn span(at: i32, len: u32, area_at: i32, area_len: u32) -> i64 {
    let (a0, a1) = (i64::from(at), i64::from(at) + i64::from(len));
    let (b0, b1) = (i64::from(area_at), i64::from(area_at) + i64::from(area_len));
    (a1.min(b1) - a0.max(b0)).max(0)
}

/// `at`, moved so that `len` from it lies within the area (its start wins when it can't).
fn inside(at: i32, len: u32, area_at: i32, area_len: u32) -> i32 {
    let last = i64::from(area_at) + i64::from(area_len) - i64::from(len);
    i64::from(at).min(last).max(i64::from(area_at)) as i32
}

/// The frame placed in `a`: no bigger than it, moved inside it.
fn within(f: Frame, a: &Area) -> Frame {
    let (width, height) = (f.width.min(a.width), f.height.min(a.height));
    Frame { x: inside(f.x, width, a.x, a.width), y: inside(f.y, height, a.y, a.height), width, height, ..f }
}

/// Where `frame` opens on `displays` (usable areas, the primary first): on the display holding
/// most of it, else on the primary, shrunk to fit and moved inside it. `None` without displays
/// (the system places the window) or for an empty frame.
pub fn fit(frame: Frame, displays: &[Area]) -> Option<Frame> {
    if frame.width == 0 || frame.height == 0 {
        return None;
    }
    let best =
        displays.iter().map(|a| (frame.overlap(a), a)).filter(|(n, _)| *n > 0).max_by_key(|(n, _)| *n).map(|(_, a)| a);
    best.or(displays.first()).map(|a| within(frame, a))
}

/// Where a window opened from one at `from` goes: `step` pixels right and down, on the display
/// `from` is on; past that display's right or bottom edge it starts again at its top-left.
pub fn cascade(from: Frame, step: i32, displays: &[Area]) -> Frame {
    let next = Frame { x: from.x.saturating_add(step), y: from.y.saturating_add(step), maximized: false, ..from };
    let Some(home) = fit(from, displays).and_then(|f| displays.iter().find(|a| f.overlap(a) > 0).copied()) else {
        return next;
    };
    let right = i64::from(home.x) + i64::from(home.width);
    let bottom = i64::from(home.y) + i64::from(home.height);
    let fits =
        i64::from(next.x) + i64::from(next.width) <= right && i64::from(next.y) + i64::from(next.height) <= bottom;
    let at = if fits { next } else { Frame { x: home.x.saturating_add(step), y: home.y.saturating_add(step), ..next } };
    within(at, &home)
}
