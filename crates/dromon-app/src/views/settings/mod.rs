//! The Settings page (§6.8, §8): in the main window, in place of the toolbar, the list and the
//! status bar, with the sidebar beside it (`state/page.rs`). A header with Back and the title;
//! under it the list of sections on the left and the chosen section's controls on the right:
//!
//! - Appearance: Theme (System / Dark / Light) and Density (Comfortable / Compact), segmented.
//! - Transfers: the download folder in mono with Choose… (the platform's folder picker) and
//!   Reset (back to the OS Downloads folder), and Transfers at once, a 1–8 stepper.
//! - Browsing: Show hidden files by default, a switch.
//! - General: the Terminal "Open in Terminal" uses, one choice per line: System default and the
//!   terminal apps found on this machine (looked for when the section shows). Check for updates
//!   automatically, a switch; Check now, with what the last check found and, for a newer
//!   version, Update (or Copy command) and Release notes. Live updates when the server supports
//!   them, a switch.
//!
//! The list is a tablist: one Tab stop, ↑ ↓ move between the sections and the section follows,
//! Tab goes on to the controls, the section's tabpanel. Every control's change goes to the
//! window's state as `Action::Setting`; the page shows the state's values back
//! (`App::settings_view`), so a change made in another window shows here too.
//!
//! Each section is a file of its own (`appearance.rs`, `transfers.rs`, `browsing.rs`,
//! `general.rs`); the rows and controls they share are here.

mod appearance;
mod browsing;
mod general;
mod transfers;

use rune::{Color, El, Orientation, ScrollSpec, Tx, col, el, row, text};

use super::widgets::{Keyboard, icon_button};
use super::{Paint, run};
use crate::icons;
use crate::state::{Action, App, SettingsChange, SettingsSection};
use crate::theme::{motion, radius, size, space, type_};

/// The page's key.
pub const PAGE: &str = "settings";
/// The chosen section's controls.
const PANEL: &str = "settings-panel";
/// The list of sections' width.
const SECTIONS_WIDTH: f32 = 176.0;
/// The widest a section's rows get, so a label and its control stay together.
const PANEL_WIDTH: f32 = 560.0;

/// The page, over the folder view (which stays underneath as it was, `views::view`).
pub fn page(s: &App, section: SettingsSection, p: Paint) -> El {
    let t = p.t;
    fn back(s: &mut App, tx: &mut Tx) {
        run(s, Action::LeaveSettings, tx)
    }
    // One piece with the title bar, as the toolbar: it drags the window and leaves room for
    // Windows' caption buttons.
    let header = row()
        .key("settings-header")
        .w_full()
        .h(size::TOOLBAR)
        .pl(space::S2)
        .pr(space::S3 + p.insets.trailing)
        .gap(space::S2)
        .center_y()
        .border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle)
        .window_drag()
        .child(icon_button(p, "settings-back", "Back", &icons::CHEVRON_LEFT, Some(back)))
        .child(text("Settings").key("settings-title").font(type_::TITLE).color(t.text));
    let body = row().w_full().grow(1.0).min_h(0.0).child(sections(section, p)).child(panel(s, section, p));
    col()
        .key(PAGE)
        .role("group")
        .label("Settings")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(t.surface)
        .child(header)
        .child(body)
}

/// The sections, the chosen one highlighted.
fn sections(current: SettingsSection, p: Paint) -> El {
    let t = p.t;
    let items = SettingsSection::ALL.into_iter().map(|section| {
        let on = section == current;
        row()
            .key(section.key())
            .role("tab")
            .label(section.title())
            .controls(PANEL)
            // The list's Tab stop is the chosen section; the arrows move it (tabs follow focus).
            .tab_stop()
            .focus_selected(on)
            .w_full()
            .h(size::ROW)
            .px(space::S2)
            .radius(radius::ROW)
            .center_y()
            .fill(if on { t.active } else { Color::TRANSPARENT })
            .hover(t.hover)
            .press(t.press)
            .motion(motion::SNAP)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::SettingsSection(section), tx))
            .child(
                text(section.title())
                    .font(if on { type_::BODY_STRONG } else { type_::BODY })
                    .color(if on { t.text } else { t.text_2 })
                    .truncate(),
            )
    });
    col()
        .key("settings-sections")
        .role("tablist")
        .label("Settings sections")
        .orientation(Orientation::Vertical)
        .focus_group()
        .w(SECTIONS_WIDTH)
        .h_full()
        .padding(space::S2)
        .gap(2.0)
        .border_sides([0.0, p.hair, 0.0, 0.0], t.border_subtle)
        .children(items)
}

/// The chosen section's controls. Paths show whole without a home folder (scenes, tests).
fn panel(s: &App, section: SettingsSection, p: Paint) -> El {
    let v = s.settings_view();
    let page = &s.ui.settings_page;
    let body = match section {
        SettingsSection::Appearance => appearance::appearance(&v, p),
        SettingsSection::Transfers => transfers::transfers(&v, page.home.as_deref(), page.error.as_deref(), p),
        SettingsSection::Browsing => browsing::browsing(&v, p),
        SettingsSection::General => general::general(s, &v, &page.terminal_choices(v.settings.terminal), p),
    };
    col()
        .key(PANEL)
        .role("tabpanel")
        .label(section.title())
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .scroll(ScrollSpec::default())
        .px(space::S6)
        .py(20.0)
        .child(body.max_w(PANEL_WIDTH))
}

pub(super) fn change(s: &mut App, change: SettingsChange, tx: &mut Tx) {
    run(s, Action::Setting(change), tx)
}

/// A caption and its rows.
pub(super) fn section(p: Paint, title: &'static str, rows: Vec<El>) -> El {
    let caption = text(title.to_uppercase()).font(type_::CAPTION).color(p.t.text_3);
    col()
        .key(format!("section-{}", title.to_lowercase()))
        .role("group")
        .label(title)
        .w_full()
        .gap(space::S2)
        .child(caption)
        .children(rows)
}

/// A label (with a line under it, maybe) on the left, its control on the right.
pub(super) fn setting(p: Paint, label: &'static str, sub: Option<El>, control: El) -> El {
    let words = col().grow(1.0).min_w(0.0).gap(2.0).child(text(label).font(type_::BODY).color(p.t.text)).children(sub);
    row().w_full().min_h(32.0).gap(space::S4).center_y().child(words).child(control)
}

pub(super) fn sub(p: Paint, line: &str) -> El {
    text(line.to_owned()).font(type_::META).color(p.t.text_2).truncate()
}

/// A segmented control of `options` (value, label, key), `current` selected; each segment is
/// `width` wide. Arrow keys move between the segments (a focus group); Space picks one.
pub(super) fn segmented<T: Copy + PartialEq + 'static>(
    p: Paint,
    key: &'static str,
    label: &'static str,
    width: f32,
    options: &[(T, &'static str, &'static str)],
    current: T,
    to: fn(T) -> SettingsChange,
) -> El {
    let t = p.t;
    let segments = options.iter().map(|&(value, name, key)| segment(p, value, name, key, value == current, width, to));
    row()
        .key(key)
        .role("radiogroup")
        .label(label)
        .focus_group()
        .h(28.0)
        .padding(2.0)
        .gap(2.0)
        .radius(radius::ROW + 1.0)
        .fill(t.field)
        .children(segments)
}

/// One choice of a radio group, `width` wide; `on` is the chosen one. Space picks it.
pub(super) fn segment<T: Copy + 'static>(
    p: Paint,
    value: T,
    name: &'static str,
    key: &'static str,
    on: bool,
    width: f32,
    to: fn(T) -> SettingsChange,
) -> El {
    let t = p.t;
    let e = row()
        .key(key)
        .role("radio")
        .label(name)
        .checked(on)
        .focusable(true)
        .keyboard_focus_only()
        .focus_selected(on)
        .w(width)
        .h(24.0)
        .radius(radius::ROW - 1.0)
        .center()
        .motion(motion::SNAP)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| change(s, to(value), tx))
        .child(text(name).font(if on { type_::BODY_STRONG } else { type_::BODY }).color(if on {
            t.text
        } else {
            t.text_2
        }));
    if on { e.fill(t.press).border(p.hair, t.border_subtle) } else { e.hover(t.hover).press(t.press) }
}

/// A bordered button with a body-size label, as the sheets' secondary ones.
pub(super) fn button(
    p: Paint,
    key: impl Into<String>,
    label: &'static str,
    describe: &'static str,
    f: impl Fn(&mut App, &mut Tx) + 'static,
) -> El {
    let t = p.t;
    row()
        .key(key)
        .role("button")
        .label(describe)
        .focusable(true)
        .keyboard_focus_only()
        .h(28.0)
        .px(space::S3)
        .radius(radius::ROW)
        .center()
        .border(p.hair, t.border)
        .hover(t.hover)
        .press(t.press)
        .press_scale(0.97)
        .press_motion(rune::Anim::INSTANT, motion::SNAP)
        .on_press_tx(f)
        .child(text(label).font(type_::BODY).color(t.text))
}

/// An on/off switch: accent track and the knob on the right when on; `to` is the change a
/// press makes.
pub(super) fn switch(p: Paint, key: &'static str, label: &'static str, on: bool, to: fn(bool) -> SettingsChange) -> El {
    let t = p.t;
    let knob = el()
        .size(14.0, 14.0)
        .radius(7.0)
        .fill(if on { t.on_accent } else { t.text_2 })
        .offset(if on { 14.0 } else { 0.0 }, 0.0);
    row()
        .key(key)
        .role("switch")
        .label(label)
        .checked(on)
        .focusable(true)
        .keyboard_focus_only()
        .w(32.0)
        .h(18.0)
        .padding(2.0)
        .radius(9.0)
        .center_y()
        .fill(if on { t.accent } else { t.field })
        .border(p.hair, if on { t.accent } else { t.border_strong })
        .motion(motion::SNAP)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| change(s, to(!on), tx))
        .child(knob)
}
