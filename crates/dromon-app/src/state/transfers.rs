//! What the UI knows of transfers (§4.6, §6.8): the engine's latest snapshot of every job, and
//! the popover's own state. The engine owns the jobs; this is a mirror, replaced whole by each
//! `ServiceEvent::Transfers`.

use std::path::PathBuf;

use dromon_core::{HostId, RemotePath};
use dromon_transfer::{ConflictView, JobId, JobKind, JobRequest, JobView, Progress, RemoteItem, Reveal, aggregate};

use super::conn::Nav;
use super::{Action, App, Command, Effect, TransferCommand};

#[cfg(test)]
#[path = "transfers_tests.rs"]
mod tests;

/// The popover's tabs: Downloads, and Uploads (which also lists remote copies and moves).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Downloads,
    Uploads,
}

impl Tab {
    pub fn holds(self, kind: JobKind) -> bool {
        match self {
            Tab::Downloads => !kind.in_uploads_tab(),
            Tab::Uploads => kind.in_uploads_tab(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Transfers {
    /// Newest first, as the engine sends them.
    pub jobs: Vec<JobView>,
    /// The transfers popover is open.
    pub open: bool,
    pub tab: Tab,
    /// The conflict sheet's "Apply to all" checkbox.
    pub apply_all: bool,
    /// Files from another app are being dragged over a place that takes them (§3.6).
    pub drop: Option<DropHover>,
}

/// Where a file drag would upload to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// The list area: the folder on screen.
    List,
    /// A folder row: that subfolder.
    Folder(String),
}

/// A file drag over a drop target: the outline, the row lift and the pill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropHover {
    pub target: DropTarget,
    /// How many items the drag carries.
    pub count: usize,
}

impl Transfers {
    /// A new snapshot from the engine. The open tab follows the newest job when the popover is
    /// closed, so opening it shows what just started. Returns the remote folders of the jobs
    /// that ended with it (finished, failed or cancelled since the last snapshot, or new and
    /// already over), to be listed again.
    pub fn set(&mut self, jobs: Vec<JobView>) -> Vec<(HostId, RemotePath)> {
        if !self.open
            && let Some(newest) = jobs.first()
            && self.jobs.first().map(|j| j.id) != Some(newest.id)
        {
            self.tab = if newest.kind.in_uploads_tab() { Tab::Uploads } else { Tab::Downloads };
        }
        let mut ended: Vec<(HostId, RemotePath)> = Vec::new();
        for job in jobs.iter().filter(|j| !j.state.is_active()) {
            let was_active = self.jobs.iter().find(|o| o.id == job.id).is_none_or(|o| o.state.is_active());
            if was_active {
                for f in &job.folders {
                    if !ended.contains(f) {
                        ended.push(f.clone());
                    }
                }
            }
        }
        self.jobs = jobs;
        ended
    }

    /// Jobs still queued, preparing, running or paused.
    pub fn active(&self) -> usize {
        self.jobs.iter().filter(|j| j.state.is_active()).count()
    }

    /// The toolbar ring: bytes over every active job, `None` when nothing is active.
    pub fn ring(&self) -> Option<Progress> {
        let active: Vec<&JobView> = self.jobs.iter().filter(|j| j.state.is_active()).collect();
        if active.is_empty() {
            return None;
        }
        Some(aggregate(active.iter().map(|j| &j.progress)))
    }

    /// The name clash to ask about: the oldest job that has one.
    pub fn conflict(&self) -> Option<(JobId, &ConflictView)> {
        self.jobs.iter().rev().find_map(|j| j.conflict.as_ref().map(|c| (j.id, c)))
    }

    /// The jobs of a tab, newest first.
    pub fn in_tab(&self, tab: Tab) -> impl Iterator<Item = &JobView> {
        self.jobs.iter().filter(move |j| tab.holds(j.kind))
    }

    pub fn get(&self, id: JobId) -> Option<&JobView> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Whether any finished job could be cleared.
    pub fn has_finished(&self) -> bool {
        self.jobs.iter().any(|j| j.state.is_finished())
    }
}

impl App {
    /// The transfer actions of [`App::act`].
    pub(super) fn transfer_action(&mut self, action: Action, fx: &mut Vec<Effect>) {
        match action {
            Action::Download => {
                let names = self.picked_rows();
                self.download(&names);
            }
            Action::Upload { paths, folder } => {
                self.transfers.drop = None;
                self.upload(paths, folder);
            }
            Action::DropHover { target, over, count } => self.drop_hover(target, over, count),
            Action::Transfer(cmd) => self.send(Command::Transfer(cmd)),
            Action::ResolveConflict(choice) => {
                if let Some((job, _)) = self.transfers.conflict() {
                    let apply_all = std::mem::take(&mut self.transfers.apply_all);
                    self.send(Command::Transfer(TransferCommand::Resolve { job, choice, apply_all }));
                }
            }
            Action::ToggleApplyAll => self.transfers.apply_all = !self.transfers.apply_all,
            Action::ToggleTransfers => self.transfers.open = !self.transfers.open,
            Action::TransfersTab(tab) => self.transfers.tab = tab,
            Action::RevealTransfer(id) => match self.transfers.get(id).and_then(|j| j.reveal.clone()) {
                Some(Reveal::Local(path)) => fx.push(Effect::ShowInFolder(path)),
                Some(Reveal::Remote(host, path)) => self.reveal_remote(host, path, fx),
                None => {}
            },
            _ => {}
        }
    }

    /// The name clash the conflict sheet asks about: [`Transfers::conflict`] while no prompt
    /// sheet is open (prompts come first) and the transfers popover is closed (its row says the
    /// job waits, and offers to answer: that closes the popover and the sheet comes up).
    pub fn conflict_on_screen(&self) -> Option<(JobId, &ConflictView)> {
        if self.ui.sheets.is_open() || self.transfers.open { None } else { self.transfers.conflict() }
    }

    /// A file drag came onto `target` (`over`) or left it. A drag only shows where it would
    /// upload while the connection takes uploads; leaving a target that isn't the current one
    /// (the drag already moved on to another) changes nothing.
    fn drop_hover(&mut self, target: DropTarget, over: bool, count: usize) {
        if over {
            let live = self.conn().is_some_and(|c| c.interactive());
            let folder_ok = match &target {
                DropTarget::List => true,
                DropTarget::Folder(name) => self.conn().and_then(|c| c.find(name)).is_some_and(|e| e.kind.is_dir_like()),
            };
            self.transfers.drop = (live && folder_ok && count > 0).then_some(DropHover { target, count });
        } else if self.transfers.drop.as_ref().is_some_and(|d| d.target == target) {
            self.transfers.drop = None;
        }
    }

    /// The selected rows on screen, in order; the cursor's row when nothing is selected.
    fn picked_rows(&self) -> Vec<String> {
        let Some(c) = self.conn() else { return Vec::new() };
        let selected: Vec<String> = c.rows().filter(|e| c.selection.contains(&e.name)).map(|e| e.name.clone()).collect();
        if selected.is_empty() { c.selection.cursor().map(str::to_owned).into_iter().collect() } else { selected }
    }

    /// Downloads these rows of the folder on screen into [`App::downloads`].
    pub(super) fn download(&mut self, names: &[String]) {
        let Some(c) = self.conn().filter(|c| c.interactive()) else { return };
        let items: Vec<RemoteItem> = names
            .iter()
            .filter_map(|n| c.find(n))
            .map(|e| RemoteItem { path: c.cwd.join(&e.name), kind: e.kind, size: e.size })
            .collect();
        if items.is_empty() {
            return;
        }
        let req = JobRequest::Download { host: c.host.clone(), items, into: self.downloads.clone() };
        self.send(Command::Transfer(TransferCommand::Start(req)));
    }

    /// Uploads local files into the folder on screen, or into its subfolder `folder`.
    fn upload(&mut self, paths: Vec<PathBuf>, folder: Option<String>) {
        let Some(c) = self.conn().filter(|c| c.interactive()) else { return };
        let into = match folder {
            Some(name) => match c.find(&name) {
                Some(e) if e.kind.is_dir_like() => c.cwd.join(&name),
                _ => return,
            },
            None => c.cwd.clone(),
        };
        if paths.is_empty() {
            return;
        }
        let req = JobRequest::Upload { host: c.host.clone(), items: paths, into };
        self.send(Command::Transfer(TransferCommand::Start(req)));
    }

    /// Shows an uploaded item: its host on screen, its folder listed, the item selected. Only
    /// for a host that is connected (revealing never starts a connection).
    fn reveal_remote(&mut self, host: HostId, path: RemotePath, fx: &mut Vec<Effect>) {
        let (Some(folder), Some(name)) = (path.parent(), path.file_name().map(str::to_owned)) else { return };
        if !self.conns.get(&host).is_some_and(|c| c.interactive()) {
            return;
        }
        if self.active.as_ref() != Some(&host) {
            self.select_host(host);
        }
        self.transfers.open = false;
        let Some(c) = self.conn_mut() else { return };
        if c.cwd == folder {
            if c.find(&name).is_some() {
                c.selection.only(&name);
                fx.push(Effect::Reveal(name));
            }
            return;
        }
        self.navigate_selecting(folder, Nav::Visit, Some(&name), fx);
    }
}
