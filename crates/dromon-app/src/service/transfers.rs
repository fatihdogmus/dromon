//! The transfer engine on the service thread (§4.6): one task that owns the pure
//! [`Queue`], hands its [`Action`]s to `dromon_transfer::io` as tokio tasks, and feeds their
//! results back. The connection tasks tell it when a host's SFTP session comes and goes
//! ([`EngineMsg::HostUp`] with the session, [`EngineMsg::HostDown`]); every copy for a host
//! runs over that host's one session.
//!
//! - The UI sees every job through [`ServiceEvent::Transfers`], sent when something changed,
//!   at most every [`EMIT_EVERY`].
//! - A dropped connection aborts the host's running files (their part files stay, and they
//!   resume from them when the host is back); naming and scanning work it cut off is replayed
//!   on the next `HostUp` instead of failing.
//! - Pause and cancel abort a file's task; its part file stays (pause) or is deleted (cancel).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dromon_core::{Error, HostId, RemotePath};
use dromon_sftp::Sftp;
use dromon_transfer::io::{self, CopyOptions};
use dromon_transfer::queue::{Action, ItemDst, Queue};
use dromon_transfer::{FileId, FilePlan, JobId, JobRequest, Scan, unique_name};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::Sink;
use crate::state::{ServiceEvent, TransferCommand};

/// Files running at once per host (§4.6; a setting later).
pub const PER_HOST: usize = 3;
/// The most often the UI hears about transfers.
pub const EMIT_EVERY: Duration = Duration::from_millis(100);

/// What the engine hears from the rest of the service.
pub enum EngineMsg {
    Command(TransferCommand),
    /// The host's SFTP session is up (first time or after a reconnect).
    HostUp(HostId, Sftp),
    /// The host's session is gone.
    HostDown(HostId),
}

/// What a task the engine started found out.
enum Outcome {
    /// Download: the item's free local name (or an error, e.g. the Downloads folder can't be made).
    Named { job: JobId, item: usize, dst: Result<ItemDst, Error> },
    /// Upload: what is at `into/name`.
    Exists { job: JobId, item: usize, existing: Result<Option<bool>, Error> },
    ScanProgress { job: JobId, item: usize, found: u64 },
    Scanned { job: JobId, item: usize, scan: Result<Scan, Error> },
    Progress { job: JobId, file: FileId, done: u64 },
    Finished { job: JobId, file: FileId, result: Result<(), Error> },
}

/// Starts the engine task; send it [`EngineMsg`]s.
pub fn start<S: Sink>(sink: S) -> mpsc::UnboundedSender<EngineMsg> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(Engine::new(sink).run(rx));
    tx
}

struct Engine<S: Sink> {
    queue: Queue,
    /// Each job's request: the item paths and destinations the queue's actions refer to.
    requests: HashMap<JobId, JobRequest>,
    sessions: HashMap<HostId, Sftp>,
    /// Running file copies, by job and file, with their host.
    running: HashMap<(JobId, FileId), (HostId, AbortHandle)>,
    /// Naming and scanning cut off by a dropped connection, run again when the host is back.
    deferred: HashMap<HostId, Vec<Action>>,
    /// Each scan in flight's destination, to run it again after a dropped connection.
    scans: HashMap<(JobId, usize), ItemDst>,
    outcomes: (mpsc::UnboundedSender<Outcome>, mpsc::UnboundedReceiver<Outcome>),
    sink: S,
    started: Instant,
    dirty: bool,
}

impl<S: Sink> Engine<S> {
    fn new(sink: S) -> Self {
        Engine {
            queue: Queue::new(PER_HOST),
            requests: HashMap::new(),
            sessions: HashMap::new(),
            running: HashMap::new(),
            deferred: HashMap::new(),
            scans: HashMap::new(),
            outcomes: mpsc::unbounded_channel(),
            sink,
            started: Instant::now(),
            dirty: false,
        }
    }

    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<EngineMsg>) {
        let mut tick = tokio::time::interval(EMIT_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => self.on_message(msg),
                    // The service is stopping: running copies end with the runtime, leaving
                    // their part files.
                    None => break,
                },
                Some(outcome) = self.outcomes.1.recv() => self.on_outcome(outcome),
                _ = tick.tick() => {
                    if std::mem::take(&mut self.dirty) {
                        self.sink.emit(ServiceEvent::Transfers(self.queue.views(self.now())));
                    }
                    continue;
                }
            }
            self.drive();
            self.dirty = true;
        }
        for (_, (_, handle)) in self.running.drain() {
            handle.abort();
        }
    }

    fn on_message(&mut self, msg: EngineMsg) {
        let now = self.now();
        match msg {
            EngineMsg::Command(cmd) => match cmd {
                TransferCommand::Start(req) => {
                    let id = self.queue.add(req.clone(), now);
                    self.requests.insert(id, req);
                }
                TransferCommand::Pause(job) => {
                    let actions = self.queue.pause(job);
                    self.carry_out(actions);
                }
                TransferCommand::Resume(job) => self.queue.resume(job),
                TransferCommand::Cancel(job) => {
                    let actions = self.queue.cancel(job);
                    self.carry_out(actions);
                }
                TransferCommand::Retry(job) => self.queue.retry(job),
                TransferCommand::Resolve { job, choice, apply_all } => self.queue.resolve(job, choice, apply_all),
                TransferCommand::ClearFinished => {
                    self.queue.clear_finished();
                    let live: Vec<JobId> = self.queue.views(now).iter().map(|v| v.id).collect();
                    self.requests.retain(|id, _| live.contains(id));
                }
            },
            EngineMsg::HostUp(host, sftp) => {
                self.sessions.insert(host.clone(), sftp);
                self.queue.host_up(&host);
                let again = self.deferred.remove(&host).unwrap_or_default();
                self.carry_out(again);
            }
            EngineMsg::HostDown(host) => {
                self.sessions.remove(&host);
                self.queue.host_down(&host);
                let gone: Vec<(JobId, FileId)> =
                    self.running.iter().filter(|(_, (h, _))| *h == host).map(|(k, _)| *k).collect();
                for (job, file) in gone {
                    if let Some((_, handle)) = self.running.remove(&(job, file)) {
                        handle.abort();
                    }
                    self.queue.file_interrupted(job, file);
                }
            }
        }
    }

    fn on_outcome(&mut self, outcome: Outcome) {
        let now = self.now();
        match outcome {
            Outcome::Named { job, item, dst } => match dst {
                Ok(dst) => self.queue.named(job, item, dst),
                Err(e) => self.item_error(job, item, e, Action::Name { job, item }),
            },
            Outcome::Exists { job, item, existing } => match existing {
                Ok(existing) => self.queue.exists(job, item, existing),
                Err(e) => self.item_error(job, item, e, Action::Name { job, item }),
            },
            Outcome::ScanProgress { job, item, found } => self.queue.scan_progress(job, item, found),
            Outcome::Scanned { job, item, scan } => {
                let dst = self.scans.remove(&(job, item));
                match (scan, dst) {
                    (Ok(scan), _) => self.queue.scanned(job, item, scan),
                    // A scan cut off by the connection runs again into the same destination.
                    (Err(e), Some(dst)) => self.item_error(job, item, e, Action::Scan { job, item, dst }),
                    (Err(e), None) => self.queue.item_failed(job, item, e.to_string()),
                }
            }
            Outcome::Progress { job, file, done } => self.queue.file_progress(job, file, done, now),
            Outcome::Finished { job, file, result } => {
                if self.running.remove(&(job, file)).is_none() {
                    // Aborted (paused, cancelled, host down) after it finished: the queue
                    // already moved the file on.
                    if result.is_ok() {
                        self.queue.file_done(job, file, now);
                    }
                    return;
                }
                match result {
                    Ok(()) => self.queue.file_done(job, file, now),
                    Err(Error::ConnectionLost) => self.queue.file_interrupted(job, file),
                    Err(e) => self.queue.file_failed(job, file, e.to_string(), now),
                }
            }
        }
    }

    /// A naming or scanning error: a lost connection defers the work until the host is back;
    /// anything else fails the item.
    fn item_error(&mut self, job: JobId, item: usize, error: Error, again: Action) {
        match (error, self.requests.get(&job)) {
            (Error::ConnectionLost, Some(req)) => self.deferred.entry(req.host().clone()).or_default().push(again),
            (error, _) => self.queue.item_failed(job, item, error.to_string()),
        }
    }

    /// Polls the queue and carries out what it hands back, until it has nothing more.
    fn drive(&mut self) {
        loop {
            let actions = self.queue.poll(self.now());
            if actions.is_empty() {
                break;
            }
            self.carry_out(actions);
        }
    }

    fn carry_out(&mut self, actions: Vec<Action>) {
        for action in actions {
            self.carry_out_one(action);
        }
    }

    fn carry_out_one(&mut self, action: Action) {
        let tx = self.outcomes.0.clone();
        match action {
            Action::Name { job, item } => {
                let Some(req) = self.requests.get(&job).cloned() else { return };
                match &req {
                    JobRequest::Download { items, into, .. } => {
                        let name = items.get(item).and_then(|i| i.path.file_name()).unwrap_or("download").to_owned();
                        let dst = self.free_local_name(into, &name).map(ItemDst::Local);
                        let _ = tx.send(Outcome::Named { job, item, dst });
                    }
                    JobRequest::Upload { host, items, into } => {
                        let Some(sftp) = self.session(host, Action::Name { job, item }) else { return };
                        let Some(name) = items.get(item).and_then(|p| p.file_name()) else {
                            self.queue.item_failed(job, item, "This item has no name".into());
                            return;
                        };
                        let path = into.join(&name.to_string_lossy());
                        tokio::spawn(async move {
                            let existing = io::remote_kind(&sftp, &path).await;
                            let _ = tx.send(Outcome::Exists { job, item, existing });
                        });
                    }
                }
            }
            Action::FreeRemoteName { job, item } => {
                let Some(JobRequest::Upload { host, items, into }) = self.requests.get(&job).cloned() else { return };
                let Some(name) = items.get(item).and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()) else {
                    return;
                };
                let Some(sftp) = self.session(&host, Action::FreeRemoteName { job, item }) else { return };
                let into = into.clone();
                tokio::spawn(async move {
                    let dst = io::free_remote_name(&sftp, &into, &name)
                        .await
                        .map(|free| ItemDst::Remote { path: into.join(&free), overwrite: false });
                    let _ = tx.send(Outcome::Named { job, item, dst });
                });
            }
            Action::Scan { job, item, dst } => {
                let Some(req) = self.requests.get(&job) else { return };
                let host = req.host().clone();
                let src = match req {
                    JobRequest::Download { items, .. } => items.get(item).map(|i| Src::Remote(i.path.clone())),
                    JobRequest::Upload { items, .. } => items.get(item).map(|p| Src::Local(p.clone())),
                };
                let Some(src) = src else { return };
                let Some(sftp) = self.session(&host, Action::Scan { job, item, dst: dst.clone() }) else { return };
                self.scans.insert((job, item), dst.clone());
                tokio::spawn(async move {
                    let progress = |found| {
                        let _ = tx.send(Outcome::ScanProgress { job, item, found });
                    };
                    let scan = scan(&sftp, src, dst, progress).await;
                    let _ = tx.send(Outcome::Scanned { job, item, scan });
                });
            }
            Action::Start { job, file, plan } => {
                let Some(host) = self.requests.get(&job).map(|r| r.host().clone()) else { return };
                let Some(sftp) = self.sessions.get(&host).cloned() else {
                    // The host went down between poll and now: back to pending.
                    self.queue.file_interrupted(job, file);
                    return;
                };
                let task = tokio::spawn(async move {
                    let progress = |done| {
                        let _ = tx.send(Outcome::Progress { job, file, done });
                    };
                    let result = match &plan {
                        FilePlan::Download { .. } => io::download(&sftp, &plan, CopyOptions::default(), progress).await.map(drop),
                        FilePlan::Upload { .. } => io::upload(&sftp, &plan, CopyOptions::default(), progress).await.map(drop),
                    };
                    let _ = tx.send(Outcome::Finished { job, file, result });
                });
                self.running.insert((job, file), (host, task.abort_handle()));
            }
            Action::Abort { job, file } => {
                if let Some((_, handle)) = self.running.remove(&(job, file)) {
                    handle.abort();
                }
            }
            Action::Cleanup { job, plans } => {
                let sftp = self.requests.get(&job).and_then(|r| self.sessions.get(r.host())).cloned();
                tokio::spawn(async move {
                    for plan in &plans {
                        match (&plan, &sftp) {
                            (_, Some(sftp)) => io::remove_part(sftp, plan).await,
                            // No session: a download's part file is local and can still go.
                            (FilePlan::Download { dst, .. }, None) => remove_local_part(dst),
                            (FilePlan::Upload { .. }, None) => {}
                        }
                    }
                });
            }
        }
    }

    /// The host's session, or `None` with `again` deferred until it is back.
    fn session(&mut self, host: &HostId, again: Action) -> Option<Sftp> {
        match self.sessions.get(host) {
            Some(sftp) => Some(sftp.clone()),
            None => {
                self.deferred.entry(host.clone()).or_default().push(again);
                None
            }
        }
    }

    /// A free name for `name` in local folder `into` (made if missing): not on disk, not a part
    /// file there, not held by another download. A few `stat`s on the service thread.
    fn free_local_name(&self, into: &Path, name: &str) -> Result<PathBuf, Error> {
        std::fs::create_dir_all(into).map_err(|e| Error::Io(format!("Couldn't create {}: {e}", into.display())))?;
        let free = unique_name(name, |n| io::local_name_taken(into, n) || self.queue.is_reserved_local(&into.join(n)));
        Ok(into.join(free))
    }
}

/// Where a scan starts.
enum Src {
    Remote(RemotePath),
    Local(PathBuf),
}

/// Scans `src` into `dst` and creates the folders the scan found.
async fn scan(sftp: &Sftp, src: Src, dst: ItemDst, found: impl FnMut(u64) + Send) -> Result<Scan, Error> {
    let scan = match (src, dst) {
        (Src::Remote(src), ItemDst::Local(dst)) => io::scan_remote(sftp, &src, &dst, found).await?,
        (Src::Local(src), ItemDst::Remote { path, overwrite }) => io::scan_local(&src, &path, overwrite, found).await?,
        _ => return Err(Error::Other("A transfer's source and destination don't match".into())),
    };
    io::create_dirs(sftp, &scan.dirs).await?;
    Ok(scan)
}

fn remove_local_part(dst: &Path) {
    if let Some(name) = dst.file_name() {
        let _ = std::fs::remove_file(dst.with_file_name(dromon_transfer::download_part_name(&name.to_string_lossy())));
    }
}
