//! Context menus (§5 M3): a row's, and the list's empty area's. One model for both ways of
//! showing them: a native menu where Rune has one (macOS, Windows), a drawn one elsewhere
//! (`views/menu.rs`). Items that would change the server are disabled while the host isn't
//! connected; Paste also needs a clip.

use dromon_core::HostId;

use super::files::FileAction;
use super::{Action, App};

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
            MenuKind::Row(_) => {
                let one = self.conn().is_some_and(|c| c.selection.len() == 1);
                vec![
                    item("open", "Open", live, None),
                    item("download", "Download", live, Some("mod+d")),
                    MenuEntry::Separator,
                    item("copy", "Copy", true, Some("mod+c")),
                    item("cut", "Cut", true, Some("mod+x")),
                    item("paste", "Paste", paste, Some("mod+v")),
                    item("copy-path", "Copy path", true, Some("mod+alt+c")),
                    MenuEntry::Separator,
                    item("rename", "Rename", live && one, Some("f2")),
                    MenuEntry::Item(MenuItem { id: "delete", title: "Delete", enabled: live, checked: None, danger: true, shortcut: Some(delete_chord()) }),
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
            (MenuKind::Row(_), "properties") => Action::File(FileAction::Properties),
            _ => return None,
        })
    }
}
