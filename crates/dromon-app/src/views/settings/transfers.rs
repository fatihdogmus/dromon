//! The Settings page's Transfers section: the download folder in mono with Choose… (the
//! platform's folder picker) and Reset, and Transfers at once, a 1–8 stepper.

use std::path::Path;

use rune::{El, FileDialogOptions, Tx, row, text};

use super::{button, change, section, setting, sub};
use crate::state::settings::AT_ONCE;
use crate::state::{App, SettingsChange, SettingsView};
use crate::theme::{motion, radius, space, type_};
use crate::views::Paint;

pub(super) fn transfers(v: &SettingsView, home: Option<&Path>, error: Option<&str>, p: Paint) -> El {
    let t = p.t;
    let s = &v.settings;
    let custom = s.download_folder.is_some();
    let folder_sub = match error {
        Some(e) => text(e.to_owned()).key("folder-error").font(type_::META).color(t.danger).truncate(),
        None => row()
            .gap(6.0)
            .min_w(0.0)
            .center_y()
            .child(
                text(v.folder_label(home)).key("folder-path").font(type_::MONO).color(t.text_2).truncate().min_w(0.0),
            )
            .children((!custom).then(|| text("Default").key("folder-default").font(type_::META).color(t.text_3))),
    };
    let folder_buttons = row()
        .gap(space::S2)
        .children(custom.then(|| {
            button(p, "folder-reset", "Reset", "Use the Downloads folder", |s: &mut App, tx: &mut Tx| {
                change(s, SettingsChange::DownloadFolder(None), tx)
            })
        }))
        .child(button(p, "folder-choose", "Choose…", "Choose the download folder", choose_folder));
    section(
        p,
        "Transfers",
        vec![
            setting(p, "Download folder", Some(folder_sub), folder_buttons),
            setting(
                p,
                "Transfers at once",
                Some(sub(p, "Files copied at the same time per server")),
                stepper(p, s.transfers_at_once),
            ),
        ],
    )
}

/// "Choose…": the platform's folder picker, starting in the folder in use. It runs off the UI
/// thread (Rune's); its answer comes back to this window's state.
fn choose_folder(s: &mut App, tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Choose the download folder".into(),
        directory: Some(s.settings_view().folder().to_path_buf()),
        ..FileDialogOptions::default()
    };
    tx.open_folder(options, |s: &mut App, picked, tx| match picked {
        Ok(Some(folder)) => change(s, SettingsChange::DownloadFolder(Some(folder)), tx),
        Ok(None) => {}
        Err(e) => s.ui.settings_page.error = Some(format!("The folder picker isn't available: {e}")),
    });
}

/// − n +, within [`AT_ONCE`]; an end's button is disabled.
fn stepper(p: Paint, n: usize) -> El {
    let t = p.t;
    let step = |key: &'static str, label: &'static str, glyph: &'static str, to: Option<usize>| {
        let b = row().key(key).role("button").label(label).size(24.0, 24.0).radius(radius::ROW - 1.0).center().child(
            text(glyph).font(type_::BODY_STRONG).color(if to.is_some() { t.text_2 } else { t.text_3.alpha(0.5) }),
        );
        match to {
            Some(to) => b
                .focusable(true)
                .keyboard_focus_only()
                .hover(t.hover)
                .press(t.press)
                .motion(motion::SNAP)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| change(s, SettingsChange::TransfersAtOnce(to), tx)),
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
        .child(
            text(n.to_string())
                .key("transfers-value")
                .font(type_::BODY_STRONG.features("'tnum' 1"))
                .color(t.text)
                .w(24.0)
                .align(rune::Align::Center),
        )
        .child(step("transfers-more", "More transfers at once", "+", more))
}
