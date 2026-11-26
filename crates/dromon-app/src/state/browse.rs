//! The folder view's actions of [`App::act`]: opening rows, moving between folders and through
//! the history, the selection and type-to-select, sort, hidden files and the filter. Picking a
//! host is `mod.rs`'s; transfers, file operations and the palette have their own modules.

use dromon_core::RemotePath;
use rune::TextEdit;

use super::{Action, App, Command, Effect, Filter, Nav, Selection, selection};

impl App {
    /// The folder view's actions of [`App::act`].
    pub(super) fn browse_action(&mut self, action: Action, fx: &mut Vec<Effect>) {
        match action {
            Action::Open(name) => self.open(&name, fx),
            Action::OpenSelection => {
                if let Some(name) = self.conn().and_then(|c| c.selection.cursor().map(str::to_owned)) {
                    self.open(&name, fx);
                }
            }
            Action::GoUp => {
                if let Some(parent) = self.conn().and_then(|c| c.cwd.parent()) {
                    self.navigate(parent, Nav::Visit, fx);
                }
            }
            Action::Back => self.history(Nav::Back, fx),
            Action::Forward => self.history(Nav::Forward, fx),
            Action::GoTo(path) => self.navigate(path, Nav::Visit, fx),
            Action::Click { name, extend, toggle } => self.click(&name, extend, toggle),
            Action::Move { delta, extend } => self.with_rows(fx, |sel, rows| sel.step(rows, delta, extend)),
            Action::End { last, extend } => self.with_rows(fx, |sel, rows| sel.jump_to_end(rows, last, extend)),
            Action::SelectAll => self.with_rows(fx, |sel, rows| {
                sel.select_all(rows);
                None
            }),
            Action::Type { text, now } => {
                let prefix = self.ui.type_select.push(&text, now).to_owned();
                self.with_rows(fx, |sel, rows| {
                    let i = selection::find_prefix(rows, &prefix, sel.cursor())?;
                    sel.only(rows[i]);
                    Some(rows[i].to_owned())
                });
            }
            Action::Sort(key) => {
                self.sort = self.sort.toggled(key);
                let sort = self.sort;
                self.save(|s| s.sort = Some(sort));
                if let Some(c) = self.conn_mut() {
                    c.set_sort(sort);
                    if let Some(cur) = c.selection.cursor() {
                        fx.push(Effect::Reveal(cur.to_owned()));
                    }
                }
            }
            Action::ToggleHidden => {
                // For this session: the remembered default is Settings' (`settings.rs`).
                self.show_hidden_default = !self.show_hidden_default;
                let show = self.show_hidden_default;
                for c in self.conns.values_mut() {
                    if c.show_hidden != show {
                        c.toggle_hidden();
                    }
                }
            }
            Action::OpenFilter => {
                let text = self.ui.filter_edit.text.clone();
                if let Some(c) = self.conn_mut() {
                    c.set_filter(Filter::On(text));
                    self.ui.filter_edit.focused = true;
                }
            }
            Action::CloseFilter => {
                self.ui.filter_edit = TextEdit::new("");
                if let Some(c) = self.conn_mut() {
                    c.set_filter(Filter::Off);
                    if let Some(cur) = c.selection.cursor() {
                        fx.push(Effect::Reveal(cur.to_owned()));
                    }
                }
                fx.push(Effect::Blur);
            }
            Action::FilterChanged(text) => {
                if let Some(c) = self.conn_mut() {
                    c.set_filter(Filter::On(text));
                    fx.push(Effect::ScrollTop);
                }
            }
            Action::Escape => {
                if self.transfers.open {
                    self.transfers.open = false;
                    return;
                }
                if self.conn().is_some_and(|c| c.filter.is_on()) {
                    *fx = self.act(Action::CloseFilter);
                    return;
                }
                if let Some(c) = self.conn_mut() {
                    c.selection.clear();
                }
            }
            _ => {}
        }
    }

    /// A click on the row `name`: Shift (`extend`) selects up to it, ⌘ (`toggle`) adds or
    /// removes it, a plain click selects it alone.
    fn click(&mut self, name: &str, extend: bool, toggle: bool) {
        self.ui.type_select.reset();
        if let Some(c) = self.conn_mut() {
            if extend {
                let rows = c.row_names().into_iter().map(str::to_owned).collect::<Vec<_>>();
                let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
                c.selection.extend_to(&rows, name, toggle);
            } else if toggle {
                c.selection.toggle(name);
            } else {
                c.selection.only(name);
            }
        }
    }

    fn open(&mut self, name: &str, fx: &mut Vec<Effect>) {
        let Some(c) = self.conn() else { return };
        let Some(e) = c.find(name) else { return };
        if e.kind.is_dir_like() {
            let path = c.cwd.join(name);
            self.navigate(path, Nav::Visit, fx);
        } else {
            let names = vec![name.to_owned()];
            self.download(&names);
        }
    }

    fn navigate(&mut self, path: RemotePath, how: Nav, fx: &mut Vec<Effect>) {
        self.navigate_selecting(path, how, None, fx);
    }

    /// [`App::navigate`], selecting the row `select` once the folder lists. Set before the
    /// listing is asked for, since a backend may answer at once.
    pub(super) fn navigate_selecting(&mut self, path: RemotePath, how: Nav, select: Option<&str>, fx: &mut Vec<Effect>) {
        let Some(c) = self.conn_mut() else { return };
        if !c.interactive() || !c.go(path, how) {
            return;
        }
        if let Some(name) = select {
            c.reveal_after(name);
        }
        c.fallback = None;
        let (host, path) = (c.host.clone(), c.cwd.clone());
        self.ui.filter_edit = TextEdit::new("");
        self.ui.type_select.reset();
        fx.push(Effect::ScrollTop);
        fx.push(Effect::Blur);
        self.remember_folder(&host, &path);
        self.send(Command::List { host, path });
        if let Some(cur) = self.conn().and_then(|c| c.selection.cursor().map(str::to_owned)) {
            fx.push(Effect::Reveal(cur));
        }
    }

    fn history(&mut self, how: Nav, fx: &mut Vec<Effect>) {
        let Some(c) = self.conn_mut() else { return };
        if !c.interactive() {
            return;
        }
        let moved = match how {
            Nav::Back => c.back(),
            Nav::Forward => c.forward(),
            Nav::Visit | Nav::Replace => false,
        };
        if moved {
            c.fallback = None;
            let (host, path) = (c.host.clone(), c.cwd.clone());
            self.ui.filter_edit = TextEdit::new("");
            fx.push(Effect::ScrollTop);
            fx.push(Effect::Blur);
            self.remember_folder(&host, &path);
            self.send(Command::List { host, path });
            if let Some(cur) = self.conn().and_then(|c| c.selection.cursor().map(str::to_owned)) {
                fx.push(Effect::Reveal(cur));
            }
        }
    }

    /// Runs a selection change over the rows on screen; a returned name is revealed.
    fn with_rows(&mut self, fx: &mut Vec<Effect>, f: impl FnOnce(&mut Selection, &[&str]) -> Option<String>) {
        let Some(c) = self.conn_mut() else { return };
        if let Some(name) = c.with_row_names(f) {
            fx.push(Effect::Reveal(name));
        }
    }
}
