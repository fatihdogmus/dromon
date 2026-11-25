//! File operations in the app (§3.7, §5 M3): the inline new-folder and rename field
//! (`edit.rs`), delete (its sheet, `sheets.rs`, and its toast, `toast.rs`), copy path, the in-app
//! clipboard (`clip.rs`), dragging rows (`drag.rs`) and the context menus (`menu.rs`).
//!
//! Everything that changes the server does nothing while the host on screen isn't connected
//! (the listing is read-only while it reconnects, §6.8); copying and copy path still work.
//! [`App::sync_files`] runs after every action and event, and closes what a change left behind:
//! a field whose folder or row went away, a drag whose folder did.

use std::collections::HashMap;

use dromon_core::{HostId, RemotePath};
use dromon_transfer::{JobRequest, RemoteItem, unique_name};
use rune::TextEdit;

use super::clip::{Clip, paths_text};
use super::drag::{DragTarget, Over, RowDrag};
use super::edit::{self, Edit, EditEffect, EditEvent, Field};
use super::menu::{MenuKind, OpenMenu};
use super::service::{Command, FileOp, OpId, TransferCommand};
use super::toast::{Toast, ToastId, Toasts, delete_toast};
use super::{App, ConnView, Effect, Listing, hosts};

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;

/// What a new folder is called, unless that's taken.
pub const NEW_FOLDER: &str = "New folder";

/// The file operations' own state.
#[derive(Clone, Debug, Default)]
pub struct FileState {
    pub edit: Edit,
    pub clip: Option<Clip>,
    pub drag: Option<RowDrag>,
    /// The drawn context menu on screen (platforms without native menus).
    pub menu: Option<OpenMenu>,
    pub toasts: Toasts,
    /// A row the list scrolls into view once, numbered so each request is new.
    pub scroll: Option<(u64, String)>,
    scrolls: u64,
    /// What each delete was asked to delete, for its toast.
    deletes: HashMap<OpId, Vec<String>>,
    /// A row to scroll to once it lists: the folder just made, the item just renamed.
    pending: Option<(HostId, RemotePath, String)>,
    /// What [`FileState::field_edit`] hands out with no field up (always empty).
    spare: TextEdit,
}

impl FileState {
    /// The field's text model, for `text_input(..).on_edit`.
    pub fn field_edit(&mut self) -> &mut TextEdit {
        match self.edit.field_mut() {
            Some(f) => &mut f.edit,
            None => {
                self.spare.clear();
                &mut self.spare
            }
        }
    }
}

/// The file operations' actions (`Action::File`).
#[derive(Clone, Debug, PartialEq)]
pub enum FileAction {
    /// ⌘⇧N: a new folder's field at the top of the list.
    NewFolder,
    /// F2: the selected row's name becomes a field.
    Rename,
    /// The field's Return, its text changing, Escape, and losing focus.
    EditSubmit,
    EditChanged,
    EditCancel,
    EditBlur,
    /// ⌘⌫ (Delete elsewhere): ask, in a sheet.
    Delete,
    /// The delete sheet's Delete.
    ConfirmDelete,
    /// ⌘C, ⌘X: the selected rows onto the clip (and their paths onto the OS clipboard).
    Copy,
    Cut,
    /// ⌥⌘C: the selected rows' full paths (or the folder on screen's) as text.
    CopyPath,
    /// The OS clipboard held `text` when ⌘V (or ⌥⌘V, `as_move`) asked: paste the clip if it's
    /// still that.
    Paste { text: String, as_move: bool },
    /// ⌥⌘V, the menus' Paste: read the OS clipboard, then [`FileAction::Paste`].
    RequestPaste { as_move: bool },
    /// A row drag left its row (the row is selected first if it wasn't).
    DragStart(String),
    /// The drag is over `target` (or nothing that takes rows); `copy` while ⌥ (Ctrl) is held.
    DragOver { target: Option<DragTarget>, copy: bool },
    DragDrop,
    DragCancel,
    /// A right-click on a row or the empty list: the row is selected first; `at` opens the
    /// drawn menu there (`None`: a native menu opens instead).
    OpenMenu { kind: MenuKind, at: Option<(f32, f32)> },
    CloseMenu,
    /// An item of `kind`'s menu was picked.
    Pick { kind: MenuKind, id: String },
    DismissToast(ToastId),
    /// List the folder on screen again.
    Refresh,
    /// The list scrolled to request `n` ([`FileState::scroll`]).
    Scrolled(u64),
}

/// How the delete sheet lists names: the first five, then "and 12 more".
pub const DELETE_NAMES: usize = 5;

/// The delete sheet's title: "Delete 3 items permanently?", or "Delete ‘name’ permanently?".
pub fn delete_title(names: &[String]) -> String {
    match names {
        [one] => format!("Delete ‘{one}’ permanently?"),
        _ => format!("Delete {} items permanently?", crate::fmt::thousands(names.len() as u64)),
    }
}

/// "and 12 more" under the first names, when there are more.
pub fn delete_more(names: &[String]) -> Option<String> {
    (names.len() > DELETE_NAMES).then(|| format!("and {} more", crate::fmt::thousands((names.len() - DELETE_NAMES) as u64)))
}

impl App {
    /// The host on screen when it takes changes: connected, its folder listed.
    fn live(&self) -> Option<&ConnView> {
        self.conn().filter(|c| c.interactive() && matches!(c.listing, Listing::Ready(_)))
    }

    /// The selected rows on screen, in order.
    fn selected_names(&self) -> Vec<String> {
        let Some(c) = self.conn() else { return Vec::new() };
        c.rows().filter(|e| c.selection.contains(&e.name)).map(|e| e.name.clone()).collect()
    }

    fn items(c: &ConnView, names: &[String]) -> Vec<RemoteItem> {
        names.iter().filter_map(|n| c.find(n)).map(|e| RemoteItem { path: c.cwd.join(&e.name), kind: e.kind, size: e.size }).collect()
    }

    /// Whether a text field has the keyboard: the list's shortcuts leave it alone.
    pub fn text_focused(&self) -> bool {
        self.ui.filter_edit.focused || self.files.edit.field().is_some_and(|f| f.edit.focused) || self.ui.sheets.field().focused
    }

    /// The toasts on screen, oldest first: running and failed deletes, then notes.
    pub fn toasts(&self) -> Vec<Toast> {
        let ops = self.ops.ops.iter().filter_map(|o| delete_toast(o, self.files.deletes.get(&o.id).map_or(&[][..], Vec::as_slice)));
        ops.chain(self.files.toasts.notes()).collect()
    }

    /// The file operations' actions of [`App::act`].
    pub(super) fn file_action(&mut self, action: FileAction, fx: &mut Vec<Effect>) {
        match action {
            FileAction::NewFolder => self.new_folder(fx),
            FileAction::Rename => self.rename(fx),
            FileAction::EditSubmit => self.edit_step(EditEvent::Submit),
            FileAction::EditChanged => self.edit_step(EditEvent::Changed),
            FileAction::EditCancel => self.edit_step(EditEvent::Cancel),
            FileAction::EditBlur => self.edit_step(EditEvent::Blur),
            FileAction::Delete => {
                let names = self.selected_names();
                if let Some(c) = self.live().filter(|_| !names.is_empty()) {
                    let (host, folder) = (c.host.clone(), c.cwd.clone());
                    self.ui.sheets.open_delete(host, folder, names);
                }
            }
            FileAction::ConfirmDelete => self.confirm_delete(),
            FileAction::Copy | FileAction::Cut => {
                let names = self.selected_names();
                let Some(c) = self.conn().filter(|_| !names.is_empty()) else { return };
                let clip = Clip::new(c.host.clone(), App::items(c, &names), action == FileAction::Cut);
                fx.push(Effect::Copy(clip.text.clone()));
                self.files.clip = Some(clip);
            }
            FileAction::CopyPath => {
                let names = self.selected_names();
                let Some(c) = self.conn() else { return };
                let paths: Vec<RemotePath> = if names.is_empty() { vec![c.cwd.clone()] } else { names.iter().map(|n| c.cwd.join(n)).collect() };
                fx.push(Effect::Copy(paths_text(&paths)));
            }
            FileAction::Paste { text, as_move } => self.paste(&text, as_move),
            FileAction::RequestPaste { as_move } => {
                if self.live().is_some() && self.files.clip.is_some() {
                    fx.push(Effect::ReadClipboard { as_move });
                }
            }
            FileAction::DragStart(name) => self.drag_start(&name),
            FileAction::DragOver { target, copy } => {
                let over = target.and_then(|t| self.drag_target(t, copy));
                if let Some(d) = &mut self.files.drag {
                    d.over = over;
                }
            }
            FileAction::DragDrop => {
                let Some(drag) = self.files.drag.take() else { return };
                if self.live().is_some_and(|c| c.host == drag.host && c.cwd == drag.folder)
                    && let Some(req) = drag.request()
                {
                    self.start(req);
                }
            }
            FileAction::DragCancel => self.files.drag = None,
            FileAction::OpenMenu { kind, at } => {
                if let (MenuKind::Row(name), Some(c)) = (&kind, self.conn_mut())
                    && !c.selection.contains(name)
                    && c.find(name).is_some()
                {
                    c.selection.only(name);
                }
                self.files.menu = at.map(|at| OpenMenu { kind, at });
            }
            FileAction::CloseMenu => self.files.menu = None,
            FileAction::Pick { kind, id } => {
                self.files.menu = None;
                if let Some(action) = self.menu_action(&kind, &id) {
                    fx.extend(self.act(action));
                }
            }
            FileAction::DismissToast(ToastId::Op(id)) => {
                self.ops.forget(id);
                self.files.deletes.remove(&id);
            }
            FileAction::DismissToast(ToastId::Note(n)) => self.files.toasts.dismiss(n),
            FileAction::Refresh => {
                if let Some(c) = self.live() {
                    let (host, path) = (c.host.clone(), c.cwd.clone());
                    self.send(Command::List { host, path });
                }
            }
            FileAction::Scrolled(n) => {
                if self.files.scroll.as_ref().is_some_and(|(m, _)| *m == n) {
                    self.files.scroll = None;
                }
            }
        }
    }

    fn new_folder(&mut self, fx: &mut Vec<Effect>) {
        let Some(c) = self.live() else { return };
        let name = unique_name(NEW_FOLDER, |n| c.find(n).is_some());
        let field = Field::new_folder(c.host.clone(), c.cwd.clone(), &name);
        if let Some(c) = self.conn_mut() {
            c.selection.clear();
        }
        self.edit_step(EditEvent::Start(Box::new(field)));
        fx.push(Effect::ScrollTop);
    }

    fn rename(&mut self, fx: &mut Vec<Effect>) {
        let names = self.selected_names();
        let Some(c) = self.live() else { return };
        let [name] = &names[..] else { return };
        let Some(e) = c.find(name) else { return };
        let field = Field::rename(c.host.clone(), c.cwd.clone(), name, e.kind.is_dir_like());
        self.edit_step(EditEvent::Start(Box::new(field)));
        fx.push(Effect::Reveal(name.clone()));
    }

    /// Steps the field and does what it says: sends its operation (with the id recorded first,
    /// as a fixture answers inside the send), or selects what it made.
    fn edit_step(&mut self, ev: EditEvent) {
        match edit::step(&mut self.files.edit, ev) {
            Some(EditEffect::Send { host, op }) => {
                // Only while the host still takes changes; else the field gives up.
                if !self.live().is_some_and(|c| c.host == host) {
                    edit::step(&mut self.files.edit, EditEvent::Gone);
                    return;
                }
                let id = self.ops.begin(host.clone(), &op);
                edit::step(&mut self.files.edit, EditEvent::Sent(id));
                self.send(Command::FileOp { id, host, op });
            }
            Some(EditEffect::Finished { host, folder, name }) => {
                if let Some(c) = self.conns.get_mut(&host).filter(|c| c.cwd == folder) {
                    if c.find(&name).is_some() {
                        c.selection.only(&name);
                    } else {
                        c.reveal_after(&name);
                    }
                }
                self.files.pending = Some((host, folder, name));
            }
            None => {}
        }
    }

    fn confirm_delete(&mut self) {
        let Some((host, folder, names)) = self.ui.sheets.take_delete() else { return };
        if !self.conns.get(&host).is_some_and(|c| c.interactive()) {
            return;
        }
        let op = FileOp::Delete { paths: names.iter().map(|n| folder.join(n)).collect() };
        let id = self.ops.begin(host.clone(), &op);
        self.files.deletes.insert(id, names);
        self.send(Command::FileOp { id, host, op });
    }

    fn start(&mut self, req: JobRequest) {
        self.send(Command::Transfer(TransferCommand::Start(req)));
    }

    /// Pastes the clip into the folder on screen, if the OS clipboard (holding `text`) still
    /// holds it. A pasted cut (or a move) empties the clip, as Finder's does.
    fn paste(&mut self, text: &str, as_move: bool) {
        let Some(c) = self.live() else { return };
        let Some(clip) = self.files.clip.as_ref().filter(|k| k.matches(text)) else { return };
        let req = clip.paste(&c.host, c.cwd.clone(), as_move);
        if clip.cut || as_move {
            self.files.clip = None;
        }
        self.start(req);
    }

    fn drag_start(&mut self, name: &str) {
        if self.files.drag.is_some() || !self.live().is_some_and(|c| c.find(name).is_some()) {
            return;
        }
        if let Some(c) = self.conn_mut()
            && !c.selection.contains(name)
        {
            c.selection.only(name);
        }
        let names = self.selected_names();
        let Some(c) = self.conn() else { return };
        let items = App::items(c, &names);
        self.files.drag = Some(RowDrag { host: c.host.clone(), folder: c.cwd.clone(), names, items, over: None });
    }

    /// Where the drag would land on `target`, if it takes the rows: a folder row that isn't one
    /// of them, a breadcrumb that isn't the folder on screen, another host that has a folder on
    /// screen (one that was never opened has none).
    fn drag_target(&self, target: DragTarget, copy: bool) -> Option<Over> {
        let drag = self.files.drag.as_ref()?;
        let c = self.live().filter(|c| c.host == drag.host && c.cwd == drag.folder)?;
        let host = c.host.clone();
        let (to, into, label, copy) = match &target {
            DragTarget::Folder(name) => {
                c.find(name).filter(|e| e.kind.is_dir_like())?;
                (host, c.cwd.join(name), name.clone(), copy)
            }
            DragTarget::Crumb(path) => {
                if *path == c.cwd {
                    return None;
                }
                let label = path.file_name().map_or_else(|| self.host_label(&host), str::to_owned);
                (host, path.clone(), label, copy)
            }
            DragTarget::Host(other) => {
                let o = self.conns.get(other).filter(|o| *other != host && o.opened && hosts::is_live(&o.status))?;
                (other.clone(), o.cwd.clone(), self.host_label(other), true)
            }
        };
        if to == drag.host && drag.contains(&into) {
            return None;
        }
        Some(Over { target, copy, to, into, label })
    }

    fn host_label(&self, host: &HostId) -> String {
        self.hosts.get(host).map_or(host.as_str(), |h| h.display_name()).to_owned()
    }

    /// After every action and event: moves the field along with its operation, closes a field
    /// or drag whose folder (or row) went away, and asks the list to scroll to a row that just
    /// appeared.
    pub(super) fn sync_files(&mut self) {
        let on_screen = |s: &App, host: &HostId, folder: &RemotePath| {
            s.active.as_ref() == Some(host) && s.conn().is_some_and(|c| c.interactive() && c.cwd == *folder)
        };
        // The field's operation ended.
        if let Edit::Sending(_, Some(id)) = &self.files.edit
            && let Some(op) = self.ops.get(*id).filter(|o| o.finished)
        {
            let ev = match &op.error {
                None => EditEvent::Done(*id),
                Some(error) => EditEvent::Failed(*id, error.clone()),
            };
            self.edit_step(ev);
        }
        if let Some(f) = self.files.edit.field() {
            let row_gone = matches!(self.files.edit, Edit::Editing(_))
                && f.renaming().is_some_and(|name| self.conn().is_some_and(|c| c.find(name).is_none()));
            if !on_screen(self, &f.host, &f.folder) || row_gone {
                self.edit_step(EditEvent::Gone);
            }
        }
        if self.files.drag.as_ref().is_some_and(|d| !on_screen(self, &d.host, &d.folder)) {
            self.files.drag = None;
        }
        if let Some((host, folder, name)) = self.files.pending.clone() {
            if !on_screen(self, &host, &folder) {
                self.files.pending = None;
            } else if self.conn().is_some_and(|c| c.rows().any(|e| e.name == name)) {
                self.files.pending = None;
                self.files.scrolls += 1;
                self.files.scroll = Some((self.files.scrolls, name));
            }
        }
    }
}
