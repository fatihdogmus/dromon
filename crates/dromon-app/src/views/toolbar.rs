//! The toolbar (§6.2), one piece with the title bar: back/forward, the path as breadcrumbs,
//! the ⌘F filter, the transfers button and the ⌘K hint. It drags the window wherever nothing
//! takes the press, and leaves room for Windows' caption buttons (`ChromeInsets::trailing`).

use dromon_core::{ConnStatus, RemotePath};
use rune::{El, HasPopup, Key, KeyEvent, KeyPlatform, LoopSpec, Tx, el, icon, row, text, text_input};

use super::widgets::{Keyboard, chord, icon_button, kbd};
use super::{Paint, run, transfers};
use crate::icons;
use crate::state::keys::{self, FileKey};
use crate::state::{Action, App, ConnView, FileAction, Filter, PaletteAction, RowTarget, Shortcut};
use crate::theme::{radius, size, space, type_};

pub fn view(s: &App, p: Paint) -> El {
    let t = p.t;
    let c = s.conn();
    let live = c.is_some_and(ConnView::interactive);
    let can_back = live && c.is_some_and(|c| c.history.can_back());
    let can_forward = live && c.is_some_and(|c| c.history.can_forward());
    fn go_back(s: &mut App, tx: &mut Tx) {
        run(s, Action::Back, tx)
    }
    fn go_forward(s: &mut App, tx: &mut Tx) {
        run(s, Action::Forward, tx)
    }
    type Handler = fn(&mut App, &mut Tx);
    let back = icon_button(p, "tb-back", "Back", &icons::CHEVRON_LEFT, can_back.then_some(go_back as Handler));
    let forward = icon_button(p, "tb-forward", "Forward", &icons::CHEVRON_RIGHT, can_forward.then_some(go_forward as Handler));
    fn pick_upload(s: &mut App, tx: &mut Tx) {
        run(s, Action::PickUpload, tx)
    }
    let upload = icon_button(p, "tb-upload", "Upload", &icons::UPLOAD, live.then_some(pick_upload as Handler));
    let mut bar = row()
        .key("toolbar")
        .role("toolbar")
        .w_full()
        .h(size::TOOLBAR)
        .pl(space::S2)
        .pr(space::S3 + p.insets.trailing)
        .gap(space::S1)
        .center_y()
        .fill(t.surface)
        .border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle)
        .window_drag()
        .child(back)
        .child(forward)
        .child(path_bar(s, p).ml(space::S2));
    if let Some(c) = c {
        bar = bar.child(filter(s, c, p));
    }
    bar.child(upload).child(transfers::button(s, p)).child(palette_button(p))
}

/// The ⌘K hint: a button that opens the command palette.
fn palette_button(p: Paint) -> El {
    let label = chord("mod+k");
    row()
        .key("tb-palette")
        .role("button")
        .label("Command palette")
        .shortcut_hint(label.clone())
        .h(28.0)
        // The chip stays where it was; the padding around it takes presses too.
        .px(space::S1)
        .mr(-space::S1)
        .radius(radius::ROW)
        .center()
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::Palette(PaletteAction::Toggle), tx))
        .has_popup(HasPopup::Dialog)
        .tab_press()
        // The chip's text repeats the shortcut the button already says.
        .child(kbd(p, &label).a11y_hidden())
}

/// Breadcrumbs for the folder on screen, or what's happening instead.
fn path_bar(s: &App, p: Paint) -> El {
    let t = p.t;
    // Clipped, so a path longer than the room fades out under the filter instead of over it.
    let bar = row().key("tb-path").role("navigation").label("Folder path").grow(1.0).min_w(0.0).h(28.0).center_y().clip_soft(0.0);
    let Some(c) = s.conn() else {
        return bar;
    };
    let host = s.hosts.get(&c.host).map_or(c.host.as_str(), |h| h.display_name()).to_owned();
    let signing_in = !c.opened && matches!(c.status, ConnStatus::NeedsInput);
    if c.status == ConnStatus::Connecting || signing_in {
        let spinner = icon(icons::SPINNER.at(p.scale), size::ICON).color(t.text_3).looping(LoopSpec::rotation(0.9));
        return bar
            .gap(space::S2)
            .px(space::S1)
            .child(spinner)
            .child(text(format!("Connecting to {host}…")).key("tb-connecting").font(type_::BODY).color(t.text_2).truncate());
    }
    let crumbs = c.cwd.ancestors_from_root();
    let last = crumbs.len() - 1;
    let mut bar = bar;
    for (i, path) in crumbs.into_iter().enumerate() {
        let label = if i == 0 { host.clone() } else { path.file_name().unwrap_or_default().to_owned() };
        if i > 0 {
            bar = bar.child(icon(icons::CRUMB.at(p.scale), size::ICON).color(t.text_3));
        }
        let over = super::drag::over(s, &RowTarget::Crumb(path.clone()));
        bar = bar.child(crumb(p, i, label, path, i == last, c.interactive(), over));
    }
    bar
}

/// One breadcrumb. It takes dragged rows (`over`: the drag is on it and it takes them).
fn crumb(p: Paint, i: usize, label: String, path: RemotePath, current: bool, live: bool, over: bool) -> El {
    let t = p.t;
    let mut e = row()
        .data(RowTarget::Crumb(path.clone()))
        .key(format!("crumb-{i}"))
        .label(label.clone())
        .h(24.0)
        .px(6.0)
        .radius(radius::ROW)
        .center_y()
        .min_w(0.0)
        .shrink()
        .child(
            text(label)
                .font(if current { type_::BODY_STRONG } else { type_::BODY })
                .color(if current { t.text } else { t.text_2 })
                .truncate(),
        );
    if over {
        e = e.fill(t.accent.alpha(0.22)).border(1.5, t.accent).shadow(2.0, 8.0, t.shadow);
    }
    // The folder on screen is the trail's end, not a link (it says so); the others are links,
    // dimmed for readers while the connection is down.
    if current {
        return e.description("Current folder");
    }
    e = e.role("link");
    if live {
        e = e.hover(t.hover).press(t.press).on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, Action::GoTo(path.clone()), tx)).tab_press();
    } else {
        e = e.disabled(true);
    }
    e
}

/// The ⌘F field: a quiet box that opens on click or ⌘F, then filters as you type. Escape
/// closes it; ↑ ↓ and Return still drive the list.
fn filter(s: &App, c: &ConnView, p: Paint) -> El {
    let t = p.t;
    let on = matches!(c.filter, Filter::On(_));
    let field = row()
        .key("tb-filter")
        .w(200.0)
        .h(28.0)
        .pl(space::S2)
        .pr(space::S1)
        .gap(6.0)
        .mr(space::S1)
        .radius(radius::ROW)
        .center_y()
        .fill(t.field)
        .border(p.hair, if on { t.border_strong } else { t.border_subtle })
        .child(icon(icons::SEARCH.at(p.scale), size::ICON).color(if on { t.text_2 } else { t.text_3 }));
    if !on {
        return field
            .role("button")
            .label("Filter")
            .shortcut_hint(chord("mod+f"))
            .hover(t.hover)
            .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::OpenFilter, tx))
            .tab_press()
            .child(text("Filter").font(type_::BODY).color(t.text_3).grow(1.0))
            .child(kbd(p, &chord("mod+f")).a11y_hidden());
    }
    // The field's box rings while the keyboard has the field (a caret alone is easy to miss).
    let field = field.focus_visible_within_ring(2.0, 1.0, t.focus);
    let input = text_input(&s.ui.filter_edit, type_::BODY)
        .key("tb-filter-input")
        .label("Filter")
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .color(t.text)
        .placeholder("Filter", t.text_3)
        .on_edit(|s: &mut App| &mut s.ui.filter_edit)
        .on_input_change(|s: &mut App, tx: &mut Tx| {
            let text = s.ui.filter_edit.text.clone();
            run(s, Action::FilterChanged(text), tx);
        })
        .on_input_key(filter_key);
    let clear = el()
        .key("tb-filter-clear")
        .role("button")
        .label("Clear filter")
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(t.hover)
        .press(t.press)
        .on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::CloseFilter, tx))
        .tab_press()
        .child(icon(icons::CLOSE.at(p.scale), size::ICON).color(t.text_3));
    field.child(input).child(clear)
}

/// Keys the filter field hands to the list before its editor sees them; Properties (⌘I,
/// Alt+Return) too, which a field keeps from the window's shortcuts otherwise.
fn filter_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let properties = Some(Shortcut::File(FileKey::Properties));
    let action = match ev.key {
        _ if keys::shortcut(ev, KeyPlatform::current()) == properties => Action::File(FileAction::Properties),
        Key::Escape => Action::CloseFilter,
        Key::Up => Action::Move { delta: -1, extend: ev.mods.shift },
        Key::Down => Action::Move { delta: 1, extend: ev.mods.shift },
        Key::Enter => Action::OpenSelection,
        _ => return false,
    };
    run(s, action, tx);
    true
}
