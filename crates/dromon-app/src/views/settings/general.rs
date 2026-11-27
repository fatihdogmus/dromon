//! The Settings page's General section: the Terminal "Open in Terminal" uses, one choice per
//! line; Check for updates automatically, a switch; Check now, with what the last check found;
//! Live updates when the server supports them, a switch.

use rune::{El, Orientation, Tx, col, row, text};

use super::{button, section, segment, setting, sub, switch};
use crate::state::menu::TERMINAL_CHORD;
use crate::state::settings::Terminal;
use crate::state::update::{UpdateAction, Version};
use crate::state::{Action, App, SettingsChange, SettingsView};
use crate::theme::{radius, space, type_};
use crate::views::widgets::chord;
use crate::views::{Paint, run};

/// The terminal "Open in Terminal" uses; then updates: checking by itself, and Check now with
/// what the last check found, the button that takes a newer version (Update, Copy command) and
/// its release notes (`state/update.rs`).
pub(super) fn general(s: &App, v: &SettingsView, terminals: &[Terminal], p: Paint) -> El {
    let t = p.t;
    let terminal_sub = format!("Where Open in Terminal ({}) opens ssh", chord(TERMINAL_CHORD));
    let terminal = terminal_list(p, terminals, v.settings.terminal);
    let live_label = "Live updates when the server supports them";
    let live = switch(p, "live-toggle", live_label, v.settings.live_updates, SettingsChange::LiveUpdates);
    let live_sub = sub(p, "On Linux servers with inotifywait, changes show at once");
    let u = &s.update;
    let auto = switch(
        p,
        "updates-toggle",
        "Check for updates automatically",
        v.settings.check_updates,
        SettingsChange::CheckUpdates,
    );
    let status = text(u.status(&Version::running())).key("update-status").font(type_::META).color(t.text_2).wrap();
    let buttons = row()
        .gap(space::S2)
        .children(u.buttons().into_iter().map(|b| {
            button(p, b.key("settings-update"), b.label(), b.label(), move |s: &mut App, tx: &mut Tx| {
                run(s, Action::Update(b.action()), tx)
            })
        }))
        .child(button(p, "update-check", "Check now", "Check for updates now", |s: &mut App, tx: &mut Tx| {
            run(s, Action::Update(UpdateAction::CheckNow), tx)
        }));
    section(
        p,
        "General",
        vec![
            setting(p, "Terminal", Some(sub(p, &terminal_sub)), terminal),
            setting(
                p,
                "Check for updates automatically",
                Some(sub(p, "At launch and once a day: one request to GitHub, nothing about you")),
                auto,
            ),
            setting(p, "Updates", Some(status), buttons),
            setting(p, live_label, Some(live_sub), live),
        ],
    )
}

/// The terminal choices, one per line: a radio group ↑ ↓ move in, as the segmented controls'.
fn terminal_list(p: Paint, terminals: &[Terminal], current: Terminal) -> El {
    let options =
        terminals.iter().map(|&t| segment(p, t, t.label(), t.key(), t == current, 168.0, SettingsChange::Terminal));
    col()
        .key("terminal")
        .role("radiogroup")
        .label("Terminal")
        .orientation(Orientation::Vertical)
        .focus_group()
        .padding(2.0)
        .gap(2.0)
        .radius(radius::ROW + 1.0)
        .fill(p.t.field)
        .children(options)
}
