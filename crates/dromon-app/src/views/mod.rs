//! The single window (§6.2): sidebar | toolbar, content, status bar, or the Settings page in
//! place of the last three (`settings.rs`). Each part is a function of `&App` and a [`Paint`];
//! handlers turn input into `state::Action`s and run them with [`run`].

mod add_config;
mod conflict;
mod delete;
mod drag;
mod drop;
mod edit;
mod list;
mod menu;
mod palette;
mod quick;
mod quit;
mod settings;
mod sheets;
mod sidebar;
mod states;
mod status;
mod toast;
mod toolbar;
mod transfers;
mod widgets;

use rune::{ChromeInsets, Cx, El, Tx, col, row};

/// The native context menu for a menu's entries (tests read it).
#[cfg(test)]
pub use menu::popup as menu_popup;
/// The app's poll (the done flourish) and its hookup to the taskbar and notifications.
pub use transfers::{OsLink, connect_os, connect_window, poll};

use crate::state::{Action, App, Effect, FileAction};
use crate::theme::{self, Access, Tokens};

/// What every part needs to paint itself.
#[derive(Clone, Copy)]
pub struct Paint {
    pub t: &'static Tokens,
    /// One device pixel, for hairlines.
    pub hair: f32,
    /// The device scale, for picking each icon's 1× or 2× body (`IconPair::at`).
    pub scale: f32,
    pub insets: ChromeInsets,
    /// Reduced motion (`access.reduce_motion`): what moves fades instead, and the refresh
    /// highlight holds, then fades quickly (§6.6).
    pub reduced: bool,
    /// The OS display preferences in force (reduce motion, reduce transparency, increased
    /// contrast; [`access`]). `t` already follows them; Rune reduces every transition,
    /// sequence and loop itself; the list's refresh highlight reads `reduced`.
    #[allow(dead_code)]
    pub access: Access,
}

/// The display preferences for this run of the view: the system's (Rune reads reduce motion and
/// increased contrast from the OS and re-runs the view when they change) or'd with what the app
/// forces on (`UiState::access`, for scenes and tests). Rune has no reduce-transparency reading
/// yet, so that one comes from `UiState::access` only.
pub fn access(s: &App, cx: &Cx<App>) -> Access {
    let system = Access {
        reduce_motion: cx.system_reduced_motion(),
        reduce_transparency: false,
        increase_contrast: cx.system_increased_contrast(),
    };
    system.or(s.ui.access)
}

/// Key of the list's scroll viewport, and of the stack of rows in it.
pub const LIST: &str = "list";
pub const ROWS: &str = "rows";

/// Runs an action from a handler and carries out its effects.
pub fn run(s: &mut App, action: Action, tx: &mut Tx) {
    for fx in s.act(action) {
        effect(fx, tx);
    }
    animate_refresh(s, tx);
}

/// A refresh that changed rows (a fixture answers inside the handler) animates them (§4.5).
fn animate_refresh(s: &mut App, tx: &mut Tx) {
    if s.take_animate() {
        tx.animated();
    }
}

/// The window gained or lost focus (the root's `on_window_focus`): the service polls by it,
/// and gaining it polls the folder on screen at once. Transfers notify only while no main
/// window has it (`App::focus_moved`).
fn window_focus(s: &mut App, focused: bool, tx: &mut Tx) {
    s.window_focus(focused);
    s.focus_moved(focused);
    animate_refresh(s, tx);
}

/// The window went off screen or came back (`App::on_visibility`): nothing polls while it's
/// away.
pub fn window_visibility(s: &mut App, visible: bool, tx: &mut Tx) {
    s.window_visibility(visible);
    animate_refresh(s, tx);
}

pub fn effect(fx: Effect, tx: &mut Tx) {
    match fx {
        // Scoped to the list's viewport (`rows` sits inside it; a reveal scoped to `rows` would
        // scroll only viewports inside `rows`: none), naming the row by its path so a file
        // called like another keyed element can't be taken for it.
        Effect::Reveal(name) => tx.scroll_into_view_in(LIST, format!("{ROWS}/{name}")),
        Effect::ScrollTop => tx.scroll_to_y(LIST, 0.0),
        Effect::Blur => tx.blur(),
        // Never from tests or headless runs (`--snap`, `--script`): no file manager there.
        Effect::ShowInFolder(path) => {
            if !cfg!(test) && rune::tooling::headless_run().is_none() {
                crate::reveal::show_in_folder(path);
            }
        }
        Effect::CloseWindow => tx.close_window(),
        Effect::Copy(text) => tx.copy(text),
        Effect::ContextMenu(kind) => menu::open_at_row(kind, tx),
        Effect::ReadClipboard { as_move } => {
            tx.read_clipboard(move |s: &mut App, content: &rune::ClipboardContent, tx: &mut Tx| pasted(s, content, as_move, tx))
        }
    }
}

/// What the OS clipboard held for ⌘V (or ⌥⌘V, `as_move`, and the menus' Paste): files copied in
/// Finder or Explorer upload to the folder on screen (not for Move here); otherwise its text
/// pastes the in-app clip, if it's still the clip's.
fn pasted(s: &mut App, content: &rune::ClipboardContent, as_move: bool, tx: &mut Tx) {
    // The folder view waits under the Settings page: nothing pastes into it.
    if s.on_settings() {
        return;
    }
    if !content.files().is_empty() {
        if !as_move {
            run(s, Action::Upload { paths: content.files().to_vec(), folder: None }, tx);
        }
        return;
    }
    run(s, Action::File(FileAction::Paste { text: content.text().to_owned(), as_move }), tx);
}

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let dark = s.ui.appearance.is_dark(cx.system_dark());
    let access = access(s, cx);
    // Rune reduces every transition, spring, sequence and loop of a commit that reduces motion
    // (§6.6: travel lands at once, opacity cross-fades); a forced preference has to say so.
    if access.reduce_motion {
        cx.reduced_motion(true);
    }
    let t = theme::tokens_for(dark, access);
    cx.caption_color(t.text_2);
    cx.foreground(t.text);
    // §6.3: keyboard focus is a 2 pt ring in the accent, just outside the element.
    cx.focus_ring(2.0, 1.0, t.focus);
    let scale = cx.env().scale;
    let p = Paint { t, hair: 1.0 / scale, scale, insets: cx.chrome_insets(), reduced: access.reduce_motion, access };
    // Under the Settings page the folder view stays mounted as it was (its scroll position and
    // fields kept for when the page goes), drawn over, out of reach and out of readers' tree.
    let section = s.ui.page.settings();
    let files = |e: El| if section.is_some() { e.inert().a11y_hidden() } else { e };
    let main = col()
        .key("main")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(files(toolbar::view(s, p)))
        .children(states::banner(s, p).map(files))
        .child(files(content(s, p)))
        .child(files(status::view(s, p)))
        .children(toast::layer(s, p).map(files))
        .children(section.map(|section| settings::page(s, section, p)));
    row()
        .key("window")
        .w_full()
        .h_full()
        .fill(t.canvas)
        // ⌘V: files copied in Finder or Explorer upload to the folder on screen; text that's
        // still the clip's pastes the clip (a focused field takes its own pastes).
        .on_paste_content(|s: &mut App, content: &rune::ClipboardContent, tx: &mut Tx| pasted(s, content, false, tx))
        .on_window_focus(window_focus)
        .child(sidebar::view(s, p))
        .child(main)
        .children(transfers::popover(s, p))
        .children(quick::popover(s, p))
        .children(palette::layer(s, p, palette::top(s, cx)))
        .children(sheets::layer(s, p))
        .children(menu::layer(s, p))
}

/// The list area: whichever state the active connection is in.
fn content(s: &App, p: Paint) -> El {
    let body = match s.conn() {
        None => states::welcome(s, p),
        Some(c) => match &c.status {
            dromon_core::ConnStatus::Failed(msg) => states::failed(s, c, msg, p),
            dromon_core::ConnStatus::Disconnected => states::disconnected(s, c, p),
            _ => drop::area(s, c, list::view(s, c, p), p),
        },
    };
    col().key("content").w_full().grow(1.0).min_h(0.0).fill(p.t.canvas).child(body)
}
