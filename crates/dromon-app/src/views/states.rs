//! The list area's other states (§6.8): welcome, connecting skeleton, empty folder, permission
//! denied, no filter matches, and the reconnecting banner.

use dromon_core::ConnStatus;
use rune::{El, LoopSpec, Transition, Tx, col, el, icon, row, text};

use super::widgets::{chord, dot};
use super::{Paint, run};
use crate::icons::{self, IconPair};
use crate::state::{Action, App, ConnView};
use crate::theme::{motion, radius, size, space, type_};

/// A centred headline, sub-text and optional extras.
fn centred(key: &'static str, p: Paint, headline: &str, sub: Option<El>) -> El {
    col()
        .key(key)
        .w_full()
        .h_full()
        .center()
        .gap(space::S2)
        .pb(size::TOOLBAR)
        .transition(Transition::opacity())
        .transition_motion(motion::MOVE, motion::MOVE)
        .child(text(headline.to_owned()).font(type_::DISPLAY).color(p.t.text))
        .children(sub)
}

fn button(p: Paint, key: &'static str, glyph: &'static IconPair, label: &'static str, action: Action) -> El {
    let t = p.t;
    row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .pl(space::S2)
        .pr(space::S3)
        .gap(6.0)
        .mt(space::S3)
        .radius(radius::ROW)
        .center_y()
        .fill(t.field)
        .border(p.hair, t.border)
        .hover(t.hover)
        .press(t.press)
        .press_scale(0.97)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, action.clone(), tx))
        .child(icon(glyph.at(p.scale), size::ICON).color(t.text_2))
        .child(text(label).font(type_::BODY_STRONG).color(t.text))
}

pub fn welcome(p: Paint) -> El {
    let sub = col()
        .center()
        .gap(space::S2)
        .child(text(format!("or press {}", chord("mod+k"))).font(type_::BODY).color(p.t.text_2))
        .child(button(p, "welcome-quick-connect", &icons::PLUS, "Quick connect", Action::QuickConnect));
    centred("welcome", p, "Pick a host to start", Some(sub))
}

pub fn empty(p: Paint) -> El {
    centred("empty", p, "Empty folder", Some(text("Drop files here to upload").font(type_::BODY).color(p.t.text_2)))
}

pub fn no_matches(p: Paint, query: &str) -> El {
    let sub = text("Nothing in this folder matches").font(type_::BODY).color(p.t.text_2);
    centred("no-matches", p, &format!("No matches for “{query}”"), Some(sub))
}

pub fn message(p: Paint, headline: &str, detail: &str, go_up: bool) -> El {
    let sub = col()
        .center()
        .gap(space::S2)
        .child(text(detail.to_owned()).font(type_::MONO).color(p.t.text_2))
        .children(go_up.then(|| button(p, "state-go-up", &icons::ARROW_UP, "Go up", Action::GoUp)));
    centred("message", p, headline, Some(sub))
}

pub fn denied(p: Paint, c: &ConnView) -> El {
    let t = p.t;
    let glyph = el()
        .size(48.0, 48.0)
        .radius(radius::SHEET)
        .center()
        .mb(space::S2)
        .fill(t.field)
        .border(p.hair, t.border_subtle)
        .child(icon(icons::LOCK.at(p.scale), 32.0).color(t.text_2));
    let sub = col()
        .center()
        .gap(space::S2)
        .child(text(c.cwd.as_str().to_owned()).font(type_::MONO).color(t.text_3))
        .children(c.cwd.parent().map(|_| button(p, "state-go-up", &icons::ARROW_UP, "Go up", Action::GoUp)));
    col()
        .key("denied")
        .w_full()
        .h_full()
        .center()
        .gap(space::S2)
        .pb(size::TOOLBAR)
        .child(glyph)
        .child(text("You don't have access to this folder").font(type_::DISPLAY).color(t.text))
        .child(sub)
}

/// The connection failed (§3.4): why, in ssh's words or ours, with Try again and Show log.
pub fn failed(s: &App, c: &ConnView, msg: &str, p: Paint) -> El {
    let host = s.hosts.get(&c.host).map_or(c.host.as_str(), |h| h.display_name()).to_owned();
    let actions = row()
        .gap(space::S2)
        .child(button(p, "state-retry", &icons::SERVER, "Try again", Action::SelectHost(c.host.clone())))
        .child(button(p, "state-show-log", &icons::LOG, "Show log", Action::OpenLog));
    let sub = col()
        .center()
        .gap(space::S2)
        .max_w(520.0)
        .child(text(msg.to_owned()).key("state-reason").font(type_::BODY).color(p.t.text_2).wrap())
        .child(actions);
    centred("failed", p, &format!("Couldn't connect to {host}"), Some(sub))
}

/// The user disconnected: offer to connect again.
pub fn disconnected(s: &App, c: &ConnView, p: Paint) -> El {
    let host = s.hosts.get(&c.host).map_or(c.host.as_str(), |h| h.display_name()).to_owned();
    let sub = col()
        .center()
        .gap(space::S2)
        .child(text("Passwords for it were forgotten.").font(type_::BODY).color(p.t.text_2))
        .child(button(p, "state-connect", &icons::SERVER, "Connect", Action::SelectHost(c.host.clone())));
    centred("disconnected", p, &format!("Disconnected from {host}"), Some(sub))
}

/// Twelve rows of placeholder bars, pulsing gently in opacity (no shimmer sweep).
pub fn skeleton(p: Paint) -> El {
    const NAMES: [f32; 12] = [128.0, 184.0, 96.0, 152.0, 220.0, 112.0, 168.0, 140.0, 204.0, 88.0, 176.0, 120.0];
    let bar = |w: f32| el().w(w).h(8.0).radius(4.0).fill(p.t.skeleton);
    let rows = NAMES.iter().enumerate().map(|(i, &w)| {
        row()
            .w_full()
            .h(size::ROW)
            .px(space::S1)
            .child(
                row()
                    .w_full()
                    .h_full()
                    .pl(space::S2)
                    .pr(space::S3)
                    .gap(space::S2)
                    .center_y()
                    .child(el().size(size::ICON, size::ICON).radius(4.0).fill(p.t.skeleton))
                    .child(row().grow(1.0).child(bar(w)))
                    .child(row().w(size::COL_SIZE).justify_end().child(bar(if i % 3 == 0 { 24.0 } else { 44.0 })))
                    .child(row().w(size::COL_MODIFIED).pl(space::S4).child(bar(64.0 + (i % 4) as f32 * 6.0))),
            )
    });
    col()
        .key("skeleton")
        .role("progressbar")
        .label("Loading")
        .busy(true)
        .w_full()
        .py(space::S1)
        .looping(LoopSpec::opacity(1.6, 0.0, 0.45, 1.0))
        .children(rows)
}

/// The slim banner under the toolbar while a connection is lost (§3.9).
pub fn banner(s: &App, p: Paint) -> Option<El> {
    let t = p.t;
    let c = s.conn()?;
    let host = s.hosts.get(&c.host).map_or(c.host.as_str(), |h| h.display_name()).to_owned();
    let (headline, detail, sign_in) = match c.status {
        ConnStatus::Reconnecting { attempt } => (format!("Reconnecting to {host}…"), format!("Attempt {attempt}"), false),
        // Signing in the first time is the sheet's business; the banner is for reconnects.
        ConnStatus::NeedsInput if c.opened => (format!("Reconnecting to {host} needs you to sign in"), String::new(), true),
        _ => return None,
    };
    let mut b = row()
        .key("banner")
        .role("status")
        .label(headline.clone())
        .w_full()
        .h(size::BANNER)
        .px(space::S4)
        .gap(space::S2)
        .center_y()
        .fill(t.banner)
        .border_sides([0.0, 0.0, p.hair, 0.0], t.warning.alpha(0.18))
        .transition(Transition::opacity())
        .transition_motion(motion::MOVE, motion::MOVE)
        .child(dot(t.warning, 6.0))
        .child(text(headline).font(type_::BODY_STRONG).color(t.text))
        .child(text(detail).font(type_::META).color(t.text_3).grow(1.0));
    if sign_in {
        b = b.child(
            row().key("banner-sign-in").role("button").h(22.0).px(space::S2).radius(radius::ROW).center_y().border(p.hair, t.border).hover(t.hover).child(text("Sign in").font(type_::META_STRONG).color(t.text)),
        );
    } else {
        b = b.child(text("Changes are paused until it's back").font(type_::META).color(t.text_3));
    }
    Some(b)
}
