//! What the UI knows of transfers (§4.6, §6.8): the engine's latest snapshot of every job, and
//! the popover's own state. The engine owns the jobs; this is a mirror, replaced whole by each
//! `ServiceEvent::Transfers`.
//!
//! Each snapshot also drives what shows outside the popover: the taskbar, the Dock badge and
//! notifications (`notify.rs`, [`Watch`]), and the done flourish of a job that just finished
//! with the toolbar ring's pulse ([`Flourishes`], stepped on the window's clock by the app's
//! poll, `views::transfers::poll`).

use std::path::PathBuf;

use dromon_core::{HostId, RemotePath};
use dromon_transfer::{ConflictView, JobId, JobKind, JobRequest, JobView, Progress, RemoteItem, Reveal, TaskState, aggregate};

use super::conn::Nav;
use super::{Action, App, Command, Effect, TransferCommand};

#[path = "notify.rs"]
pub mod notify;

pub use notify::{Bar, Dock, Notice, Os, OsEffect, Watch};

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
    /// The jobs as the OS shows them: taskbar, Dock badge, notifications.
    pub watch: Watch,
    /// Done flourishes playing, and the toolbar ring's pulse.
    pub flourish: Flourishes,
}

/// How long a finished job's bar takes to fill before it collapses into the check (the `move`
/// spring's duration), in seconds.
pub const FILL: f64 = 0.30;
/// How long the toolbar ring's pulse lasts (the `settle` spring's duration), in seconds.
pub const PULSE: f64 = 0.45;

/// The done flourish of a job that finished while the app watched it (§6.6): its bar fills,
/// then collapses into a mint check and the toolbar ring pulses once. A job without one (it
/// finished long ago, or arrived finished) shows the check at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flourish {
    /// It just finished: the bar is still where the last snapshot had it.
    Finished { from: f32 },
    /// The bar fills to the end, until `until` (seconds on the window's clock).
    Filling { until: f64 },
}

impl Flourish {
    /// The phase after this one at `clock`; `None` once the bar has filled: it collapses into
    /// the check.
    pub fn step(self, clock: f64) -> Option<Flourish> {
        match self {
            Flourish::Finished { .. } => Some(Flourish::Filling { until: clock + FILL }),
            Flourish::Filling { until } if clock >= until => None,
            f => Some(f),
        }
    }
}

/// What a step of the flourishes did: whether anything on screen changes (the view runs again,
/// animated), and when the next step is due.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stepped {
    pub changed: bool,
    pub wake: Option<f64>,
}

/// The flourishes playing and the ring's pulse.
#[derive(Clone, Debug, Default)]
pub struct Flourishes {
    jobs: Vec<(JobId, Flourish)>,
    /// The ring pulses until this time (the window's clock).
    pulse: Option<f64>,
    /// Pulses so far: each one draws a new element, so each plays.
    pulses: u32,
}

impl Flourishes {
    /// The flourish `id` is in, if any.
    pub fn get(&self, id: JobId) -> Option<Flourish> {
        self.jobs.iter().find(|(j, _)| *j == id).map(|(_, f)| *f)
    }

    /// Some bar is still filling.
    pub fn filling(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// The pulse playing, numbered.
    pub fn pulse(&self) -> Option<u32> {
        self.pulse.map(|_| self.pulses)
    }

    /// Steps every flourish and the pulse at `clock`: a filled bar collapses and starts the
    /// ring's pulse; a pulse that has run ends.
    pub fn step(&mut self, clock: f64) -> Stepped {
        let mut out = Stepped::default();
        let mut collapsed = false;
        self.jobs.retain_mut(|(_, f)| match f.step(clock) {
            Some(next) => {
                out.changed |= next != *f;
                *f = next;
                true
            }
            None => {
                collapsed = true;
                false
            }
        });
        if collapsed {
            self.pulse = Some(clock + PULSE);
            self.pulses += 1;
            out.changed = true;
        } else if self.pulse.is_some_and(|until| clock >= until) {
            self.pulse = None;
            out.changed = true;
        }
        let dues = self.jobs.iter().map(|(_, f)| match f {
            Flourish::Filling { until } => *until,
            Flourish::Finished { .. } => clock,
        });
        out.wake = dues.chain(self.pulse).reduce(f64::min);
        out
    }

    /// `before` was replaced by `jobs`: a job that was active and is done now starts its
    /// flourish; flourishes of jobs no longer done (or cleared) stop.
    fn update(&mut self, before: &[JobView], jobs: &[JobView]) {
        for j in jobs.iter().filter(|j| j.state == TaskState::Done) {
            if let Some(old) = before.iter().find(|o| o.id == j.id && o.state.is_active()) {
                self.jobs.push((j.id, Flourish::Finished { from: old.progress.fraction() }));
            }
        }
        self.jobs.retain(|(id, _)| jobs.iter().any(|j| j.id == *id && j.state == TaskState::Done));
    }
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
        let now = (self.watch.clock)();
        self.set_at(jobs, now)
    }

    /// [`Transfers::set`] at `now` (milliseconds on [`Watch::clock`]): also times the jobs for
    /// notifications, updates the taskbar and starts the done flourishes.
    pub fn set_at(&mut self, jobs: Vec<JobView>, now: i64) -> Vec<(HostId, RemotePath)> {
        self.watch.update(&self.jobs, &jobs, now);
        self.flourish.update(&self.jobs, &jobs);
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

    /// The fraction the toolbar ring shows: the active jobs' bytes, or (with nothing active)
    /// a full ring while a finished job's bar fills and the ring pulses for it; `None`: no ring.
    pub fn ring_fraction(&self) -> Option<f32> {
        let flourishing = self.flourish.filling() || self.flourish.pulse().is_some();
        self.ring().map(|p| p.fraction()).or(flourishing.then_some(1.0))
    }

    /// The window gained or lost focus (the taskbar's red bar for failures is seen once it
    /// gains it; notifications go out only while it doesn't have it).
    pub fn window_focus(&mut self, focused: bool) {
        self.watch.window_focus(focused, &self.jobs);
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

    /// A click on a transfer's notification: the popover opens on the job's tab (the app has
    /// come to the front for it).
    pub fn show_transfer(&mut self, job: JobId) {
        if let Some(j) = self.transfers.get(job) {
            self.transfers.tab = if j.kind.in_uploads_tab() { Tab::Uploads } else { Tab::Downloads };
        }
        self.transfers.open = true;
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
        let req = JobRequest::Download { host: c.host.clone(), items, into: self.download_folder() };
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
