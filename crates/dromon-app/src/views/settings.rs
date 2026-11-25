//! The Settings window (§6.8, §8): a small second window, 480 × 360, with three sections.
//!
//! - Appearance: Theme (System / Dark / Light) and Density (Comfortable / Compact), segmented.
//! - Transfers: the download folder in mono with Choose… (the platform's folder picker) and
//!   Reset (back to the OS Downloads folder), and Transfers at once, a 1–8 stepper.
//! - Browsing: Show hidden files by default, a switch.
//!
//! The window has its own state, a [`Pane`]: a copy of the app's [`SettingsView`] that the main
//! window refreshes after every change, and where its changes go (`settings_window.rs`). A
//! control's change shows in the pane at once and goes to the main window, whose state is the
//! one that counts (and is shown back). Headless runs render no second window, so the scenes
//! draw [`body`] inside the main window instead ([`preview`]).

use std::path::{Path, PathBuf};

use rune::{Cx, El, FileDialogOptions, Key, KeyEvent, Tx, col, el, row, text};

use super::Paint;
use crate::state::settings::AT_ONCE;
use crate::state::{App, Density, SettingsChange, SettingsView};
use crate::theme::{self, Access, Appearance, motion, radius, space, type_};

/// The window's content size.
pub const SIZE: (f32, f32) = (480.0, 360.0);

/// The Settings window's state.
pub struct Pane {
    /// What the main window last said (or what this window just changed).
    pub view: SettingsView,
    /// The home folder, to write the download folder as `~/…`.
    pub home: Option<PathBuf>,
    /// The folder picker couldn't open (Linux without the desktop portal).
    pub error: Option<String>,
    /// Display preferences forced on over the system's (tests).
    pub access: Access,
    /// Where each change goes: the main window's state.
    send: Box<dyn Fn(SettingsChange)>,
}

impl Pane {
    pub fn new(send: impl Fn(SettingsChange) + 'static) -> Pane {
        Pane { view: SettingsView::default(), home: rune::dirs::home(), error: None, access: Access::default(), send: Box::new(send) }
    }

    /// A control was set: shown here at once, and sent to the main window.
    pub fn change(&mut self, change: SettingsChange) {
        self.error = None;
        self.view.settings = self.view.settings.with(change.clone());
        (self.send)(change);
    }
}

/// Escape and ⌘W (Ctrl+W) close the window; it hides, and ⌘, brings it back as it was.
pub fn on_key(_: &mut Pane, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let close = closes(ev);
    if close {
        tx.close_window();
    }
    close
}

/// Whether `ev` closes the window (window keys nothing focused took).
pub fn closes(ev: &KeyEvent) -> bool {
    matches!(ev.key, Key::Escape) || (matches!(ev.key, Key::Command('w')) && !ev.mods.shift && !ev.mods.alt)
}

pub fn view(s: &Pane, cx: &mut Cx<Pane>) -> El {
    let dark = s.view.settings.theme.is_dark(cx.system_dark());
    let system = Access {
        reduce_motion: cx.system_reduced_motion(),
        reduce_transparency: false,
        increase_contrast: cx.system_increased_contrast(),
    };
    let access = system.or(s.access);
    if access.reduce_motion {
        cx.reduced_motion(true);
    }
    let t = theme::tokens_for(dark, access);
    cx.caption_color(t.text_2);
    cx.foreground(t.text);
    cx.focus_ring(2.0, 1.0, t.focus);
    let scale = cx.env().scale;
    let p = Paint { t, hair: 1.0 / scale, scale, insets: cx.chrome_insets(), reduced: access.reduce_motion, access };
    body(&s.view, s.home.as_deref(), s.error.as_deref(), p).w_full().h_full()
}

/// The main window's stand-in for the Settings window in scenes (`UiState::settings_preview`):
/// [`body`] at the window's size, floating over the main window, taking no input. Paths show
/// whole (no `~`), so the picture is the same for every user.
pub fn preview(s: &App, p: Paint) -> Option<El> {
    if !s.ui.settings_preview {
        return None;
    }
    let window = body(&s.settings_view(), None, None, p)
        .size(SIZE.0, SIZE.1)
        .radius(radius::POPOVER)
        .border(p.hair, p.t.elevated_edge())
        .shadow(16.0, 48.0, p.t.sheet_shadow)
        .inert();
    Some(el().key("settings-preview").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center().child(window))
}

/// The three sections, on the window's surface.
pub fn body(v: &SettingsView, home: Option<&Path>, error: Option<&str>, p: Paint) -> El {
    let t = p.t;
    let s = &v.settings;
    let theme_control = segmented(
        p,
        "theme",
        "Theme",
        72.0,
        &[(Appearance::System, "System", "theme-system"), (Appearance::Dark, "Dark", "theme-dark"), (Appearance::Light, "Light", "theme-light")],
        s.theme,
        SettingsChange::Theme,
    );
    let density_control = segmented(
        p,
        "density",
        "Density",
        100.0,
        &[(Density::Comfortable, "Comfortable", "density-comfortable"), (Density::Compact, "Compact", "density-compact")],
        s.density,
        SettingsChange::Density,
    );
    let appearance = section(
        p,
        "Appearance",
        vec![
            setting(p, "Theme", None, theme_control),
            setting(p, "Density", Some(sub(p, "How tall the rows of the file list are")), density_control),
        ],
    );

    let custom = s.download_folder.is_some();
    let folder_sub = match error {
        Some(e) => text(e.to_owned()).key("folder-error").font(type_::META).color(t.danger).truncate(),
        None => row()
            .gap(6.0)
            .min_w(0.0)
            .center_y()
            .child(text(v.folder_label(home)).key("folder-path").font(type_::MONO).color(t.text_2).truncate().min_w(0.0))
            .children((!custom).then(|| text("Default").key("folder-default").font(type_::META).color(t.text_3))),
    };
    let folder_buttons = row()
        .gap(space::S2)
        .children(custom.then(|| button(p, "folder-reset", "Reset", "Use the Downloads folder", |s: &mut Pane, _: &mut Tx| {
            s.change(SettingsChange::DownloadFolder(None))
        })))
        .child(button(p, "folder-choose", "Choose…", "Choose the download folder", choose_folder));
    let transfers = section(
        p,
        "Transfers",
        vec![
            setting(p, "Download folder", Some(folder_sub), folder_buttons),
            setting(p, "Transfers at once", Some(sub(p, "Files copied at the same time per server")), stepper(p, s.transfers_at_once)),
        ],
    );

    let hidden_sub = format!("For servers opened from now on; {} switches them", super::widgets::chord("mod+shift+period"));
    let browsing =
        section(p, "Browsing", vec![setting(p, "Show hidden files by default", Some(sub(p, &hidden_sub)), switch(p, s.show_hidden))]);

    col()
        .key("settings")
        .role("group")
        .label("Settings")
        .px(space::S6)
        .py(20.0)
        .gap(20.0)
        .fill(t.surface)
        .child(appearance)
        .child(transfers)
        .child(browsing)
}

/// "Choose…": the platform's folder picker, starting in the folder in use.
fn choose_folder(s: &mut Pane, tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Choose the download folder".into(),
        directory: Some(s.view.folder().to_path_buf()),
        ..FileDialogOptions::default()
    };
    tx.open_folder(options, |s: &mut Pane, picked, _| match picked {
        Ok(Some(folder)) => s.change(SettingsChange::DownloadFolder(Some(folder))),
        Ok(None) => {}
        Err(e) => s.error = Some(format!("The folder picker isn't available: {e}")),
    });
}

/// A caption and its rows.
fn section(p: Paint, title: &'static str, rows: Vec<El>) -> El {
    let caption = text(title.to_uppercase()).font(type_::CAPTION).color(p.t.text_3);
    col().key(format!("section-{}", title.to_lowercase())).role("group").label(title).w_full().gap(space::S2).child(caption).children(rows)
}

/// A label (with a line under it, maybe) on the left, its control on the right.
fn setting(p: Paint, label: &'static str, sub: Option<El>, control: El) -> El {
    let words = col().grow(1.0).min_w(0.0).gap(2.0).child(text(label).font(type_::BODY).color(p.t.text)).children(sub);
    row().w_full().min_h(32.0).gap(space::S4).center_y().child(words).child(control)
}

fn sub(p: Paint, line: &str) -> El {
    text(line.to_owned()).font(type_::META).color(p.t.text_2).truncate()
}

/// A segmented control of `options` (value, label, key), `current` selected; each segment is
/// `width` wide. Arrow keys move between the segments (a focus group); Space picks one.
fn segmented<T: Copy + PartialEq + 'static>(
    p: Paint,
    key: &'static str,
    label: &'static str,
    width: f32,
    options: &[(T, &'static str, &'static str)],
    current: T,
    change: fn(T) -> SettingsChange,
) -> El {
    let t = p.t;
    let segments = options.iter().map(|&(value, name, key)| {
        let on = value == current;
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
            .on_press_tx(move |s: &mut Pane, _: &mut Tx| s.change(change(value)))
            .child(text(name).font(if on { type_::BODY_STRONG } else { type_::BODY }).color(if on { t.text } else { t.text_2 }));
        if on { e.fill(t.press).border(p.hair, t.border_subtle) } else { e.hover(t.hover).press(t.press) }
    });
    row().key(key).role("radiogroup").label(label).focus_group().h(28.0).padding(2.0).gap(2.0).radius(radius::ROW + 1.0).fill(t.field).children(segments)
}

/// A bordered button with a body-size label, as the sheets' secondary ones.
fn button(p: Paint, key: &'static str, label: &'static str, describe: &'static str, f: fn(&mut Pane, &mut Tx)) -> El {
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

/// − n +, within [`AT_ONCE`]; an end's button is disabled.
fn stepper(p: Paint, n: usize) -> El {
    let t = p.t;
    let step = |key: &'static str, label: &'static str, glyph: &'static str, to: Option<usize>| {
        let b = row()
            .key(key)
            .role("button")
            .label(label)
            .size(24.0, 24.0)
            .radius(radius::ROW - 1.0)
            .center()
            .child(text(glyph).font(type_::BODY_STRONG).color(if to.is_some() { t.text_2 } else { t.text_3.alpha(0.5) }));
        match to {
            Some(to) => b
                .focusable(true)
                .keyboard_focus_only()
                .hover(t.hover)
                .press(t.press)
                .motion(motion::SNAP)
                .on_press_tx(move |s: &mut Pane, _: &mut Tx| s.change(SettingsChange::TransfersAtOnce(to))),
            None => b.disabled(true),
        }
    };
    let fewer = (n > *AT_ONCE.start()).then(|| n - 1);
    let more = (n < *AT_ONCE.end()).then(|| n + 1);
    row()
        .key("transfers-at-once")
        .role("group")
        .label(format!("Transfers at once: {n}"))
        .h(28.0)
        .padding(2.0)
        .gap(2.0)
        .radius(radius::ROW + 1.0)
        .center_y()
        .fill(t.field)
        .child(step("transfers-fewer", "Fewer transfers at once", "−", fewer))
        .child(text(n.to_string()).key("transfers-value").font(type_::BODY_STRONG.features("'tnum' 1")).color(t.text).w(24.0).align(rune::Align::Center))
        .child(step("transfers-more", "More transfers at once", "+", more))
}

/// An on/off switch: accent track and the knob on the right when on.
fn switch(p: Paint, on: bool) -> El {
    let t = p.t;
    let knob = el().size(14.0, 14.0).radius(7.0).fill(if on { t.on_accent } else { t.text_2 }).offset(if on { 14.0 } else { 0.0 }, 0.0);
    row()
        .key("hidden-toggle")
        .role("switch")
        .label("Show hidden files by default")
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
        .on_press_tx(move |s: &mut Pane, _: &mut Tx| s.change(SettingsChange::ShowHidden(!on)))
        .child(knob)
}
