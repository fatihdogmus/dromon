//! Context menus (`state::menu`): native on macOS and Windows through Rune's `Tx::popup_menu`,
//! drawn here elsewhere (Linux, and every headless run, so goldens and tests see the same menu
//! on every platform). The drawn menu opens at the pointer on the `raised` surface (10 pt
//! radius, popover elevation), flips to stay in the window, and closes on a pick, a press
//! outside or Escape.

use rune::{Align, El, PointerPress, PopupItem, PopupMenu, Side, Transition, Tx, col, el, icon, row, text};

use super::widgets::chord;
use super::{Paint, run};
use crate::icons;
use crate::state::menu::{MenuEntry, MenuKind};
use crate::state::{Action, App, FileAction};
use crate::theme::{motion, radius, size, space, type_};

const ANCHOR: &str = "ctx-point";
const MIN_WIDTH: f32 = 220.0;

/// Whether menus are the platform's own: macOS and Windows windows. Tests and headless runs
/// draw them, so what they show can be checked and snapped.
pub fn native() -> bool {
    rune::popup_menu_supported() && !cfg!(test) && rune::tooling::headless_run().is_none()
}

/// The native menu for `entries`.
pub fn popup(entries: &[MenuEntry]) -> PopupMenu {
    let mut menu = PopupMenu::new();
    for e in entries {
        menu = match e {
            MenuEntry::Separator => menu.separator(),
            MenuEntry::Item(i) => {
                let mut item = PopupItem::new(i.id, i.title).enabled(i.enabled);
                if let Some(on) = i.checked {
                    item = item.checked(on);
                }
                if let Some(sc) = i.shortcut.and_then(|s| s.parse::<rune::Shortcut>().ok()) {
                    item = item.key_equivalent(sc);
                }
                menu.item(item)
            }
        };
    }
    menu
}

/// A right-click asked for `kind`'s menu at `at` (window points).
pub fn open(s: &mut App, kind: MenuKind, at: PointerPress, tx: &mut Tx) {
    if !native() {
        run(s, Action::File(FileAction::OpenMenu { kind, at: Some((at.x, at.y)) }), tx);
        return;
    }
    run(s, Action::File(FileAction::OpenMenu { kind: kind.clone(), at: None }), tx);
    let menu = popup(&s.menu(&kind));
    tx.popup_menu(menu, at, move |s: &mut App, picked: Option<&str>, tx: &mut Tx| {
        if let Some(id) = picked {
            run(s, Action::File(FileAction::Pick { kind, id: id.to_owned() }), tx);
        }
    });
}

/// The keyboard asked for `kind`'s menu (Shift+F10, the Menu key): it opens below its row's
/// bottom-left corner, as a keyboard menu does; a row that isn't on screen, and the empty
/// area, use the list's top-left.
pub fn open_at_row(kind: MenuKind, tx: &mut Tx) {
    let at_list = |kind: MenuKind, tx: &mut Tx| {
        tx.read_layout(super::LIST, move |s: &mut App, rect, tx: &mut Tx| {
            let [x, y, ..] = rect.unwrap_or_default();
            open(s, kind, PointerPress { x, y }, tx);
        });
    };
    let MenuKind::Row(name) = &kind else { return at_list(kind, tx) };
    tx.read_layout(format!("{}/{name}", super::ROWS), move |s: &mut App, rect, tx: &mut Tx| match rect {
        Some([x, y, _, h]) => open(s, kind, PointerPress { x, y: y + h }, tx),
        None => at_list(kind, tx),
    });
}

/// The drawn menu, when one is open.
pub fn layer(s: &App, p: Paint) -> Option<El> {
    let open = s.files.menu.as_ref()?;
    let t = p.t;
    let entries = s.menu(&open.kind);
    let rows = entries.iter().enumerate().map(|(i, e)| entry(e, i, &open.kind, p));
    let panel = col()
        .key("ctx-menu")
        .role("menu")
        .label("Actions")
        .min_w(MIN_WIDTH)
        .padding(space::S1)
        .radius(radius::POPOVER)
        .fill(t.raised)
        .border(p.hair, t.elevated_edge())
        .shadow(8.0, 24.0, t.shadow)
        .anchored_fit(ANCHOR, Side::Bottom, Align::Start, 2.0)
        .origin(0.0, 0.0)
        .transition(Transition::scale(0.96).combined(Transition::opacity()))
        .transition_motion(motion::SNAP, motion::MOVE)
        .focus_group()
        .modal_tx(true, |s: &mut App, tx: &mut Tx| run(s, Action::File(FileAction::CloseMenu), tx))
        .children(rows);
    let (x, y) = open.at;
    Some(
        el().key("ctx-layer")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .child(el().key(ANCHOR).absolute(x, y).size(0.0, 0.0).a11y_hidden())
            .child(panel),
    )
}

fn entry(e: &MenuEntry, i: usize, kind: &MenuKind, p: Paint) -> El {
    let t = p.t;
    let MenuEntry::Item(item) = e else {
        return el().key(format!("ctx-sep-{i}")).h(p.hair).my(space::S1).mx(space::S1).fill(t.border).a11y_hidden();
    };
    let ink = if !item.enabled {
        t.text_3
    } else if item.danger {
        t.danger
    } else {
        t.text
    };
    let mark = el().size(size::ICON, size::ICON).center().children(
        item.checked.filter(|on| *on).map(|_| icon(icons::CHECK.at(p.scale), size::ICON).color(if item.enabled { t.text_2 } else { t.text_3 })),
    );
    let mut r = row()
        .key(format!("ctx-{}", item.id))
        .role(if item.checked.is_some() { "menuitemcheckbox" } else { "menuitem" })
        .label(item.title)
        .h(26.0)
        .pl(space::S1)
        .pr(space::S2)
        .gap(space::S2)
        .radius(radius::ROW)
        .center_y()
        .child(mark)
        .child(text(item.title).key("label").font(type_::BODY).color(ink).grow(1.0));
    if let Some(on) = item.checked {
        r = r.checked(on);
    }
    if let Some(spec) = item.shortcut {
        r = r.child(text(chord(spec)).key("shortcut").font(type_::META).color(t.text_3));
    }
    if !item.enabled {
        return r.disabled(true);
    }
    let (kind, id) = (kind.clone(), item.id.to_owned());
    // ↑ ↓ move the highlight (the menu is a focus group), Return or Space picks. The highlight
    // is the keyboard's focus too, so it's the selection's accent wash, not the 4% hover.
    r.focusable(true)
        .focus_highlight(t.selection)
        .hover(t.hover)
        .press(t.press)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::File(FileAction::Pick { kind: kind.clone(), id: id.clone() }), tx))
}
