//! The status bar (§6.2): counts and selection on the left, the connection on the right, in
//! tabular figures so nothing jiggles as numbers change.

use dromon_core::ConnStatus;
use rune::{Color, El, Tx, row, text};

use super::widgets::{Keyboard, dot};
use super::{Paint, run};
use crate::fmt;
use crate::state::{Action, App, ConnView, Listing};
use crate::theme::{size, space, type_};

/// "42 items · 3 selected, 18.2 MB", or what the folder is doing instead.
pub fn summary(c: &ConnView) -> String {
    match &c.listing {
        Listing::Loading => "Loading…".into(),
        Listing::Failed(_) => String::new(),
        Listing::Ready(_) => {
            let total = c.total();
            let mut s = if c.filter.is_on() && c.row_count() != total {
                format!("{} of {}", fmt::thousands(c.row_count() as u64), fmt::items(total))
            } else {
                fmt::items(total)
            };
            if c.loading_more() {
                s.push('…');
            }
            if !c.selection.is_empty() {
                let n = c.selection.len();
                s.push_str(&format!(" · {} selected", fmt::thousands(n as u64)));
                let bytes = c.selected_bytes();
                if bytes > 0 {
                    s.push_str(&format!(", {}", fmt::size(bytes)));
                }
            }
            s
        }
    }
}

pub fn view(s: &App, p: Paint) -> El {
    let t = p.t;
    let bar = row()
        .key("status")
        .role("status")
        .w_full()
        .h(size::STATUS)
        .px(space::S4)
        .gap(space::S2)
        .center_y()
        .fill(t.surface)
        .border_sides([p.hair, 0.0, 0.0, 0.0], t.border_subtle);
    let Some(c) = s.conn() else {
        return bar.child(text("Not connected").font(type_::META).color(t.text_3));
    };
    let (label, color): (&str, Color) = match &c.status {
        ConnStatus::Ready => ("Connected", t.accent),
        ConnStatus::Connecting => ("Connecting…", t.text_3),
        ConnStatus::NeedsInput if !c.opened => ("Signing in…", t.text_3),
        ConnStatus::Reconnecting { .. } | ConnStatus::NeedsInput => ("Reconnecting…", t.warning),
        ConnStatus::Failed(_) => ("Couldn't connect", t.danger),
        ConnStatus::Disconnected => ("Disconnected", t.text_3),
    };
    // The connection's status opens its log.
    let conn = row()
        .key("status-conn")
        .role("button")
        .label(format!("{label} · Show connection log"))
        .gap(6.0)
        .center_y()
        .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::OpenLog, tx))
        .has_popup(rune::HasPopup::Dialog)
        .tab_press()
        .radius(4.0)
        .child(dot(color, 6.0))
        .child(text(label).font(type_::META).color(t.text_2));
    // A search's results count themselves in place of the folder (`search.rs`).
    let summary = s.results().map_or_else(|| summary(c), super::search::summary);
    bar.child(text(summary).key("status-summary").font(type_::META).color(t.text_2).grow(1.0).truncate()).child(conn)
}
