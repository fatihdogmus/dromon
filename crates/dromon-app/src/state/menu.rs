//! Context menus (§5 M3): a row's, and the list's empty area's (both offer New folder: a full
//! folder may leave no empty area to right-click), and a search result's (`search.rs`: what
//! works on a result away from its folder). One model for both ways of
//! showing them: a native menu where Rune has one (macOS, Windows), a drawn one elsewhere
//! (`views/menu.rs`). Items that would change the server are disabled while the host isn't
//! connected; Paste also needs a clip.
//!
//! "Open in Terminal" is in both (and in the palette, and ⌘⇧T): the user's terminal app, signed
//! in over its own ssh, in a folder row's folder, else in the folder on screen. The state only
//! says what to open ([`Effect::OpenTerminal`]); `crate::terminal` starts it off the UI thread.
//!
//! A row's "Edit" opens its file in a local editor (`remote_edit.rs`); it's disabled for what
//! quick preview wouldn't read as text (folders, links, images, binaries) and while the host
//! isn't connected.

use dromon_core::HostId;

use super::files::FileAction;
use super::remote_edit::{EDIT_CHORD, RemoteEditAction};
use super::{Action, App, Effect};

/// "Open in Terminal"'s chord: ⌘⇧T (Ctrl+Shift+T elsewhere).
pub const TERMINAL_CHORD: &str = "mod+shift+t";

#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;

/// Which menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// Right-clicked on this row (which is selected first if it wasn't).
    Row(String),
    /// Right-clicked on the list where there's no row.
    Empty,
    /// Right-clicked on a search result, by its key (`search::hit_key`; selected first if it
    /// wasn't): Download, Copy path, Properties.
    Hit(String),
    /// Right-clicked on a host in the sidebar (`hostbook.rs`).
    Host(HostId),
}

/// One choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub id: &'static str,
    pub title: &'static str,
    pub enabled: bool,
    /// A check mark's state, for a toggle.
    pub checked: Option<bool>,
    /// What can't be undone (Delete): drawn in the danger colour.
    pub danger: bool,
    /// The shortcut hint, as a chord spec (`mod+c`).
    pub shortcut: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

impl MenuEntry {
    pub fn item(&self) -> Option<&MenuItem> {
        match self {
            MenuEntry::Item(i) => Some(i),
            MenuEntry::Separator => None,
        }
    }
}

/// A drawn menu on screen, at a window point.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenMenu {
    pub kind: MenuKind,
    pub at: (f32, f32),
}

fn item(id: &'static str, title: &'static str, enabled: bool, shortcut: Option<&'static str>) -> MenuEntry {
    MenuEntry::Item(MenuItem { id, title, enabled, checked: None, danger: false, shortcut })
}

/// Delete's chord: ⌘⌫ on macOS, Delete elsewhere (§7).
pub fn delete_chord() -> &'static str {
    if cfg!(target_os = "macos") { "mod+backspace" } else { "delete" }
}

/// Properties' chord: ⌘I on macOS (Finder's Get Info), Alt+Return elsewhere (Explorer's).
pub fn properties_chord() -> &'static str {
    if cfg!(target_os = "macos") { "mod+i" } else { "alt+enter" }
}

impl App {
    /// The menu's entries, as things stand.
    pub fn menu(&self, kind: &MenuKind) -> Vec<MenuEntry> {
        let live = self.conn().is_some_and(|c| c.interactive());
        let paste = live && self.clip().is_some();
        match kind {
            MenuKind::Row(name) => {
                let one = self.conn().is_some_and(|c| c.selection.len() == 1);
                vec![
                    item("open", "Open", live, None),
                    item("edit", "Edit", self.editable(name), Some(EDIT_CHORD)),
                    item("download", "Download", live, Some("mod+d")),
                    // A folder row's folder; a file's is the folder on screen.
                    item("terminal", "Open in Terminal", live, Some(TERMINAL_CHORD)),
                    MenuEntry::Separator,
                    item("copy", "Copy", true, Some("mod+c")),
                    item("cut", "Cut", true, Some("mod+x")),
                    item("paste", "Paste", paste, Some("mod+v")),
                    item("copy-path", "Copy path", true, Some("mod+alt+c")),
                    MenuEntry::Separator,
                    // In the folder on screen: a full folder may have no empty area to right-click.
                    item("new-folder", "New folder", live, Some("mod+shift+n")),
                    item("rename", "Rename", live && one, Some("f2")),
                    MenuEntry::Item(MenuItem {
                        id: "delete",
                        title: "Delete",
                        enabled: live,
                        checked: None,
                        danger: true,
                        shortcut: Some(delete_chord()),
                    }),
                    MenuEntry::Separator,
                    // Shows what the listing knows, so it works while reconnecting too.
                    item("properties", "Properties", self.properties_row().is_some(), Some(properties_chord())),
                ]
            }
            MenuKind::Empty => {
                let hidden = self.conn().map_or(self.show_hidden_default, |c| c.show_hidden);
                vec![
                    item("new-folder", "New folder", live, Some("mod+shift+n")),
                    item("paste", "Paste", paste, Some("mod+v")),
                    MenuEntry::Separator,
                    item("terminal", "Open in Terminal", live, Some(TERMINAL_CHORD)),
                    item("refresh", "Refresh", live, None),
                    MenuEntry::Item(MenuItem {
                        id: "hidden",
                        title: "Show hidden files",
                        enabled: true,
                        checked: Some(hidden),
                        danger: false,
                        shortcut: Some("mod+shift+period"),
                    }),
                ]
            }
            MenuKind::Hit(_) => vec![
                item("download", "Download", live, Some("mod+d")),
                item("copy-path", "Copy path", true, Some("mod+alt+c")),
                MenuEntry::Separator,
                item("properties", "Properties", self.hit_properties().is_some(), Some(properties_chord())),
            ],
            MenuKind::Host(id) => self.host_menu(id),
        }
    }

    /// What picking `id` in `kind`'s menu does; `None` for an item that isn't there or is
    /// disabled now.
    pub fn menu_action(&self, kind: &MenuKind, id: &str) -> Option<Action> {
        if let MenuKind::Host(host) = kind {
            return self.host_menu_action(host, id);
        }
        let entries = self.menu(kind);
        entries.iter().filter_map(MenuEntry::item).find(|i| i.id == id && i.enabled)?;
        Some(match (kind, id) {
            (MenuKind::Row(name), "open") => Action::Open(name.clone()),
            (MenuKind::Row(name), "edit") => Action::RemoteEdit(RemoteEditAction::Edit(Some(name.clone()))),
            (_, "download") => Action::Download,
            (_, "copy") => Action::File(FileAction::Copy),
            (_, "cut") => Action::File(FileAction::Cut),
            (_, "paste") => Action::File(FileAction::RequestPaste { as_move: false }),
            (_, "copy-path") => Action::File(FileAction::CopyPath),
            (_, "rename") => Action::File(FileAction::Rename),
            (_, "delete") => Action::File(FileAction::Delete),
            (_, "new-folder") => Action::File(FileAction::NewFolder),
            (_, "refresh") => Action::File(FileAction::Refresh),
            (_, "hidden") => Action::ToggleHidden,
            (MenuKind::Row(_) | MenuKind::Hit(_), "properties") => Action::File(FileAction::Properties),
            (MenuKind::Row(name), "terminal") => Action::OpenTerminal(Some(name.clone())),
            (_, "terminal") => Action::OpenTerminal(None),
            _ => return None,
        })
    }

    /// The row the keyboard is on, if it's on screen: where ⌘⇧T and the palette's "Open in
    /// Terminal" open when it's a folder.
    pub(super) fn cursor_row(&self) -> Option<String> {
        let c = self.conn()?;
        c.selection.cursor().filter(|name| c.row_names().contains(name)).map(str::to_owned)
    }

    /// The terminal app couldn't be opened: a toast says why.
    pub(super) fn terminal_failed(&mut self, why: &str) {
        self.files.toasts.push(format!("Couldn't open a terminal: {why}"));
    }

    /// "Open in Terminal": in the row `row` when it's a folder, else in the folder on screen;
    /// nothing while the host isn't connected.
    pub(super) fn open_terminal(&mut self, row: Option<String>, fx: &mut Vec<Effect>) {
        let Some(c) = self.conn().filter(|c| c.interactive()) else { return };
        let Some(host) = self.hosts.get(&c.host) else { return };
        let folder = match row.as_deref().and_then(|name| c.find(name)) {
            Some(e) if e.kind.is_dir_like() => c.cwd.join(&e.name),
            _ => c.cwd.clone(),
        };
        fx.push(Effect::OpenTerminal {
            window: self.window,
            terminal: self.settings.terminal,
            host: host.clone(),
            folder,
        });
    }
}
