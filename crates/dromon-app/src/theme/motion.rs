//! The three springs of §6.6. Everything that moves uses one of these, so all motion retargets
//! from where it is and nothing bounces unless it lands after momentum.

use rune::Spring;

/// 0.18 s, no bounce: hover and press fills, selection, checkbox, tab indicator.
pub const SNAP: Spring = Spring::new(0.18, 0.0);
/// 0.30 s, no bounce: popovers and sheets, row insert/remove, list ⇄ empty state, banners.
pub const MOVE: Spring = Spring::new(0.30, 0.0);
/// 0.45 s, bounce 0.15: only after momentum (a drag landing, the done flourish, the connected
/// dot coming on).
pub const SETTLE: Spring = Spring::new(0.45, 0.15);
