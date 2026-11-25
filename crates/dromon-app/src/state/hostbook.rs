//! Saved and recent hosts in the app (§3.8, §4.3 "Saved and recent hosts"): Connect and Save
//! from the quick-connect popover (`quick.rs`), the sidebar's host menu, the Add to SSH config
//! sheet, and what the service answers.
//!
//! - **Recent** is the app's: the last [`MAX_RECENT`] quick connects, newest first, one per
//!   `[user@]host:port`, remembered in `prefs.json` ([`super::prefs::RecentHost`]) and handed to
//!   the service ([`HostBookCommand::Recent`]) so it can connect them.
//! - **Saved** is the service's: `hosts.conf`, read at startup and when it changes, written on
//!   the service side; the app shows what [`HostBookEvent::Saved`] says (and a Save or Remove
//!   at once, before the answer).
//! - **SSH Config** is discovery's (`ServiceEvent::Hosts`). A host that leaves a section while
//!   it's connected or on screen stays, so a config edit never loses the connection.
//!
//! The directory never holds two hosts with one id: SSH Config wins over Saved, Saved over
//! Recent (a saved host just added to the ssh config is the config's).

use std::path::PathBuf;

use dromon_core::{HostId, HostSource, HostSpec};
use dromon_ssh::{QuickField, QuickInput, quick_host, same_target};

use super::menu::{MenuEntry, MenuItem};
use super::prefs::RecentHost;
use super::service::Command;
use super::{Action, App, Effect, hosts};

#[cfg(test)]
#[path = "hostbook_tests.rs"]
mod tests;

/// How many recent hosts are kept.
pub const MAX_RECENT: usize = 10;

/// What the UI asks of the service about saved and recent hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostBookCommand {
    /// The Recent section, newest first: the service connects these by id. Replaces its list.
    Recent(Vec<HostSpec>),
    /// Saves `spec` in `hosts.conf`, in place of the saved host `replaces` (Edit…) or of one
    /// with its id; answered with `Saved` (and `SaveFailed` when the file couldn't be written).
    Save { spec: HostSpec, replaces: Option<HostId> },
    /// Takes a saved host out of `hosts.conf`; answered like `Save`.
    Remove(HostId),
    /// What adding the saved host to the user's ssh config would write, and where: answered
    /// with `Preview`.
    PreviewAddToConfig(HostId),
    /// Backs up the user's ssh config, appends the saved host's block, and takes the host out of
    /// `hosts.conf`: answered with `Added` (after `Hosts` and `Saved`), or `AddFailed`.
    AddToConfig(HostId),
}

/// What adding a saved host to the ssh config will do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigPreview {
    /// The ssh config, as the user knows its path (`~/.ssh/config`).
    pub target: String,
    /// Where the copy of it goes; `None`: the file doesn't exist and will be created.
    pub backup: Option<String>,
    /// The exact `Host` block appended.
    pub block: String,
}

/// What the service tells the UI about saved hosts and the ssh config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostBookEvent {
    /// The saved hosts (`hosts.conf`), in order: replaces the Saved section.
    Saved(Vec<HostSpec>),
    /// `hosts.conf` couldn't be written: why. It's as it was (a `Saved` with it follows).
    SaveFailed(String),
    /// The answer to `PreviewAddToConfig`.
    Preview { host: HostId, preview: Result<ConfigPreview, String> },
    /// The host's block is in the ssh config.
    Added { host: HostId },
    /// It isn't: why. The ssh config is as it was.
    AddFailed { host: HostId, error: String },
}

/// A `HostBook` event's trace line (`DROMON_TRACE=1`): its kind, a count or a host id, never
/// paths, blocks or errors.
pub fn trace_line(ev: &HostBookEvent) -> String {
    match ev {
        HostBookEvent::Saved(list) => format!("Saved count={}", list.len()),
        HostBookEvent::SaveFailed(_) => "SaveFailed".to_owned(),
        HostBookEvent::Preview { host, preview } => format!("Preview host={host} ok={}", preview.is_ok()),
        HostBookEvent::Added { host } => format!("Added host={host}"),
        HostBookEvent::AddFailed { host, .. } => format!("AddFailed host={host}"),
    }
}

/// The Add to SSH config sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddToConfigView {
    pub host: HostId,
    /// The host's name, for the title.
    pub name: String,
    /// `None` until the service says what it would do.
    pub preview: Option<ConfigPreview>,
    /// Add to config was pressed: waiting for the service.
    pub busy: bool,
    pub error: Option<String>,
}

/// Quick connect and the host menus (`Action::HostBook`).
#[derive(Clone, Debug, PartialEq)]
pub enum HostAction {
    /// Escape or a press outside the popover.
    CloseQuick,
    ToggleMore,
    /// A field's text changed (its error goes).
    Edited(QuickField),
    /// An address pasted into Host: split across Host, Username and Port.
    PasteHost(String),
    /// The popover's Connect (and Return in its fields).
    Connect,
    /// The popover's Save.
    Save,
    /// The identity file's picker answered: a file, nothing (cancelled), or why it couldn't open.
    PickedIdentity(Result<Option<PathBuf>, String>),
    /// The host menu's Edit… (saved hosts): the popover on its settings.
    Edit(HostId),
    /// Remove (saved and recent hosts).
    Remove(HostId),
    /// A recent host's Save.
    SaveRecent(HostId),
    /// A saved host's Add to SSH config…: the sheet.
    AddToConfig(HostId),
    /// The sheet's Add to config.
    ConfirmAddToConfig,
}

impl RecentHost {
    fn of(h: &HostSpec) -> RecentHost {
        let input = QuickInput::from_spec(h);
        RecentHost {
            alias: input.alias,
            hostname: h.hostname.clone().unwrap_or_else(|| h.alias.clone()),
            user: h.user.clone(),
            port: h.port,
            identity_file: h.identity_file.clone(),
            proxy_jump: h.proxy_jump.clone(),
        }
    }

    /// The host again, checked as typed input; `None` when it no longer passes.
    fn spec(&self) -> Option<HostSpec> {
        let input = QuickInput {
            host: self.hostname.clone(),
            user: self.user.clone().unwrap_or_default(),
            port: self.port.map(|p| p.to_string()).unwrap_or_default(),
            alias: self.alias.clone(),
            identity_file: self.identity_file.clone().unwrap_or_default(),
            proxy_jump: self.proxy_jump.clone().unwrap_or_default(),
        };
        quick_host(&input, HostSource::Recent).ok()
    }
}

/// Puts `spec` in the saved list `list`: where `replaces` (an edit) or a host with its id was,
/// else at the end. The edited host goes if its name changed.
pub fn upsert(list: &mut Vec<HostSpec>, spec: HostSpec, replaces: Option<&HostId>) {
    let id = spec.id.clone();
    match list.iter().position(|h| Some(&h.id) == replaces || h.id == id) {
        Some(i) => list[i] = spec,
        None => list.push(spec),
    }
    let mut seen = false;
    list.retain(|h| {
        if h.id == id {
            // Only the first of its id (an edit renamed onto its own old slot).
            !std::mem::replace(&mut seen, true)
        } else {
            Some(&h.id) != replaces
        }
    });
}

/// `hosts` with one host per id: the first of each in section order (SSH Config, Saved, Recent).
fn one_per_id(hosts: Vec<HostSpec>) -> Vec<HostSpec> {
    let mut out: Vec<HostSpec> = Vec::with_capacity(hosts.len());
    for source in [HostSource::SshConfig, HostSource::Saved, HostSource::Recent] {
        for h in hosts.iter().filter(|h| h.source == source) {
            if !out.iter().any(|o| o.id == h.id) {
                out.push(h.clone());
            }
        }
    }
    out
}

fn item(id: &'static str, title: &'static str, enabled: bool) -> MenuEntry {
    MenuEntry::Item(MenuItem { id, title, enabled, checked: None, danger: false, shortcut: None })
}

impl App {
    /// Whether `host` is connected (or on its way) or on screen: it stays in the sidebar.
    fn in_use(&self, host: &HostId) -> bool {
        self.active.as_ref() == Some(host) || self.conns.get(host).is_some_and(|c| hosts::is_live(&c.status))
    }

    /// Replaces the hosts of `source` with `new`, keeping ones that went but are in use.
    fn replace_section(&mut self, source: HostSource, new: Vec<HostSpec>) {
        let kept: Vec<HostSpec> =
            self.hosts.hosts.iter().filter(|h| h.source == source && !new.iter().any(|n| n.id == h.id) && self.in_use(&h.id)).cloned().collect();
        let mut hosts: Vec<HostSpec> = self.hosts.hosts.drain(..).filter(|h| h.source != source).collect();
        hosts.extend(new.into_iter().filter(|h| h.source == source));
        hosts.extend(kept);
        self.hosts.hosts = one_per_id(hosts);
    }

    /// Discovery's hosts (`ServiceEvent::Hosts`): the SSH Config section.
    pub(super) fn set_config_hosts(&mut self, specs: Vec<HostSpec>) {
        self.replace_section(HostSource::SshConfig, specs);
    }

    /// The Recent section, newest first.
    pub fn recent_hosts(&self) -> Vec<HostSpec> {
        self.hosts.hosts.iter().filter(|h| h.source == HostSource::Recent).cloned().collect()
    }

    /// Makes `list` the Recent section: shown, remembered and handed to the service.
    fn set_recent(&mut self, mut list: Vec<HostSpec>) {
        list.truncate(MAX_RECENT);
        self.replace_section(HostSource::Recent, list.clone());
        self.saved.recent = list.iter().map(RecentHost::of).collect();
        self.save();
        self.send(Command::HostBook(HostBookCommand::Recent(list)));
    }

    /// The Recent section remembered from last time (`remember`), if any.
    pub(super) fn restore_recent(&mut self) {
        if self.saved.recent.is_empty() {
            return;
        }
        let list: Vec<HostSpec> = self.saved.recent.iter().filter_map(RecentHost::spec).collect();
        self.replace_section(HostSource::Recent, list.clone());
        self.send(Command::HostBook(HostBookCommand::Recent(list)));
    }

    /// `spec` becomes the newest recent host, replacing any for the same server.
    fn add_recent(&mut self, spec: HostSpec) {
        let mut list = self.recent_hosts();
        let port = |h: &HostSpec| h.port.unwrap_or(22);
        list.retain(|h| h.id != spec.id && !(h.hostname == spec.hostname && h.user == spec.user && port(h) == port(&spec)));
        list.insert(0, spec);
        self.set_recent(list);
    }

    /// Opens the quick-connect popover (the sidebar's +, the welcome button, ⌘⇧K).
    pub(super) fn open_quick(&mut self) {
        self.files.menu = None;
        self.transfers.open = false;
        self.ui.quick.open_empty();
    }

    /// Quick connect's and the host menus' actions.
    pub(super) fn host_action(&mut self, action: HostAction, _fx: &mut Vec<Effect>) {
        match action {
            HostAction::CloseQuick => self.ui.quick.close(),
            HostAction::ToggleMore => self.ui.quick.toggle_more(),
            HostAction::Edited(f) => self.ui.quick.edited(f),
            HostAction::PasteHost(text) => {
                self.ui.quick.paste_host(&text);
            }
            HostAction::Connect => self.quick_connect(),
            HostAction::Save => self.quick_save(),
            HostAction::PickedIdentity(Ok(Some(path))) => self.ui.quick.picked_identity(&path.to_string_lossy()),
            HostAction::PickedIdentity(Ok(None)) => {}
            HostAction::PickedIdentity(Err(e)) => {
                let msg = format!("No file picker here ({e}): type the path");
                self.ui.quick.fail(vec![(QuickField::IdentityFile, msg)]);
            }
            HostAction::Edit(id) => {
                if let Some(spec) = self.hosts.get(&id).filter(|h| h.source == HostSource::Saved).cloned() {
                    self.files.menu = None;
                    self.ui.quick.open_with(&spec, Some(id));
                }
            }
            HostAction::Remove(id) => self.remove_host(&id),
            HostAction::SaveRecent(id) => self.save_recent(&id),
            HostAction::AddToConfig(id) => {
                let Some(spec) = self.hosts.get(&id).filter(|h| h.source == HostSource::Saved) else { return };
                let view = AddToConfigView { host: id.clone(), name: spec.display_name().to_owned(), preview: None, busy: false, error: None };
                if self.ui.sheets.open_add_to_config(view) {
                    self.send(Command::HostBook(HostBookCommand::PreviewAddToConfig(id)));
                }
            }
            HostAction::ConfirmAddToConfig => {
                let Some(v) = self.ui.sheets.add_to_config_mut() else { return };
                if v.busy || v.preview.is_none() {
                    return;
                }
                v.busy = true;
                v.error = None;
                let host = v.host.clone();
                self.send(Command::HostBook(HostBookCommand::AddToConfig(host)));
            }
        }
    }

    /// Connect: a host the fields describe. One that's saved (the same server, account, key and
    /// jump host, and the same name if one was typed) connects as itself; anything else becomes
    /// the newest recent host.
    fn quick_connect(&mut self) {
        let input = self.ui.quick.input();
        let spec = match quick_host(&input, HostSource::Recent) {
            Ok(spec) => spec,
            Err(errors) => return self.ui.quick.fail(errors),
        };
        let name = input.alias.trim();
        let saved = self
            .hosts
            .hosts
            .iter()
            .find(|h| h.source == HostSource::Saved && same_target(h, &spec) && (name.is_empty() || h.alias == name))
            .map(|h| h.id.clone());
        self.ui.quick.close();
        let id = match saved {
            Some(id) => id,
            None => {
                let id = spec.id.clone();
                self.add_recent(spec);
                id
            }
        };
        self.select_host(id);
    }

    /// Save: a saved host from the fields, unless its name is another host's.
    fn quick_save(&mut self) {
        let input = self.ui.quick.input();
        let spec = match quick_host(&input, HostSource::Saved) {
            Ok(spec) => spec,
            Err(errors) => return self.ui.quick.fail(errors),
        };
        let editing = self.ui.quick.editing.clone();
        if let Some(why) = self.name_taken(&spec, editing.as_ref()) {
            return self.ui.quick.fail(vec![(QuickField::Alias, why)]);
        }
        self.ui.quick.close();
        self.save_host(spec, editing);
    }

    /// Why `spec`'s name can't be used (another SSH Config or saved host has it), if so.
    fn name_taken(&self, spec: &HostSpec, editing: Option<&HostId>) -> Option<String> {
        let other = self.hosts.hosts.iter().find(|h| h.id == spec.id && h.source != HostSource::Recent && Some(&h.id) != editing)?;
        let section = if other.source == HostSource::SshConfig { "your ssh config" } else { "Saved" };
        Some(format!("“{}” is already a host in {section}: give this one another name", spec.alias))
    }

    /// Shows `spec` under Saved at once and asks the service to write it.
    fn save_host(&mut self, spec: HostSpec, replaces: Option<HostId>) {
        let mut saved: Vec<HostSpec> = self.hosts.hosts.iter().filter(|h| h.source == HostSource::Saved).cloned().collect();
        upsert(&mut saved, spec.clone(), replaces.as_ref());
        self.replace_section(HostSource::Saved, saved);
        self.send(Command::HostBook(HostBookCommand::Save { spec, replaces }));
    }

    /// A recent host's Save: saved under its own name, and out of Recent. A name that's taken
    /// opens the popover on it to choose another.
    fn save_recent(&mut self, id: &HostId) {
        let Some(recent) = self.hosts.get(id).filter(|h| h.source == HostSource::Recent).cloned() else { return };
        let Ok(spec) = quick_host(&QuickInput::from_spec(&recent), HostSource::Saved) else { return };
        if let Some(why) = self.name_taken(&spec, None) {
            self.ui.quick.open_with(&spec, None);
            self.ui.quick.fail(vec![(QuickField::Alias, why)]);
            return;
        }
        self.save_host(spec, None);
        if !self.in_use(id) {
            let list = self.recent_hosts().into_iter().filter(|h| &h.id != id).collect();
            self.set_recent(list);
        }
    }

    fn remove_host(&mut self, id: &HostId) {
        match self.hosts.get(id).map(|h| h.source) {
            Some(HostSource::Saved) => {
                let saved = self.hosts.hosts.iter().filter(|h| h.source == HostSource::Saved && &h.id != id).cloned().collect();
                self.replace_section(HostSource::Saved, saved);
                self.send(Command::HostBook(HostBookCommand::Remove(id.clone())));
            }
            Some(HostSource::Recent) => {
                let list = self.recent_hosts().into_iter().filter(|h| &h.id != id).collect();
                self.set_recent(list);
            }
            _ => {}
        }
    }

    /// What the service said about saved hosts and the ssh config.
    pub(super) fn host_event(&mut self, ev: HostBookEvent) {
        match ev {
            HostBookEvent::Saved(list) => self.replace_section(HostSource::Saved, list),
            HostBookEvent::SaveFailed(why) => {
                self.files.toasts.push(format!("Couldn't save your hosts: {why}"));
            }
            HostBookEvent::Preview { host, preview } => {
                if let Some(v) = self.ui.sheets.add_to_config_mut().filter(|v| v.host == host) {
                    match preview {
                        Ok(p) => v.preview = Some(p),
                        Err(e) => v.error = Some(e),
                    }
                }
            }
            HostBookEvent::Added { host } => self.ui.sheets.close_add_to_config(&host),
            HostBookEvent::AddFailed { host, error } => match self.ui.sheets.add_to_config_mut().filter(|v| v.host == host) {
                Some(v) => {
                    v.busy = false;
                    v.error = Some(error);
                }
                None => {
                    let name = self.hosts.get(&host).map_or(host.as_str(), |h| h.display_name()).to_owned();
                    self.files.toasts.push(format!("Couldn't add {name} to your ssh config: {error}"));
                }
            },
        }
    }

    /// The sidebar host menu: Connect (or Disconnect while connected); for saved hosts Add to
    /// SSH config…, Edit… and Remove; for recent ones Save and Remove.
    pub fn host_menu(&self, id: &HostId) -> Vec<MenuEntry> {
        let Some(h) = self.hosts.get(id) else { return Vec::new() };
        let live = self.conns.get(id).is_some_and(|c| hosts::is_live(&c.status));
        let mut out = vec![if live { item("host-disconnect", "Disconnect", true) } else { item("host-connect", "Connect", true) }];
        match h.source {
            HostSource::Saved => out.extend([
                MenuEntry::Separator,
                item("host-add-to-config", "Add to SSH config…", true),
                item("host-edit", "Edit…", true),
                MenuEntry::Separator,
                item("host-remove", "Remove", true),
            ]),
            HostSource::Recent => {
                out.extend([MenuEntry::Separator, item("host-save", "Save", true), item("host-remove", "Remove", true)]);
            }
            HostSource::SshConfig => {}
        }
        out
    }

    /// What picking `item` in `id`'s menu does.
    pub fn host_menu_action(&self, id: &HostId, item: &str) -> Option<Action> {
        self.host_menu(id).iter().filter_map(MenuEntry::item).find(|i| i.id == item && i.enabled)?;
        let id = id.clone();
        Some(match item {
            "host-connect" => Action::SelectHost(id),
            "host-disconnect" => Action::Disconnect(id),
            "host-add-to-config" => Action::HostBook(HostAction::AddToConfig(id)),
            "host-edit" => Action::HostBook(HostAction::Edit(id)),
            "host-remove" => Action::HostBook(HostAction::Remove(id)),
            "host-save" => Action::HostBook(HostAction::SaveRecent(id)),
            _ => return None,
        })
    }
}
