//! The ⌘K command palette (§6.8 "⌘K palette", §3.8 step 1, §7): one search field over three
//! groups, fuzzy matched on their titles (`fuzzy.rs`):
//!
//! - **Hosts**: every host in the sidebar, in its order; Return connects to it or switches to it.
//!   The first nine carry their ⌘1 … ⌘9.
//! - **Go to folder…**: a typed path that starts with `/` or `~` offers "Go to <path>" on the host
//!   on screen, and the host's recent folders (its history, most recent first, at most
//!   [`MAX_FOLDERS`]) match too. ⌘L opens the palette in [`Mode::Path`], on the folder on screen's
//!   path (all selected) and only this group: any path goes there, a relative one from the folder
//!   on screen, `~` being the session's home.
//! - **Actions**: what the app can do right now; what can't be done now isn't listed (never
//!   greyed), so nothing that changes the server shows while the host isn't connected.
//!
//! An empty query lists Hosts, then Actions. ↑ ↓ move (and wrap), Return runs the row, Escape or
//! a press outside closes it; a click runs a row. The palette is modal: while it's open the
//! list's shortcuts don't run. Its phases are [`Phase`], moved by the pure [`step`].

use dromon_core::{HostId, HostSpec, RemotePath};
use rune::TextEdit;

use super::fuzzy;
use super::keys::{Shortcut, host_chord};
use super::menu::{TERMINAL_CHORD, delete_chord};
use super::{Action, App, Effect, FileAction, Listing, SearchAction, TransferCommand, hosts};

#[cfg(test)]
#[path = "palette_tests.rs"]
mod tests;

/// The most rows each group shows.
pub const MAX_HOSTS: usize = 6;
pub const MAX_FOLDERS: usize = 8;
pub const MAX_ACTIONS: usize = 10;

/// What the palette is searching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// ⌘K: hosts, folders and actions.
    Commands,
    /// ⌘L: a folder's path on the host on screen.
    Path,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Closed,
    Open(Mode),
}

/// The palette's state: its phase, the field, the row under the cursor.
#[derive(Clone, Debug)]
pub struct Palette {
    pub phase: Phase,
    pub edit: TextEdit,
    /// Index into [`App::palette_items`].
    pub cursor: usize,
    /// The text [`Mode::Path`] opened with: until it's changed, every recent folder shows.
    prefill: Option<String>,
}

impl Default for Palette {
    fn default() -> Self {
        Palette { phase: Phase::Closed, edit: TextEdit::new(""), cursor: 0, prefill: None }
    }
}

impl Palette {
    pub fn mode(&self) -> Option<Mode> {
        match self.phase {
            Phase::Closed => None,
            Phase::Open(m) => Some(m),
        }
    }

    pub fn is_open(&self) -> bool {
        self.phase != Phase::Closed
    }

    /// Whether its field has the keyboard.
    pub fn focused(&self) -> bool {
        self.is_open() && self.edit.focused
    }

    /// What's typed, without the spaces around it.
    pub fn query(&self) -> &str {
        self.edit.text.trim()
    }

    /// Whether the path field still holds what it opened with.
    fn untouched(&self) -> bool {
        self.prefill.as_deref() == Some(self.edit.text.as_str())
    }
}

/// What moves the palette from phase to phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Opens (or re-opens) in `mode` with `text` in the field, all selected, the field focused.
    Open {
        mode: Mode,
        text: String,
    },
    /// The field's text changed: the cursor goes back to the first row.
    Edited,
    /// ↑ ↓ over `count` rows, wrapping at the ends.
    Move {
        delta: isize,
        count: usize,
    },
    Close,
}

/// The palette's state machine.
pub fn step(p: &mut Palette, ev: Event) {
    match ev {
        Event::Open { mode, text } => {
            let mut edit = TextEdit::new(text.clone());
            edit.set_selection(0, text.len());
            edit.focused = true;
            *p = Palette { phase: Phase::Open(mode), edit, cursor: 0, prefill: (mode == Mode::Path).then_some(text) };
        }
        _ if !p.is_open() => {}
        Event::Edited => p.cursor = 0,
        Event::Move { delta, count } => {
            p.cursor = match count {
                0 => 0,
                n => (p.cursor.min(n - 1) as isize + delta).rem_euclid(n as isize) as usize,
            };
        }
        Event::Close => *p = Palette::default(),
    }
}

/// The palette's own actions (`Action::Palette`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    /// ⌘K and the toolbar's ⌘K button: opens it, or closes it when it's open.
    Toggle,
    /// ⌘L and "Go to folder…": the folder on screen's path, all selected.
    OpenPath,
    /// Escape, a press outside.
    Close,
    /// The field's text changed.
    Edited,
    /// ↑ ↓.
    Move(isize),
    /// Return: runs the row under the cursor.
    Run,
    /// A click on the row with this key.
    Pick(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Hosts,
    Folders,
    Actions,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Hosts => "Hosts",
            Group::Folders => "Go to folder…",
            Group::Actions => "Actions",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Group::Hosts => "palette-group-hosts",
            Group::Folders => "palette-group-folders",
            Group::Actions => "palette-group-actions",
        }
    }
}

/// One row.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub group: Group,
    /// Its element key: `pal-host-<id>`, `pal-go`, `pal-folder-<path>`, `pal-<action id>`.
    pub key: String,
    pub title: String,
    /// Quieter text after the title: a host's address, the host a path is on.
    pub detail: Option<String>,
    /// The shortcut that does the same, as a chord spec (`mod+shift+n`).
    pub shortcut: Option<&'static str>,
    pub action: Action,
    /// The title's bytes the query matched, for highlighting.
    pub hits: Vec<(usize, usize)>,
}

/// Where a typed path leads from `cwd`: `/…` as it is, `~` and `~/…` from `home`, anything else
/// relative to `cwd` (`..` included). `None` for nothing typed, `~` without a known home, or
/// another user's `~name`.
pub fn resolve(typed: &str, cwd: &RemotePath, home: Option<&RemotePath>) -> Option<RemotePath> {
    let t = typed.trim();
    if t.is_empty() {
        return None;
    }
    if t == "~" {
        return home.cloned();
    }
    if let Some(rest) = t.strip_prefix("~/") {
        return home.map(|h| h.join(rest));
    }
    if t.starts_with('~') {
        return None;
    }
    Some(cwd.join(t))
}

/// One action row, before matching.
struct Command {
    id: &'static str,
    title: &'static str,
    shortcut: Option<&'static str>,
    detail: Option<String>,
    action: Action,
}

impl App {
    /// Every host in the sidebar, in the order it shows them (Connected, SSH Config, Saved,
    /// Recent): what ⌘1 … ⌘9 count.
    pub fn sidebar_order(&self) -> Vec<HostId> {
        self.hosts
            .sections(&self.conns)
            .into_iter()
            .flat_map(|(_, hosts)| hosts.into_iter().map(|h| h.id.clone()))
            .collect()
    }

    /// The palette's rows, in group order; empty while it's closed.
    pub fn palette_items(&self) -> Vec<Item> {
        let Some(mode) = self.ui.palette.mode() else { return Vec::new() };
        let query = self.ui.palette.query();
        match mode {
            Mode::Path => self.folder_items(query, mode),
            Mode::Commands => {
                let mut items = self.host_items(query);
                if !query.is_empty() {
                    items.extend(self.folder_items(query, mode));
                }
                items.extend(self.action_items(query));
                items
            }
        }
    }

    fn host_items(&self, query: &str) -> Vec<Item> {
        let order = self.sidebar_order();
        let specs: Vec<&HostSpec> = order.iter().filter_map(|id| self.hosts.get(id)).collect();
        let hits = fuzzy::rank(query, specs.iter().map(|h| h.display_name()));
        hits.into_iter()
            .take(MAX_HOSTS)
            .map(|(i, m)| Item {
                group: Group::Hosts,
                key: format!("pal-host-{}", specs[i].id),
                title: specs[i].display_name().to_owned(),
                detail: hosts::address(specs[i]),
                shortcut: host_chord(i + 1),
                action: Action::SelectHost(specs[i].id.clone()),
                hits: m.ranges,
            })
            .collect()
    }

    fn folder_items(&self, query: &str, mode: Mode) -> Vec<Item> {
        let Some(c) = self.conn().filter(|c| c.interactive()) else { return Vec::new() };
        let host = self.hosts.get(&c.host).map_or(c.host.as_str(), |h| h.display_name());
        let typed = match mode {
            Mode::Path => !query.is_empty(),
            Mode::Commands => query.starts_with('/') || query.starts_with('~'),
        };
        let go = if typed { resolve(query, &c.cwd, c.home.as_ref()) } else { None };
        let mut items = Vec::new();
        if let Some(path) = &go {
            items.push(Item {
                group: Group::Folders,
                key: "pal-go".into(),
                title: format!("Go to {path}"),
                detail: Some(format!("on {host}")),
                shortcut: None,
                action: Action::GoTo(path.clone()),
                hits: Vec::new(),
            });
        }
        let recent: Vec<RemotePath> =
            c.history.recent().into_iter().filter(|p| *p != c.cwd).take(MAX_FOLDERS).collect();
        let picked: Vec<(usize, Vec<(usize, usize)>)> = if mode == Mode::Path
            && (query.is_empty() || self.ui.palette.untouched())
        {
            (0..recent.len()).map(|i| (i, Vec::new())).collect()
        } else {
            fuzzy::rank(query, recent.iter().map(RemotePath::as_str)).into_iter().map(|(i, m)| (i, m.ranges)).collect()
        };
        for (i, hits) in picked {
            let path = &recent[i];
            if go.as_ref() == Some(path) {
                continue;
            }
            items.push(Item {
                group: Group::Folders,
                key: format!("pal-folder-{path}"),
                title: path.to_string(),
                detail: None,
                shortcut: None,
                action: Action::GoTo(path.clone()),
                hits,
            });
        }
        items
    }

    /// What can be done now, in the order an empty query lists them.
    fn commands(&self) -> Vec<Command> {
        let c = self.conn();
        let opened = c.is_some_and(|c| c.opened);
        let interactive = c.is_some_and(|c| c.interactive());
        let listed = interactive && c.is_some_and(|c| matches!(c.listing, Listing::Ready(_)));
        let selected = c.map_or(0, |c| c.selection.len());
        let picked = selected > 0 || c.is_some_and(|c| c.selection.cursor().is_some());
        let live = c.filter(|c| hosts::is_live(&c.status));
        let host = c.map(|c| self.hosts.get(&c.host).map_or(c.host.as_str(), |h| h.display_name()).to_owned());
        let mut out = Vec::new();
        let mut add = |on: bool, id, title, shortcut, action| {
            if on {
                out.push(Command { id, title, shortcut, detail: None, action });
            }
        };
        add(true, "quick-connect", "Quick connect…", Some("mod+shift+k"), Action::QuickConnect);
        add(interactive, "go-to-folder", "Go to folder…", Some("mod+l"), Action::Palette(PaletteAction::OpenPath));
        add(opened && listed, "filter", "Filter", Some("mod+f"), Action::OpenFilter);
        add(listed, "refresh", "Refresh", Some("mod+r"), Action::File(FileAction::Refresh));
        add(listed, "new-folder", "New folder", Some("mod+shift+n"), Action::File(FileAction::NewFolder));
        add(interactive && picked, "download", "Download", Some("mod+d"), Action::Download);
        add(interactive, "upload", "Upload files…", Some("mod+u"), Action::PickUpload);
        add(listed && selected == 1, "rename", "Rename", Some("f2"), Action::File(FileAction::Rename));
        add(listed && selected > 0, "delete", "Delete", Some(delete_chord()), Action::File(FileAction::Delete));
        add(opened, "copy-path", "Copy path", Some("mod+alt+c"), Action::File(FileAction::CopyPath));
        add(true, "show-transfers", "Show transfers", Some("mod+shift+j"), Action::ToggleTransfers);
        add(true, "hidden", "Toggle hidden files", Some("mod+shift+period"), Action::ToggleHidden);
        add(c.is_some(), "log", "Show connection log", None, Action::OpenLog);
        add(interactive, "terminal", "Open in Terminal", Some(TERMINAL_CHORD), Action::OpenTerminal(self.cursor_row()));
        if let Some(live) = live {
            add(true, "disconnect", "Disconnect", None, Action::Disconnect(live.host.clone()));
        }
        // Last, so an empty query's cap trims it rather than Disconnect; typing finds it.
        add(true, "settings", "Settings…", Some("mod+comma"), Action::OpenSettings);
        add(
            self.transfers.has_finished(),
            "clear-finished",
            "Clear finished transfers",
            None,
            Action::Transfer(TransferCommand::ClearFinished),
        );
        // Past an empty query's cap: typing finds it ("search", "sub…").
        let search = Action::Search(SearchAction::Open);
        add(opened && listed, "search", "Search in subfolders…", Some("mod+shift+f"), search);
        for cmd in &mut out {
            if matches!(cmd.id, "log" | "disconnect") {
                cmd.detail = host.clone();
            }
        }
        out
    }

    fn action_items(&self, query: &str) -> Vec<Item> {
        let commands = self.commands();
        fuzzy::rank(query, commands.iter().map(|c| c.title))
            .into_iter()
            .take(MAX_ACTIONS)
            .map(|(i, m)| {
                let c = &commands[i];
                Item {
                    group: Group::Actions,
                    key: format!("pal-{}", c.id),
                    title: c.title.to_owned(),
                    detail: c.detail.clone(),
                    shortcut: c.shortcut,
                    action: c.action.clone(),
                    hits: m.ranges,
                }
            })
            .collect()
    }

    /// Opens the palette: popovers and the drawn menu close, the filter field lets go.
    fn open_palette(&mut self, mode: Mode, text: String) {
        self.files.menu = None;
        self.transfers.open = false;
        self.ui.filter_edit.focused = false;
        self.ui.type_select.reset();
        step(&mut self.ui.palette, Event::Open { mode, text });
    }

    /// The palette's actions of [`App::act`].
    pub(super) fn palette_action(&mut self, action: PaletteAction, fx: &mut Vec<Effect>) {
        match action {
            PaletteAction::Toggle if self.ui.palette.is_open() => self.close_palette(fx),
            PaletteAction::Toggle => self.open_palette(Mode::Commands, String::new()),
            PaletteAction::OpenPath => {
                if let Some(path) = self.conn().filter(|c| c.interactive()).map(|c| c.cwd.to_string()) {
                    self.open_palette(Mode::Path, path);
                }
            }
            PaletteAction::Close => self.close_palette(fx),
            PaletteAction::Edited => step(&mut self.ui.palette, Event::Edited),
            PaletteAction::Move(delta) => {
                let count = self.palette_items().len();
                step(&mut self.ui.palette, Event::Move { delta, count });
            }
            PaletteAction::Run => {
                let cursor = self.ui.palette.cursor;
                if let Some(item) = self.palette_items().into_iter().nth(cursor) {
                    self.run_item(item.action, fx);
                }
            }
            PaletteAction::Pick(key) => {
                if let Some(item) = self.palette_items().into_iter().find(|i| i.key == key) {
                    self.run_item(item.action, fx);
                }
            }
        }
    }

    fn close_palette(&mut self, fx: &mut Vec<Effect>) {
        if self.ui.palette.is_open() {
            step(&mut self.ui.palette, Event::Close);
            fx.push(Effect::Blur);
        }
    }

    /// Runs a row: the palette closes (the keyboard goes back to the list) and the action runs.
    /// "Go to folder…" turns the palette to its path instead. Every row but "Settings…" is about
    /// the folder view, so it leaves the Settings page first (`page.rs`).
    fn run_item(&mut self, action: Action, fx: &mut Vec<Effect>) {
        if let Action::Palette(a) = action {
            return self.palette_action(a, fx);
        }
        self.close_palette(fx);
        if action != Action::OpenSettings {
            self.leave_settings();
        }
        fx.extend(self.act(action));
    }

    /// A shortcut while the palette is open: only its own keys (when its field doesn't have the
    /// keyboard, they arrive here); ⌘K closes it, ⌘L turns it to a path.
    pub(super) fn palette_shortcut(&mut self, sc: Shortcut) -> Vec<Effect> {
        let action = match sc {
            Shortcut::Palette => PaletteAction::Toggle,
            Shortcut::GoToPath => PaletteAction::OpenPath,
            Shortcut::Escape => PaletteAction::Close,
            Shortcut::Move { delta, .. } => PaletteAction::Move(delta),
            Shortcut::Open => PaletteAction::Run,
            _ => return Vec::new(),
        };
        self.act(Action::Palette(action))
    }
}
