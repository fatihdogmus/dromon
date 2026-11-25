//! The status bar (§6.2): counts and selection on the left, the connection on the right, in
//! tabular figures so nothing jiggles as numbers change.

use dromon_core::ConnStatus;
use rune::{Color, El, row, text};

use super::Paint;
use super::widgets::dot;
use crate::fmt;
use crate::state::{App, ConnView, Listing};
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
        ConnStatus::Reconnecting { .. } | ConnStatus::NeedsInput => ("Reconnecting…", t.warning),
        ConnStatus::Failed(_) => ("Couldn't connect", t.danger),
        ConnStatus::Disconnected => ("Disconnected", t.text_3),
    };
    bar.child(text(summary(c)).key("status-summary").font(type_::META).color(t.text_2).grow(1.0).truncate())
        .child(row().key("status-conn").gap(6.0).center_y().child(dot(color, 6.0)).child(text(label).font(type_::META).color(t.text_2)))
}
