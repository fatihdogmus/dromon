//! The host sidebar (§6.2): Connected, SSH Config, Saved and Recent, with a quick-connect
//! button and the Settings gear at the bottom. Its right edge drags to resize it between 200
//! and 320 pt. It stays while the Settings page is on screen; picking a host leaves the page.

use dromon_core::{ConnStatus, HostSpec};
use rune::{Color, Cursor, Drag, El, Phase, ScrollSpec, Tx, col, el, icon, row, text};

use super::brand::{self, Mark};
use super::widgets::{Keyboard, dot, icon_button};
use super::{Paint, menu};
use crate::icons;
use crate::state::{Action, App, MenuKind, RowTarget, Section, hosts};
use crate::theme::{motion, radius, size, space, type_};

/// The key of the + button, which the quick-connect popover hangs from.
pub const QUICK_CONNECT: &str = "sb-quick-connect";
/// The key of the gear beside it.
pub const SETTINGS: &str = "sidebar-settings";

pub fn view(s: &App, p: Paint) -> El {
    let t = p.t;
    // One list of every host (§6.9): Tab reaches the host on screen (or the first), ↑ ↓ move
    // between them across the sections, and Return or Space connects.
    let mut hosts = col().key("sb-hosts").role("listbox").label("Hosts").w_full().focus_group();
    for (section, list) in s.hosts.sections(&s.conns) {
        hosts = hosts.child(section_view(s, p, section, &list));
    }
    let sections = col()
        .key("sb-sections")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .px(space::S2)
        .pb(space::S2)
        .child(hosts);
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
        .child(footer(s, p))
        .child(resize_grip())
}

/// The top band, one piece with the toolbar: it drags the window and, on macOS, holds the
/// traffic lights. Elsewhere it carries the brand's simple mark and the app's name.
fn band(p: Paint) -> El {
    let b = row().key("sb-band").w_full().h(size::TOOLBAR).center_y().window_drag();
    if p.insets.leading > 0.0 {
        return b;
    }
    b.px(space::S4)
        .gap(space::S2)
        .child(brand::mark("sb-mark", Mark::Simple, p))
        .child(text("Dromon").font(type_::BODY_STRONG).color(p.t.text_2))
}

fn section_view(s: &App, p: Paint, section: Section, hosts: &[&HostSpec]) -> El {
    let label = row()
        .h(size::ROW)
        .px(space::S2)
        .pt(space::S1)
        .center_y()
        .a11y_hidden()
        .child(text(section.title().to_uppercase()).font(type_::CAPTION).color(p.t.text_3));
    col()
        .key(section.key())
        .role("group")
        .label(section.title())
        .w_full()
        .pt(space::S2)
        .child(label)
        .children(hosts.iter().map(|h| host_row(s, p, h)))
}

/// What the row's dot says, in words (never colour alone, §6.9).
fn status_words(status: Option<&ConnStatus>, opened: bool) -> Option<&'static str> {
    Some(match status? {
        ConnStatus::Ready => "Connected",
        ConnStatus::Connecting => "Connecting",
        ConnStatus::NeedsInput if !opened => "Signing in",
        ConnStatus::Reconnecting { .. } | ConnStatus::NeedsInput => "Reconnecting",
        ConnStatus::Failed(_) => "Couldn't connect",
        ConnStatus::Disconnected => return None,
    })
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
    let menu_id = h.id.clone();
    // Rows dragged here copy into this host's folder on screen.
    let target = RowTarget::Host(h.id.clone());
    let over = super::drag::over(s, &target);
    let name = text(h.display_name().to_owned())
        .font(if active { type_::BODY_STRONG } else { type_::BODY })
        .color(if active { t.text } else { t.text_2 })
        .truncate()
        .grow(1.0);
    let opened = s.conns.get(&h.id).is_some_and(|c| c.opened);
    let address = hosts::address(h).unwrap_or_default();
    let description = match status_words(status, opened) {
        Some(w) if address.is_empty() => w.to_owned(),
        Some(w) => format!("{w}, {address}"),
        None => address,
    };
    let mut r = row()
        .key(format!("host-{}", h.id))
        .role("option")
        .label(h.display_name().to_owned())
        .description(description)
        .tab_stop()
        // The group's Tab stop is the host on screen; the item draws its own ring, as list
        // items have none by default (their selection stands in, which isn't the focus here).
        .focus_selected(active)
        .focus_ring(2.0, -2.0, t.focus)
        .w_full()
        .h(size::ROW)
        .px(space::S2)
        .gap(space::S2)
        .radius(radius::ROW)
        .center_y()
        .data(target)
        .fill(if over {
            t.accent.alpha(0.22)
        } else if active {
            t.active
        } else {
            Color::TRANSPARENT
        })
        .hover(t.hover)
        .press(t.press)
        .motion(motion::SNAP)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| super::run(s, Action::SelectHost(id.clone()), tx))
        // Connect / Disconnect; Add to SSH config…, Edit… and Remove for saved hosts; Save and
        // Remove for recent ones (`state::hostbook`).
        .on_secondary_press(move |s: &mut App, at, tx: &mut Tx| menu::open(s, MenuKind::Host(menu_id.clone()), at, tx))
        .child(icon(icons::SERVER.at(p.scale), size::ICON).color(if active { t.text_2 } else { t.text_3 }))
        .child(name);
    if let Some(c) = dot_color {
        r = r.child(dot(c, 6.0).mr(space::S1));
    }
    if over {
        r = r.border(1.5, t.accent).shadow(2.0, 8.0, t.shadow);
    }
    r
}

/// Quick connect, and the gear that shows the Settings page (filled while it's on screen).
fn footer(s: &App, p: Paint) -> El {
    let t = p.t;
    fn open_settings(s: &mut App, tx: &mut Tx) {
        super::run(s, Action::OpenSettings, tx)
    }
    let mut settings = icon_button(p, SETTINGS, "Settings", &icons::GEAR, Some(open_settings));
    if s.on_settings() {
        settings = settings.fill(t.active);
    }
    let button = row()
        .key(QUICK_CONNECT)
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
        .tab_press()
        .child(text("Quick connect").font(type_::BODY).color(t.text_2).grow(1.0));
    row()
        .w_full()
        .h(44.0)
        .px(space::S2)
        .gap(space::S1)
        .center_y()
        .border_sides([p.hair, 0.0, 0.0, 0.0], t.border_subtle)
        .child(button)
        .child(settings)
}

/// The resize handle on the sidebar's right edge.
fn resize_grip() -> El {
    el().key("sb-grip").pin(None, Some(0.0), Some(-3.0), Some(0.0)).w(6.0).cursor(Cursor::ResizeColumn).on_drag(
        |s: &mut App, d: &mut Drag, _tx: &mut Tx| {
            if d.phase == Phase::Down {
                d.data = s.ui.sidebar_width;
            }
            s.ui.sidebar_width = (d.data + d.dx).clamp(size::SIDEBAR_MIN, size::SIDEBAR_MAX);
            if d.phase == Phase::Up {
                s.sidebar_resized();
            }
        },
    )
}
