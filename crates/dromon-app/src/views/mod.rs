//! The single window (§6.2): sidebar | toolbar, content, status bar. Each part is a function of
//! `&App` and a [`Paint`]; handlers turn input into `state::Action`s and run them with [`run`].

mod list;
mod sidebar;
mod states;
mod status;
mod toolbar;
mod widgets;

use rune::{ChromeInsets, Cx, El, Tx, col, row};

use crate::state::{Action, App, Effect};
use crate::theme::{self, Tokens};

/// What every part needs to paint itself.
#[derive(Clone, Copy)]
pub struct Paint {
    pub t: &'static Tokens,
    /// One device pixel, for hairlines.
    pub hair: f32,
    pub insets: ChromeInsets,
}

/// Key of the list's scroll viewport, and of the stack of rows in it.
pub const LIST: &str = "list";
pub const ROWS: &str = "rows";

/// Runs an action from a handler and carries out its effects.
pub fn run(s: &mut App, action: Action, tx: &mut Tx) {
    for fx in s.act(action) {
        effect(fx, tx);
    }
}

pub fn effect(fx: Effect, tx: &mut Tx) {
    match fx {
        Effect::Reveal(name) => tx.scroll_into_view_in(ROWS, name),
        Effect::ScrollTop => tx.scroll_to_y(LIST, 0.0),
        Effect::Blur => tx.blur(),
    }
}

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let dark = s.ui.appearance.is_dark(cx.system_dark());
    let t = theme::tokens(dark);
    cx.caption_color(t.text_2);
    cx.foreground(t.text);
    let p = Paint { t, hair: 1.0 / cx.env().scale, insets: cx.chrome_insets() };
    let main = col()
        .key("main")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(toolbar::view(s, p))
        .children(states::banner(s, p))
        .child(content(s, p))
        .child(status::view(s, p));
    row().key("window").w_full().h_full().fill(t.canvas).child(sidebar::view(s, p)).child(main)
}

/// The list area: whichever state the active connection is in.
fn content(s: &App, p: Paint) -> El {
    let body = match s.conn() {
        None => states::welcome(p),
        Some(c) => list::view(s, c, p),
    };
    col().key("content").w_full().grow(1.0).min_h(0.0).fill(p.t.canvas).child(body)
}
