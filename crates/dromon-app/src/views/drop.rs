//! Uploading by dropping files from Finder or Explorer (§3.6, §6.8 "Drop target"): the list
//! area takes files for the folder on screen, a folder row for itself. While a drag is over a
//! target the area gets a 1.5 pt accent outline and a 6% accent wash (a folder row lifts
//! instead), and a pill at the bottom says where the files would go. In on `snap`, out on
//! `move`. Nothing takes files unless the connection is live and the folder listed.

use std::path::PathBuf;

use rune::{El, Transition, Tx, col, icon, row, text};

use super::{Paint, run};
use crate::icons;
use crate::state::{Action, App, ConnView, DropHover, DropTarget, Listing};
use crate::theme::{motion, radius, size, space, type_};

#[cfg(test)]
#[path = "drop_tests.rs"]
mod tests;

/// The longest folder path the pill shows whole; longer ones keep their last folders.
const PATH_CHARS: usize = 36;

/// Whether the connection on screen takes dropped files now.
pub fn takes_files(s: &App) -> bool {
    s.conn().is_some_and(|c| c.interactive() && matches!(c.listing, Listing::Ready(_)))
}

fn count(tx: &Tx) -> usize {
    tx.drag_paths().map_or(0, <[PathBuf]>::len)
}

/// The list area of `c` as a drop target for its folder, with the drop highlight and pill.
pub fn area(s: &App, c: &ConnView, body: El, p: Paint) -> El {
    let mut area = col().key("drop-area").w_full().h_full().min_h(0.0).child(body);
    if !(c.interactive() && matches!(c.listing, Listing::Ready(_))) {
        return area;
    }
    area = area
        .on_drop_files(|s: &mut App, files, tx| run(s, Action::Upload { paths: files.to_vec(), folder: None }, tx))
        .accepts_drag(|s: &App, _| takes_files(s))
        .on_drop_hover(|s: &mut App, over, tx| {
            let count = count(tx);
            run(s, Action::DropHover { target: DropTarget::List, over, count }, tx)
        })
        // Rows dragged within the window: where they'd go.
        .children(super::drag::pill(s, p));
    let Some(hover) = s.transfers.drop.as_ref() else { return area };
    let t = p.t;
    if hover.target == DropTarget::List {
        area = area.child(
            col()
                .key("drop-outline")
                .pin(Some(space::S1), Some(space::S1), Some(space::S1), Some(space::S1))
                .radius(radius::ROW)
                .fill(t.accent.alpha(0.06))
                .border(1.5, t.accent)
                .inert()
                .transition(Transition::opacity())
                .transition_motion(motion::SNAP, motion::MOVE),
        );
    }
    area.child(pill(c, hover, p))
}

/// Makes a folder row take dropped files for that folder.
pub fn folder_row(r: El, name: &str) -> El {
    let (target, drop) = (name.to_owned(), name.to_owned());
    r.on_drop_files(move |s: &mut App, files, tx| run(s, Action::Upload { paths: files.to_vec(), folder: Some(drop.clone()) }, tx))
        .accepts_drag(|s: &App, _| takes_files(s))
        .on_drop_hover(move |s: &mut App, over, tx| {
            let count = count(tx);
            run(s, Action::DropHover { target: DropTarget::Folder(target.clone()), over, count }, tx)
        })
}

/// Whether the drag is over the folder row `name`.
pub fn over_folder(s: &App, name: &str) -> bool {
    matches!(&s.transfers.drop, Some(DropHover { target: DropTarget::Folder(f), .. }) if f == name)
}

/// "Upload 3 items to /var/www", or to the folder row under the drag.
pub fn pill_text(hover: &DropHover, cwd: &str) -> String {
    let what = if hover.count == 1 { "1 item".to_owned() } else { format!("{} items", crate::fmt::thousands(hover.count as u64)) };
    let to = match &hover.target {
        DropTarget::List => short_path(cwd),
        DropTarget::Folder(name) => name.clone(),
    };
    format!("Upload {what} to {to}")
}

/// A folder path for the pill: whole when short, else "…/" and as many of its last folders as
/// fit (at least the last).
pub fn short_path(path: &str) -> String {
    if path.chars().count() <= PATH_CHARS {
        return path.to_owned();
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let mut tail: Vec<&str> = Vec::new();
    let mut len = 2; // "…/"
    for part in parts.iter().rev() {
        let add = part.chars().count() + usize::from(!tail.is_empty());
        if !tail.is_empty() && len + add > PATH_CHARS {
            break;
        }
        len += add;
        tail.push(part);
    }
    tail.reverse();
    format!("…/{}", tail.join("/"))
}

fn pill(c: &ConnView, hover: &DropHover, p: Paint) -> El {
    pill_row("drop", pill_text(hover, c.cwd.as_str()), &icons::UPLOAD, p)
}

/// The floating pill at the bottom of the list that says where a drag would go, keyed
/// `{key}-pill`, `{key}-pill-text` and `{key}-pill-row`.
pub fn pill_row(key: &str, label: String, glyph: &'static icons::IconPair, p: Paint) -> El {
    let t = p.t;
    let pill = row()
        .key(format!("{key}-pill"))
        .role("status")
        .label(label.clone())
        .h(32.0)
        .pl(space::S3)
        .pr(space::S4)
        .gap(space::S2)
        .radius(16.0)
        .center_y()
        .fill(t.sheet)
        .border(p.hair, t.accent.alpha(0.5))
        .shadow(8.0, 24.0, t.shadow)
        .child(icon(glyph.at(p.scale), size::ICON).color(t.accent))
        .child(text(label).key(format!("{key}-pill-text")).font(type_::BODY_STRONG).color(t.text));
    row()
        .key(format!("{key}-pill-row"))
        .pin(Some(0.0), None, Some(0.0), Some(space::S6))
        .justify_center()
        .inert()
        .transition(Transition::offset(0.0, 8.0).combined(Transition::opacity()))
        .transition_motion(motion::SNAP, motion::MOVE)
        .child(pill)
}
