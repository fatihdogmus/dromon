//! The window's shortcut dispatch: which surface a [`Shortcut`] (`keys.rs` maps the key) goes
//! to, and the [`Action`] it becomes there. Window shortcuts (⌘, ⌘N ⌘W) work under anything;
//! a sheet that asks (a conflict, quitting, updating) or the bug report holds the rest; the quick-connect popover,
//! the palette, the Settings page and the preview panel take their own first; what's left is the
//! list's, except where a focused text field keeps the key (§7).

use super::keys::{self, Shortcut};
use super::remote_edit::RemoteEditAction;
use super::{Action, App, Effect, HostAction, PaletteAction, PreviewAction, SearchAction};

impl App {
    /// Maps a shortcut to its action (`now` is for type-to-select). The conflict sheet is
    /// modal like the prompt sheets: while it's up, the list's shortcuts do nothing.
    pub fn shortcut(&mut self, sc: Shortcut, now: f64) -> Vec<Effect> {
        // ⌘, shows the Settings page under whatever is modal here, as the app menu's item does;
        // ⌘N and ⌘W are about windows, not this one's content.
        match sc {
            Shortcut::Settings => return self.act(Action::OpenSettings),
            Shortcut::NewWindow => return self.act(Action::NewWindow),
            Shortcut::CloseWindow => return self.act(Action::CloseWindow),
            _ => {}
        }
        let conflict = self.conflict_on_screen().is_some() || self.edit_conflict_on_screen().is_some();
        if conflict || self.ui.confirm_quit || self.update.confirming() || self.report.is_open() {
            return Vec::new();
        }
        // The quick-connect popover is modal too: Escape closes it, nothing reaches the list.
        if self.ui.quick.open {
            return match sc {
                Shortcut::Escape => self.act(Action::HostBook(HostAction::CloseQuick)),
                _ => Vec::new(),
            };
        }
        // So is the palette: only its own keys (`palette.rs`).
        if self.ui.palette.is_open() {
            return self.palette_shortcut(sc);
        }
        // The Settings page has a few of its own; the folder view's wait for it (`page.rs`).
        if self.on_settings() {
            return self.settings_shortcut(sc);
        }
        // The preview panel takes a few, lets the cursor's through, and holds the rest.
        if self.ui.preview.is_open()
            && let Some(fx) = self.preview_shortcut(&sc, now)
        {
            return fx;
        }
        let action = match sc {
            // ⌘K opens the palette from anywhere, a focused field included.
            Shortcut::Palette => Action::Palette(PaletteAction::Toggle),
            Shortcut::QuickConnect => Action::QuickConnect,
            Shortcut::Settings | Shortcut::NewWindow | Shortcut::CloseWindow => return Vec::new(),
            Shortcut::Download => Action::Download,
            Shortcut::Upload => Action::PickUpload,
            Shortcut::Back => Action::Back,
            Shortcut::Forward => Action::Forward,
            Shortcut::Up => Action::GoUp,
            Shortcut::Open => Action::OpenSelection,
            Shortcut::SelectAll => Action::SelectAll,
            Shortcut::ToggleHidden => Action::ToggleHidden,
            Shortcut::Filter => Action::OpenFilter,
            Shortcut::Search => Action::Search(SearchAction::Open),
            Shortcut::Escape => Action::Escape,
            Shortcut::Move { delta, extend } => Action::Move { delta, extend },
            Shortcut::Page { down, extend } => {
                Action::Move { delta: if down { keys::PAGE_ROWS } else { -keys::PAGE_ROWS }, extend }
            }
            Shortcut::End { last, extend } => Action::End { last, extend },
            Shortcut::Type(text) => Action::Type { text, now },
            // Space types while type-to-select is collecting letters ("my file"), as in Finder.
            Shortcut::Preview if self.ui.type_select.collecting(now) => Action::Type { text: " ".into(), now },
            Shortcut::Preview => Action::Preview(PreviewAction::Toggle),
            // Shift+F10 and the Menu key: the cursor row's menu, else the empty list's.
            Shortcut::ContextMenu => return self.hit_menu_key().unwrap_or_else(|| self.context_menu_key()),
            // A focused text field keeps its keys (§7): these are the list's.
            Shortcut::File(_)
            | Shortcut::GoToPath
            | Shortcut::Host(_)
            | Shortcut::ShowTransfers
            | Shortcut::EditFile
                if self.text_focused() =>
            {
                return Vec::new();
            }
            Shortcut::File(k) => Action::File(k.action()),
            Shortcut::GoToPath => Action::Palette(PaletteAction::OpenPath),
            Shortcut::Host(n) => match n.checked_sub(1).and_then(|i| self.sidebar_order().get(i).cloned()) {
                Some(host) => Action::SelectHost(host),
                None => return Vec::new(),
            },
            Shortcut::ShowTransfers => Action::ToggleTransfers,
            Shortcut::OpenTerminal => Action::OpenTerminal(self.cursor_row()),
            Shortcut::EditFile => Action::RemoteEdit(RemoteEditAction::Edit(None)),
        };
        self.act(action)
    }
}
