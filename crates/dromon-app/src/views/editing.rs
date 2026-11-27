//! Edit in a local editor (`state/remote_edit.rs`): the transfers popover's Editing section and
//! the edit conflict sheet.
//!
//! The section heads the popover while any file is being edited: one row per file (its name,
//! host and how it stands, the full path on hover), with Stop editing, and Retry after a failed
//! upload. A file whose server copy changed says so with "Resolve…": that closes the popover,
//! and its sheet comes up, as a transfer's name clash does.
//!
//! The sheet (§6.8's conflict sheet's look): the server's copy changed since the edit opened or
//! last saved it. **Replace** and **Discard my changes** (danger: one loses their change, the
//! other yours), or **Keep both** (the default: nothing is lost). The two copies stand side by
//! side with their sizes and dates, the newer said in words. Shown while no other sheet, no
//! transfer clash, the popover or the palette is up (`App::edit_conflict_on_screen`); like the
//! transfer conflict sheet, there's nothing to cancel, so Escape does nothing.

use dromon_transfer::Side;
use rune::{El, LoopSpec, Tx, col, el, icon, row, text};

use super::conflict::{danger, date_text, size_text};
use super::sheets::{button, buttons, frame};
use super::widgets::Keyboard;
use super::{Paint, run};
use crate::icons::{self, IconPair};
use crate::state::remote_edit::{Phase, RemoteEditAction, Session};
use crate::state::{Action, App, EditChoice};
use crate::theme::{motion, radius, size, space, type_};

/// Sheet width, as the other sheets.
const WIDTH: f32 = 440.0;

fn host_name<'a>(s: &'a App, e: &'a Session) -> &'a str {
    s.hosts.get(&e.host).map_or(e.host.as_str(), |h| h.display_name())
}

/// How an edit stands, in a word or two (its row's line says it after the host).
pub fn status(e: &Session) -> String {
    match &e.phase {
        Phase::Downloading => "Downloading…".to_owned(),
        Phase::Open => "Editing".to_owned(),
        Phase::Uploading | Phase::Resolving { .. } => "Saving…".to_owned(),
        Phase::Conflict { .. } => "Changed on the server".to_owned(),
        Phase::Failed { error } => format!("Couldn't save: {error}"),
        Phase::Closed => "Stopped".to_owned(),
    }
}

/// The Editing section, while any file is being edited.
pub fn section(s: &App, p: Paint) -> Option<El> {
    if s.edits.sessions.is_empty() {
        return None;
    }
    let t = p.t;
    // The list's name says it to readers.
    let heading = text("EDITING").key("editing-title").font(type_::CAPTION).color(t.text_3).a11y_hidden();
    Some(
        col()
            .key("editing")
            .role("list")
            .label("Editing")
            .w_full()
            .pt(space::S3)
            .pb(space::S1)
            .border_sides([0.0, 0.0, p.hair, 0.0], t.border_subtle)
            .child(row().w_full().px(space::S4).pb(space::S1).child(heading))
            .children(s.edits.sessions.iter().map(|e| edit_row(s, p, e))),
    )
}

/// The tile left of a row: the file, a spinner while it moves, a warning when it needs you.
fn tile(p: Paint, e: &Session) -> El {
    let t = p.t;
    let glyph = |pair: &'static IconPair, color| icon(pair.at(p.scale), size::ICON).color(color);
    let inner = match &e.phase {
        Phase::Downloading | Phase::Uploading | Phase::Resolving { .. } => {
            glyph(&icons::SPINNER, t.text_2).looping(LoopSpec::rotation(0.9))
        }
        Phase::Conflict { .. } | Phase::Failed { .. } => glyph(&icons::WARNING, t.danger),
        Phase::Open | Phase::Closed => glyph(&icons::FILE_TEXT, t.text_2),
    };
    el().size(28.0, 28.0).radius(radius::ROW).fill(t.field).center().child(inner)
}

/// A small text button of a row.
fn row_button(p: Paint, key: String, label: &'static str, accent: bool, action: Action) -> El {
    let t = p.t;
    let b = row()
        .key(key)
        .role("button")
        .label(label)
        .h(22.0)
        .px(space::S2)
        .radius(radius::ROW - 2.0)
        .center()
        .press_scale(0.97)
        .press_motion(rune::Anim::INSTANT, motion::SNAP)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| run(s, action.clone(), tx))
        .tab_press();
    if accent {
        b.border(p.hair, t.accent.alpha(0.5))
            .hover(t.accent.alpha(0.1))
            .press(t.accent.alpha(0.16))
            .child(text(label).font(type_::META_STRONG).color(t.accent))
    } else {
        b.hover(t.hover).press(t.press).child(text(label).font(type_::META_STRONG).color(t.text_2))
    }
}

/// One file being edited.
fn edit_row(s: &App, p: Paint, e: &Session) -> El {
    let t = p.t;
    let id = e.id;
    let host = host_name(s, e);
    let status = status(e);
    let needs_you = matches!(e.phase, Phase::Conflict { .. } | Phase::Failed { .. });
    let mut actions = Vec::new();
    if matches!(e.phase, Phase::Conflict { .. }) {
        // Closing the popover brings up its sheet.
        actions.push(row_button(p, format!("edit-resolve-{}", id.0), "Resolve…", true, Action::ToggleTransfers));
    }
    if matches!(e.phase, Phase::Failed { .. }) {
        let retry = Action::RemoteEdit(RemoteEditAction::Retry(id));
        actions.push(row_button(p, format!("edit-retry-{}", id.0), "Retry", false, retry));
    }
    let stop = Action::RemoteEdit(RemoteEditAction::Stop(id));
    actions.push(row_button(p, format!("edit-stop-{}", id.0), "Stop editing", false, stop));
    let title = text(e.name().to_owned())
        .key(format!("edit-title-{}", id.0))
        .font(type_::BODY)
        .color(t.text)
        .truncate()
        .w_full();
    let line = text(format!("{host} · {status}"))
        .key(format!("edit-line-{}", id.0))
        .font(type_::META)
        .color(if needs_you { t.danger } else { t.text_2 })
        .mt(2.0)
        .truncate()
        .w_full();
    let folder = e.path.parent().map_or_else(|| "/".to_owned(), |f| f.to_string());
    // The whole path on hover, and what went wrong in full (the line may be cut).
    let help = if needs_you { format!("{}\n{status}", e.path) } else { e.path.to_string() };
    row()
        .key(format!("edit-{}", id.0))
        .role("listitem")
        .label(format!("{} in {folder} on {host}, {status}", e.name()))
        .help(help)
        .w_full()
        .px(space::S1)
        .child(
            row()
                .w_full()
                .px(space::S2)
                .py(space::S2)
                .gap(space::S3)
                .center_y()
                .radius(radius::ROW)
                .hover(t.hover)
                .motion(motion::SNAP)
                .child(tile(p, e))
                .child(col().grow(1.0).min_w(0.0).child(title).child(line))
                .child(row().gap(2.0).children(actions)),
        )
}

/// One copy's column: its label, size and date.
fn side_column(p: Paint, key: &'static str, label: &'static str, side: &Side, newer: bool) -> El {
    let t = p.t;
    let key_of = |part: &str| format!("{key}-{part}");
    col()
        .key(key)
        .grow(1.0)
        .min_w(0.0)
        .gap(space::S1)
        .child(text(label.to_uppercase()).key(key_of("label")).font(type_::CAPTION).color(t.text_3))
        .child(text(size_text(side)).key(key_of("size")).font(type_::META).color(t.text))
        .child(
            text(date_text(side, newer))
                .key(key_of("date"))
                .font(type_::META)
                .color(if newer { t.text } else { t.text_2 })
                .w_full()
                .wrap(),
        )
}

/// The explanation under the sheet's title. Keep both's name isn't known until the upload finds
/// a free one, so the Saved toast names it after.
const EXPLAIN: &str = "Someone changed it on the server since you opened or last saved it. Replace uploads yours \
                       over theirs. Keep both uploads yours beside theirs under a new name. Discard my changes \
                       downloads theirs over your copy.";

/// The edit conflict sheet, while one is on screen.
pub fn conflict_sheet(s: &App, p: Paint) -> Option<El> {
    let e = s.edit_conflict_on_screen()?;
    let Phase::Conflict { server, local } = &e.phase else { return None };
    let t = p.t;
    let heading = format!("‘{}’ changed on {}", e.name(), host_name(s, e));
    let (theirs, yours) = match (server.mtime, local.mtime) {
        (Some(a), Some(b)) => (a > b, b > a),
        _ => (false, false),
    };
    let sides = row()
        .key("edit-conflict-sides")
        .w_full()
        .gap(space::S4)
        .padding(space::S3)
        .radius(radius::ROW)
        .fill(t.surface)
        .border(p.hair, t.border_subtle)
        .child(side_column(p, "edit-conflict-server", "On the server", server, theirs))
        .child(side_column(p, "edit-conflict-local", "Yours", local, yours));
    let resolve = |choice| Action::RemoteEdit(RemoteEditAction::Resolve(choice));
    let actions = vec![
        danger(p, "edit-conflict-replace", "Replace", resolve(EditChoice::Replace)),
        danger(p, "edit-conflict-discard", "Discard my changes", resolve(EditChoice::Discard)),
        row().grow(1.0),
        button(p, "edit-conflict-keep-both", "Keep both", true, resolve(EditChoice::KeepBoth)),
    ];
    Some(
        frame(p, "edit-conflict", &heading, WIDTH)
            .child(text(heading.clone()).key("sheet-title").font(type_::TITLE).color(t.text).w_full().wrap_anywhere())
            .child(text(EXPLAIN).key("edit-conflict-explain").font(type_::BODY).color(t.text_2).w_full().wrap())
            .child(sides)
            .child(buttons(actions)),
    )
}
