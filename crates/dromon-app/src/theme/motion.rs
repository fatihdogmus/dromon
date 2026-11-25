//! The three springs of §6.6. Everything that moves uses one of these, so all motion retargets
//! from where it is and nothing bounces unless it lands after momentum.
//!
//! Reduce motion (`theme::Access::reduce_motion`, `Paint::access`): Rune reduces every commit
//! the view marks with `Cx::reduced_motion` (it is, whenever the OS or `UiState::access` asks):
//! slides, scales and turns land at once, opacity and colour cross-fade (Rune's `REDUCED_FADE`,
//! 0.18 s), sequences such as a field's shake land on their last beat, loops (spinners, the
//! skeleton's pulse) hold still. What a view picks itself goes through [`settle`].

use rune::Spring;

/// 0.18 s, no bounce: hover and press fills, selection, checkbox, tab indicator.
pub const SNAP: Spring = Spring::new(0.18, 0.0);
/// 0.30 s, no bounce: popovers and sheets, row insert/remove, list ⇄ empty state, banners.
pub const MOVE: Spring = Spring::new(0.30, 0.0);
/// 0.45 s, bounce 0.15: only after momentum (a drag landing, the done flourish, the connected
/// dot coming on).
pub const SETTLE: Spring = Spring::new(0.45, 0.15);

/// [`SETTLE`], without its bounce under reduce motion (§6.6).
pub fn settle(reduce_motion: bool) -> Spring {
    if reduce_motion { Spring::new(0.45, 0.0) } else { SETTLE }
}
