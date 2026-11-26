//! The quick-connect popover's form (§3.8, §6.8 "Quick connect"): Host (mono), Username and
//! Port, and under **More** a name, an identity file and a jump host. Pasting an address
//! (`user@host:port`, `host:port`, `[v6]:port`) into Host splits it across the three fields.
//! Every field is checked by `dromon_ssh::quick_host` before anything is sent; problems show
//! under their fields, the first one gets the keyboard and the popover shakes once.
//!
//! The app's side of Connect, Save and the host menus is `hostbook.rs`.

use std::collections::BTreeMap;

use dromon_core::{HostId, HostSpec};
use dromon_ssh::{QuickField, QuickInput, split_address};
use rune::TextEdit;

#[cfg(test)]
#[path = "quick_tests.rs"]
mod tests;

/// Every field, in the order they show (and errors are reported).
pub const FIELDS: [QuickField; 6] = [
    QuickField::Host,
    QuickField::User,
    QuickField::Port,
    QuickField::Alias,
    QuickField::IdentityFile,
    QuickField::ProxyJump,
];

/// The fields under More.
pub fn under_more(f: QuickField) -> bool {
    matches!(f, QuickField::Alias | QuickField::IdentityFile | QuickField::ProxyJump)
}

/// The popover and what's typed into it.
#[derive(Clone, Debug, Default)]
pub struct QuickConnect {
    pub open: bool,
    /// More is expanded.
    pub more: bool,
    /// Edit… of this saved host: Save replaces it.
    pub editing: Option<HostId>,
    host: TextEdit,
    user: TextEdit,
    port: TextEdit,
    alias: TextEdit,
    identity: TextEdit,
    jump: TextEdit,
    /// What's wrong with each field, shown under it.
    pub errors: BTreeMap<QuickField, String>,
    /// How many times a Connect or Save was refused: each one shakes the popover once more.
    pub shakes: u32,
}

impl QuickConnect {
    pub fn edit(&self, f: QuickField) -> &TextEdit {
        match f {
            QuickField::Host => &self.host,
            QuickField::User => &self.user,
            QuickField::Port => &self.port,
            QuickField::Alias => &self.alias,
            QuickField::IdentityFile => &self.identity,
            QuickField::ProxyJump => &self.jump,
        }
    }

    /// A field's text model, for `text_input(..).on_edit`.
    pub fn edit_mut(&mut self, f: QuickField) -> &mut TextEdit {
        match f {
            QuickField::Host => &mut self.host,
            QuickField::User => &mut self.user,
            QuickField::Port => &mut self.port,
            QuickField::Alias => &mut self.alias,
            QuickField::IdentityFile => &mut self.identity,
            QuickField::ProxyJump => &mut self.jump,
        }
    }

    /// Opens it empty, Host focused. Already open, it stays as it is.
    pub fn open_empty(&mut self) {
        if self.open {
            return;
        }
        *self = QuickConnect { open: true, ..QuickConnect::default() };
        self.focus(QuickField::Host);
    }

    /// Opens it on `spec`'s settings (Edit…, or Save of a recent host to name it first); Save
    /// replaces `editing`. More shows when any of its fields has something.
    pub fn open_with(&mut self, spec: &HostSpec, editing: Option<HostId>) {
        let input = QuickInput::from_spec(spec);
        *self = QuickConnect { open: true, editing, ..QuickConnect::default() };
        self.set_input(&input);
        self.more = FIELDS.iter().any(|f| under_more(*f) && !self.edit(*f).text.is_empty());
        self.focus(QuickField::Host);
    }

    /// Closes it; what was typed goes.
    pub fn close(&mut self) {
        *self = QuickConnect::default();
    }

    /// What the fields hold now.
    pub fn input(&self) -> QuickInput {
        let t = |f| self.edit(f).text.clone();
        QuickInput {
            host: t(QuickField::Host),
            user: t(QuickField::User),
            port: t(QuickField::Port),
            alias: t(QuickField::Alias),
            identity_file: t(QuickField::IdentityFile),
            proxy_jump: t(QuickField::ProxyJump),
        }
    }

    fn set_input(&mut self, i: &QuickInput) {
        for (f, text) in [
            (QuickField::Host, &i.host),
            (QuickField::User, &i.user),
            (QuickField::Port, &i.port),
            (QuickField::Alias, &i.alias),
            (QuickField::IdentityFile, &i.identity_file),
            (QuickField::ProxyJump, &i.proxy_jump),
        ] {
            self.set_text(f, text);
        }
    }

    /// Replaces a field's text, caret at its end; its error goes.
    pub fn set_text(&mut self, f: QuickField, text: &str) {
        let focused = self.edit(f).focused;
        let mut edit = TextEdit::new(text);
        edit.focused = focused;
        *self.edit_mut(f) = edit;
        self.errors.remove(&f);
    }

    /// Gives `f` the keyboard (and takes it from the others).
    pub fn focus(&mut self, f: QuickField) {
        for g in FIELDS {
            self.edit_mut(g).focused = g == f;
        }
    }

    /// Whether one of its fields has the keyboard.
    pub fn focused(&self) -> bool {
        self.open && FIELDS.iter().any(|f| self.edit(*f).focused)
    }

    /// `text` pasted into Host: an address splits across Host, Username and Port (true);
    /// anything else is the field's own paste (false).
    pub fn paste_host(&mut self, text: &str) -> bool {
        let Some(a) = split_address(text) else { return false };
        self.set_text(QuickField::Host, &a.host);
        if let Some(u) = a.user {
            self.set_text(QuickField::User, &u);
        }
        if let Some(p) = a.port {
            self.set_text(QuickField::Port, &p);
        }
        true
    }

    /// A field's text changed: its error goes.
    pub fn edited(&mut self, f: QuickField) {
        self.errors.remove(&f);
    }

    /// Shows `errors` under their fields: More opens if one is there, the first one gets the
    /// keyboard, and the popover shakes.
    pub fn fail(&mut self, errors: Vec<(QuickField, String)>) {
        self.errors = errors.into_iter().collect();
        if self.errors.keys().any(|f| under_more(*f)) {
            self.more = true;
        }
        if let Some(first) = self.errors.keys().next().copied() {
            self.focus(first);
        }
        self.shakes += 1;
    }

    /// The identity file's picker chose `path`.
    pub fn picked_identity(&mut self, path: &str) {
        self.more = true;
        self.set_text(QuickField::IdentityFile, path);
    }

    pub fn toggle_more(&mut self) {
        self.more = !self.more;
        // A field that's going away gives the keyboard back to Host.
        if !self.more && FIELDS.iter().any(|f| under_more(*f) && self.edit(*f).focused) {
            self.focus(QuickField::Host);
        }
    }
}
