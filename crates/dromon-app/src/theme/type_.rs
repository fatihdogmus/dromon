//! The type scale (§6.4). Tracking is in points: the table's em values times the size.
//! Every style that shows numbers in columns or the status bar sets tabular figures.

use rune::TextStyle;

const TNUM: &str = "'tnum' 1";

/// Section labels (`CONNECTED`): 11/14, 600, +0.04em, set in upper case by the caller.
pub const CAPTION: TextStyle = TextStyle::new(11.0, 600.0).lh(14.0).tracking(0.44);
/// Size, date, status bar: 12/16, 400, +0.005em, tabular figures.
pub const META: TextStyle = TextStyle::new(12.0, 400.0).lh(16.0).tracking(0.06).features(TNUM);
/// Meta at 500, for the column headers.
pub const META_STRONG: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0).tracking(0.06).features(TNUM);
/// Rows, menus, fields: 13/18, 400.
pub const BODY: TextStyle = TextStyle::new(13.0, 400.0).lh(18.0);
/// Body for selected rows and the current crumb: 500.
pub const BODY_STRONG: TextStyle = TextStyle::new(13.0, 500.0).lh(18.0);
/// Sheet titles: 15/20, 600, −0.005em.
pub const TITLE: TextStyle = TextStyle::new(15.0, 600.0).lh(20.0).tracking(-0.075);
/// Empty-state headlines: 20/24, 600, −0.015em.
pub const DISPLAY: TextStyle = TextStyle::new(20.0, 600.0).lh(24.0).tracking(-0.3);
/// Paths and hosts in mono (Geist Mono), 12/16.
pub const MONO: TextStyle = TextStyle::new(12.0, 400.0).lh(16.0).mono();
/// Keyboard hints (`⌘K`), 11/14.
pub const KBD: TextStyle = TextStyle::new(11.0, 500.0).lh(14.0).features(TNUM);
/// The count badge on the toolbar's transfers button: 10/12, 600, tabular figures.
pub const BADGE: TextStyle = TextStyle::new(10.0, 600.0).lh(12.0).features(TNUM);
