//! The host sidebar (§6.2): Connected, SSH Config, Saved and Recent, with a quick-connect
//! button at the bottom. Its right edge drags to resize it between 200 and 320 pt.

use dromon_core::{ConnStatus, HostSpec};
use rune::{Color, Cursor, Drag, El, Phase, ScrollSpec, Tx, col, el, icon, row, text};

use super::Paint;
use super::widgets::dot;
use crate::icons;
use crate::state::{Action, App, Section, hosts};
use crate::theme::{motion, radius, size, space, type_};

pub fn view(s: &App, p: Paint) -> El {
    let t = p.t;
    let mut sections = col().key("sb-sections").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec::default()).px(space::S2).pb(space::S2);
    for (section, hosts) in s.hosts.sections(&s.conns) {
        sections = sections.child(section_view(s, p, section, &hosts));
    }
    col()
        .key("sidebar")
        .role("navigation")
        .label("Hosts")
        .w(s.ui.sidebar_width)
        .h_full()
        .fill(t.sidebar)
        .border_sides([0.0, p.hair, 0.0, 0.0], t.border_subtle)
        .child(band(p))
        .child(sections)
        .child(footer(p))
        .child(resize_grip())
}

/// The top band, one piece with the toolbar: it drags the window and, on macOS, holds the
/// traffic lights. Elsewhere it carries the app's name.
fn band(p: Paint) -> El {
    let b = row().key("sb-band").w_full().h(size::TOOLBAR).center_y().window_drag();
    if p.insets.leading > 0.0 {
        return b;
    }
    b.px(space::S4).child(text("Dromon").font(type_::BODY_STRONG).color(p.t.text_2))
}

fn section_view(s: &App, p: Paint, section: Section, hosts: &[&HostSpec]) -> El {
    let label = row()
        .h(size::ROW)
        .px(space::S2)
        .pt(space::S1)
        .center_y()
        .child(text(section.title().to_uppercase()).font(type_::CAPTION).color(p.t.text_3));
    col().key(section.key()).w_full().pt(space::S2).child(label).children(hosts.iter().map(|h| host_row(s, p, h)))
}

fn host_row(s: &App, p: Paint, h: &HostSpec) -> El {
    let t = p.t;
    let active = s.active.as_ref() == Some(&h.id);
    let status = s.conns.get(&h.id).map(|c| &c.status);
    let dot_color: Option<Color> = match status {
        Some(ConnStatus::Ready) => Some(t.accent),
        Some(ConnStatus::Connecting | ConnStatus::Reconnecting { .. } | ConnStatus::NeedsInput) => Some(t.warning),
        _ => None,
    };
    let id = h.id.clone();
    let name = text(h.display_name().to_owned())
        .font(if active { type_::BODY_STRONG } else { type_::BODY })
        .color(if active { t.text } else { t.text_2 })
        .truncate()
        .grow(1.0);
    let mut r = row()
        .key(format!("host-{}", h.id))
        .role("button")
        .label(h.display_name().to_owned())
        .description(hosts::address(h).unwrap_or_default())
        .w_full()
        .h(size::ROW)
        .px(space::S2)
        .gap(space::S2)
        .radius(radius::ROW)
        .center_y()
        .fill(if active { t.active } else { Color::TRANSPARENT })
        .hover(t.hover)
        .press(t.press)
        .motion(motion::SNAP)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| super::run(s, Action::SelectHost(id.clone()), tx))
        .child(icon(icons::SERVER.at(p.scale), size::ICON).color(if active { t.text_2 } else { t.text_3 }))
        .child(name);
    if let Some(c) = dot_color {
        r = r.child(dot(c, 6.0).mr(space::S1));
    }
    r
}

fn footer(p: Paint) -> El {
    let t = p.t;
    let button = row()
        .key("sb-quick-connect")
        .role("button")
        .label("Quick connect")
        .h(size::ROW)
        .px(space::S2)
        .gap(space::S2)
        .grow(1.0)
        .radius(radius::ROW)
        .center_y()
        .hover(t.hover)
        .press(t.press)
        .child(icon(icons::PLUS.at(p.scale), size::ICON).color(t.text_3))
        .on_press_tx(|s: &mut App, tx: &mut Tx| super::run(s, Action::QuickConnect, tx))
        .child(text("Quick connect").font(type_::BODY).color(t.text_2).grow(1.0));
    row().w_full().h(44.0).px(space::S2).center_y().border_sides([p.hair, 0.0, 0.0, 0.0], t.border_subtle).child(button)
}

/// The resize handle on the sidebar's right edge.
fn resize_grip() -> El {
    el().key("sb-grip")
        .pin(None, Some(0.0), Some(-3.0), Some(0.0))
        .w(6.0)
        .cursor(Cursor::ResizeColumn)
        .on_drag(|s: &mut App, d: &mut Drag, _tx: &mut Tx| {
            if d.phase == Phase::Down {
                d.data = s.ui.sidebar_width;
            }
            s.ui.sidebar_width = (d.data + d.dx).clamp(size::SIDEBAR_MIN, size::SIDEBAR_MAX);
        })
}
