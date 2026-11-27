//! Edit in a local editor on the service thread (`Command::RemoteEdit`, `state/remote_edit.rs`):
//! the local copies, their watch, and their uploads.
//!
//! - **The editing folder** ([`Editing`]): `Config::editing_root`, the app's cache folder's
//!   `editing` ([`default_root`]) or a test's own; without one, editing isn't available. Each
//!   run of the service has a folder of its own in it, `<run>/`, held by a lock on `<run>.lock`
//!   while it lives; each edit has a folder in it, named after its host and a hash of its host and
//!   path ([`session_dir`]), holding the copy under the file's own name ([`local_name`]), so
//!   the editor shows it. Folders are `0700`, copies `0600` on Unix (on Windows the user's
//!   cache folder is private already). An edit's folder goes when it stops, the run's when the
//!   service stops (quitting), and at start [`sweep`] removes what runs that are gone left
//!   behind (a crash, or a quit that ended before the service could clean up).
//! - **The watch**: every [`WATCH_EVERY`] each copy's modification time and size are looked at;
//!   a change counts as a save once it has held still for a whole look ([`Watch`]), so an
//!   editor that writes a temporary file and renames it over the copy saves once. All file I/O
//!   is on blocking threads.
//! - **Saving**: before each upload the server's copy is looked at (`lstat`); when its size or
//!   time changed since the edit last downloaded or uploaded it, nothing is uploaded and the UI
//!   asks ([`RemoteEditEvent::Conflict`]). Downloads and uploads are the transfer engine's own
//!   (`dromon_transfer::io`): part files and an atomic rename, so a half-written upload never
//!   replaces the server's copy. They run quietly, outside the transfers list: the Editing
//!   section shows them. A save cut off by a lost connection is uploaded again once the session
//!   is back.
//!
//! Each host's task owns its edits ([`Edits`], in `conn.rs`): one tokio task per edit, told of
//! the host's session as it comes and goes. Disconnecting stops them all. Nothing about what a
//! file holds is logged or sent; events carry ids, paths and sizes.

use std::collections::HashMap;
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dromon_core::{Error, HostId, RemotePath};
use dromon_sftp::Sftp;
use dromon_transfer::io::{self, CopyOptions};
use dromon_transfer::{FilePlan, Side};
use tokio::sync::{OnceCell, mpsc, watch};

use super::Sink;
use super::conn::{ConnCmd, ConnHandle};
use crate::state::{EditChoice, EditId, EditStage, RemoteEditCommand, RemoteEditEvent, ServiceEvent};

#[cfg(test)]
#[path = "remote_edit_tests.rs"]
mod tests;

/// How often the local copies are looked at.
pub const WATCH_EVERY: Duration = Duration::from_secs(1);
/// The folder in an edit's folder that Discard my changes downloads into before the copy is
/// replaced.
const STAGING: &str = ".dromon-reload";

/// Where the local copies go: `editing` in the app's cache folder; `None` when the system has
/// no folders for the app (then editing isn't available, rather than copies landing somewhere
/// shared).
pub fn default_root() -> Option<PathBuf> {
    rune::dirs::AppDirs::new(crate::persist::APP_ID).map(|d| d.cache().join("editing"))
}

fn local_err(verb: &str, path: &Path, e: &std::io::Error) -> Error {
    Error::Io(format!("Couldn't {verb} {}: {e}", path.display()))
}

/// Runs `f` on a blocking thread (file I/O).
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    tokio::task::spawn_blocking(f).await.map_err(|e| Error::Other(format!("A file task stopped: {e}")))?
}

/// Makes the folder `dir` (and what's missing above it), for the user alone (`0700` on Unix).
/// Something else in its place, a link included, is refused.
pub fn private_dir(dir: &Path) -> Result<(), Error> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(|e| local_err("create", dir, &e))?;
    let meta = std::fs::symlink_metadata(dir).map_err(|e| local_err("read", dir, &e))?;
    if !meta.is_dir() {
        return Err(Error::Io(format!("Couldn't use {}: it isn't a folder", dir.display())));
    }
    restrict(dir, 0o700)
}

/// Makes `path` the user's alone: `mode` on Unix. On Windows the user's cache folder is private
/// already, and what's made in it inherits that.
#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| local_err("protect", path, &e))
}

#[cfg(not(unix))]
fn restrict(_: &Path, _: u32) -> Result<(), Error> {
    Ok(())
}

/// A name for this run's folder: unique among the runs of every process.
fn run_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{}-{nanos:x}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The lock file of the run folder `dir`: `<dir>.lock` beside it.
fn lock_of(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    dir.with_file_name(name)
}

/// Whether no living run holds the lock file `lock`: it can be locked now (it's released when
/// the lock is dropped).
fn abandoned(lock: &Path) -> bool {
    File::options().read(true).write(true).open(lock).is_ok_and(|f| f.try_lock().is_ok())
}

/// Removes what runs that are gone left in `root`: a run's folder whose lock file nobody holds
/// (or which has none), and a lock file without its folder that nobody holds. A living run's
/// are left alone, another Dromon's too. Best effort: what can't be removed stays.
pub fn sweep(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            let lock = lock_of(&path);
            if !lock.exists() || abandoned(&lock) {
                let _ = std::fs::remove_dir_all(&path);
                let _ = std::fs::remove_file(&lock);
            }
        } else if let Some(run) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".lock"))
            && !root.join(run).exists()
            && abandoned(&path)
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// This run's folder under the root, held while it lives.
#[derive(Debug)]
pub struct Run {
    pub dir: PathBuf,
    lock: PathBuf,
    /// The lock on `lock`, released with the file.
    _held: File,
}

impl Run {
    /// Makes a fresh run folder in `root` (made private first), lock file first, so a sweep
    /// never takes the folder for a leftover.
    pub fn open(root: &Path) -> Result<Run, Error> {
        private_dir(root)?;
        let dir = root.join(run_name());
        let lock = lock_of(&dir);
        let held = File::create(&lock).map_err(|e| local_err("create", &lock, &e))?;
        held.try_lock().map_err(|e| Error::Io(format!("Couldn't lock {}: {e}", lock.display())))?;
        restrict(&lock, 0o600)?;
        private_dir(&dir)?;
        Ok(Run { dir, lock, _held: held })
    }

    /// Removes the run's folder and everything in it, then its lock file.
    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_file(&self.lock);
    }
}

/// The editing folder of one service: its root, and this run's folder in it, made at the first
/// edit.
#[derive(Debug)]
pub struct Editing {
    root: PathBuf,
    run: OnceCell<Arc<Run>>,
}

impl Editing {
    pub fn new(root: PathBuf) -> Editing {
        Editing { root, run: OnceCell::new() }
    }

    /// Removes what earlier runs that are gone left (`sweep`), on a blocking thread.
    pub fn sweep_later(&self) {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || sweep(&root));
    }

    /// This run's folder, made now if it isn't yet (a failure is tried again next time).
    async fn run(&self) -> Result<Arc<Run>, Error> {
        let root = self.root.clone();
        let run = self.run.get_or_try_init(|| async move { blocking(move || Run::open(&root)).await.map(Arc::new) });
        run.await.cloned()
    }

    /// The service stops (the app quits): this run's folder goes, every copy in it.
    pub fn clear(&self) {
        if let Some(run) = self.run.get() {
            run.clear();
        }
    }
}

/// The folder of the edit of `path` on `host` in the run folder `run`: the host's name (made
/// safe for a file name) and a hash of the host and the path, so no two hosts or paths share
/// one.
pub fn session_dir(run: &Path, host: &HostId, path: &RemotePath) -> PathBuf {
    let mut h = DefaultHasher::new();
    (host.as_str(), path.as_str()).hash(&mut h);
    let slug: String = host
        .as_str()
        .chars()
        .take(40)
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    run.join(format!("{slug}-{:016x}", h.finish()))
}

/// The local copy's file name: the remote file's own, so the editor shows it. What no local
/// name can hold (`/`, NUL) becomes `_`, and on Windows (`windows`) what it refuses too:
/// `<>:"\|?*` and control characters, a trailing dot or space, a device name (`CON`, `nul.txt`).
pub fn local_name(name: &str, windows: bool) -> String {
    let refused = |c: char| c == '/' || c == '\0' || (windows && (c.is_control() || "<>:\"\\|?*".contains(c)));
    let mut out: String = name.chars().map(|c| if refused(c) { '_' } else { c }).collect();
    if windows {
        if out.ends_with(['.', ' ']) {
            out.pop();
            out.push('_');
        }
        let stem = out.split('.').next().unwrap_or_default().to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.ends_with(|c: char| c.is_ascii_digit()));
        if device {
            out.insert(0, '_');
        }
    }
    if out.is_empty() || out == "." || out == ".." { "file".to_owned() } else { out }
}

/// A local copy's modification time and size; `None` while it isn't there (an editor between
/// removing it and renaming its new file into place).
pub type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path).ok().map(|m| (m.modified().unwrap_or(UNIX_EPOCH), m.len()))
}

/// When a local copy was saved (pure: its owner looks every [`WATCH_EVERY`] and passes what it
/// saw). A save counts once the copy has changed from what was last uploaded or downloaded and
/// then held still for a whole look, so a write in several steps (an editor's temporary file
/// renamed over the copy, a truncate then a write) counts once, and only once it's done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    /// What was last uploaded, tried, or downloaded.
    done: Stamp,
    /// What the last look saw.
    seen: Stamp,
}

impl Watch {
    /// The copy is `at` now and that's known: only a change from it is a save.
    pub fn settle(&mut self, at: Stamp) {
        *self = Watch { done: at, seen: at };
    }

    /// A look saw `now`: whether it's a save (then it's what was tried, whether the upload works
    /// out or not, so it counts once).
    pub fn look(&mut self, now: Stamp) -> bool {
        let before = std::mem::replace(&mut self.seen, now);
        let save = now.is_some() && now == before && now != self.done;
        if save {
            self.done = now;
        }
        save
    }
}

/// What the host's task tells an edit.
#[derive(Debug)]
enum EditCmd {
    Stop,
    Resolve(EditChoice),
    Retry,
}

/// A command for `cmd`'s host: its task carries it out; a host without one isn't connected, and
/// a `Start` fails at once (anything else: the edit is gone).
pub(super) fn route<S: Sink>(cmd: RemoteEditCommand, conns: &HashMap<HostId, ConnHandle>, sink: &S) {
    let (id, host, start) = match &cmd {
        RemoteEditCommand::Start { id, host, .. } => (*id, host.clone(), true),
        RemoteEditCommand::Stop { id, host }
        | RemoteEditCommand::Resolve { id, host, .. }
        | RemoteEditCommand::Retry { id, host } => (*id, host.clone(), false),
    };
    let unsent = match conns.get(&host) {
        Some(c) => c.cmds.send(ConnCmd::RemoteEdit(cmd)).is_err(),
        None => true,
    };
    if unsent {
        let ev = if start {
            RemoteEditEvent::Failed { id, host, stage: EditStage::Download, error: Error::ConnectionLost }
        } else {
            RemoteEditEvent::Closed { id, host }
        };
        sink.emit(ServiceEvent::RemoteEdit(ev));
    }
}

/// One host's edits, owned by its task (`conn.rs`).
pub struct Edits {
    /// `None`: the service has no editing folder, and every edit fails at once.
    editing: Option<Arc<Editing>>,
    /// Each running edit's path and command channel.
    running: HashMap<EditId, (RemotePath, mpsc::UnboundedSender<EditCmd>)>,
    /// The host's SFTP session, as it comes and goes.
    session: watch::Sender<Option<Sftp>>,
}

impl Edits {
    pub fn new(editing: Option<Arc<Editing>>) -> Edits {
        Edits { editing, running: HashMap::new(), session: watch::channel(None).0 }
    }

    /// The host's session is up (`Some`) or gone: edits upload on it, and an upload cut off by
    /// a lost connection runs again.
    pub fn session(&self, sftp: Option<Sftp>) {
        self.session.send_replace(sftp);
    }

    /// Carries out `cmd` for `host`. An edit that isn't running answers anything but a `Start`
    /// with `Closed`; a `Start` without an editing folder or a session, or for a file being
    /// edited already, fails.
    pub fn command<S: Sink>(&mut self, cmd: RemoteEditCommand, host: &HostId, sink: &S) {
        self.running.retain(|_, (_, tx)| !tx.is_closed());
        let emit = |ev| sink.emit(ServiceEvent::RemoteEdit(ev));
        let (id, cmd) = match cmd {
            RemoteEditCommand::Start { id, path, .. } => return self.start(id, path, host, sink),
            RemoteEditCommand::Stop { id, .. } => (id, EditCmd::Stop),
            RemoteEditCommand::Resolve { id, choice, .. } => (id, EditCmd::Resolve(choice)),
            RemoteEditCommand::Retry { id, .. } => (id, EditCmd::Retry),
        };
        if !self.running.get(&id).is_some_and(|(_, tx)| tx.send(cmd).is_ok()) {
            emit(RemoteEditEvent::Closed { id, host: host.clone() });
        }
    }

    fn start<S: Sink>(&mut self, id: EditId, path: RemotePath, host: &HostId, sink: &S) {
        let fail = |error| {
            let ev = RemoteEditEvent::Failed { id, host: host.clone(), stage: EditStage::Download, error };
            sink.emit(ServiceEvent::RemoteEdit(ev));
        };
        let Some(editing) = &self.editing else {
            return fail(Error::Other("Editing in a local editor isn't available".into()));
        };
        if self.session.borrow().is_none() {
            return fail(Error::ConnectionLost);
        }
        if self.running.values().any(|(p, _)| *p == path) {
            return fail(Error::Other(format!("{path} is open for editing already")));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let start = Start {
            id,
            host: host.clone(),
            path: path.clone(),
            editing: editing.clone(),
            session: self.session.subscribe(),
            sink: sink.clone(),
        };
        tokio::spawn(start.run(rx));
        self.running.insert(id, (path, tx));
    }

    /// The host disconnects: every edit stops, its copy goes, and the UI hears it closed.
    pub fn stop_all(&mut self) {
        for (_, (_, tx)) in self.running.drain() {
            let _ = tx.send(EditCmd::Stop);
        }
    }
}

/// What a new edit's task starts from.
struct Start<S: Sink> {
    id: EditId,
    host: HostId,
    path: RemotePath,
    editing: Arc<Editing>,
    session: watch::Receiver<Option<Sftp>>,
    sink: S,
}

impl<S: Sink> Start<S> {
    /// The edit's task: its folder and copy, then the watch until it's stopped (or the host's
    /// task goes: the app quits).
    async fn run(self, cmds: mpsc::UnboundedReceiver<EditCmd>) {
        let Start { id, host, path, editing, session, sink } = self;
        let fail = |error| RemoteEditEvent::Failed { id, host: host.clone(), stage: EditStage::Download, error };
        let run = match editing.run().await {
            Ok(run) => run,
            Err(error) => return sink.emit(ServiceEvent::RemoteEdit(fail(error))),
        };
        let dir = session_dir(&run.dir, &host, &path);
        let local = dir.join(local_name(path.file_name().unwrap_or_default(), cfg!(windows)));
        let mut edit = Edit {
            id,
            host: host.clone(),
            path,
            folder: Folder(dir),
            local,
            base: Side::default(),
            watch: Watch::default(),
            wait: Wait::Watching,
            session,
            sink,
        };
        if let Err(error) = edit.open().await {
            // The folder goes with `edit`.
            return edit.sink.emit(ServiceEvent::RemoteEdit(fail(error)));
        }
        let (path, local) = (edit.path.clone(), edit.local.clone());
        edit.emit(RemoteEditEvent::Opened { id, host, path, local });
        edit.watch(cmds).await;
    }
}

/// An edit's folder, removed when the edit ends, however it ends (a task dropped at quit too).
struct Folder(PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        // Small, and usually gone already (`Edit::close`); a dropped task can't wait for a
        // blocking thread.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What an edit's task heard.
enum Heard {
    Cmd(Option<EditCmd>),
    /// Time to look at the copy.
    Look,
    /// The host's session came or went.
    Session,
}

/// What an edit waits for between saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// The next save.
    Watching,
    /// The conflict sheet's answer: nothing uploads meanwhile.
    Asking,
    /// Retry or the next save after a failed upload; `lost`: the connection was, and the upload
    /// runs again once the session is back.
    Failed { lost: bool },
}

/// One file being edited.
struct Edit<S: Sink> {
    id: EditId,
    host: HostId,
    path: RemotePath,
    folder: Folder,
    local: PathBuf,
    /// The server's copy as the edit last downloaded or uploaded it: a save checks against it.
    base: Side,
    watch: Watch,
    wait: Wait,
    session: watch::Receiver<Option<Sftp>>,
    sink: S,
}

impl<S: Sink> Edit<S> {
    fn emit(&self, ev: RemoteEditEvent) {
        self.sink.emit(ServiceEvent::RemoteEdit(ev));
    }

    fn sftp(&self) -> Option<Sftp> {
        self.session.borrow().clone()
    }

    /// A fresh folder, and the file downloaded into it.
    async fn open(&mut self) -> Result<(), Error> {
        let sftp = self.sftp().ok_or(Error::ConnectionLost)?;
        let dir = self.folder.0.clone();
        blocking(move || {
            // Left by an edit of the same file that couldn't clean up: it's this run's own.
            let _ = std::fs::remove_dir_all(&dir);
            private_dir(&dir)
        })
        .await?;
        self.fetch(&sftp, false).await
    }

    /// Looks at the copy, and carries out the UI's commands, until it's stopped.
    async fn watch(mut self, mut cmds: mpsc::UnboundedReceiver<EditCmd>) {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + WATCH_EVERY, WATCH_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let heard = tokio::select! {
                cmd = cmds.recv() => Heard::Cmd(cmd),
                _ = tick.tick() => Heard::Look,
                Ok(()) = self.session.changed() => Heard::Session,
            };
            match heard {
                // Stopped; or the host's task is gone (quitting), and the folder goes with it.
                Heard::Cmd(None | Some(EditCmd::Stop)) => break,
                Heard::Cmd(Some(EditCmd::Resolve(choice))) if self.wait == Wait::Asking => self.resolve(choice).await,
                Heard::Cmd(Some(EditCmd::Retry)) if matches!(self.wait, Wait::Failed { .. }) => self.save().await,
                Heard::Cmd(Some(_)) => {}
                Heard::Look => {
                    let local = self.local.clone();
                    let now = blocking(move || Ok(stamp(&local))).await.unwrap_or(None);
                    if self.watch.look(now) && self.wait != Wait::Asking {
                        self.save().await;
                    }
                }
                Heard::Session => {
                    let back = self.session.borrow_and_update().is_some();
                    if back && self.wait == (Wait::Failed { lost: true }) {
                        self.save().await;
                    }
                }
            }
        }
        self.close().await;
    }

    /// The edit stops: its folder goes, and the UI hears it closed.
    async fn close(self) {
        let dir = self.folder.0.clone();
        let _ = blocking(move || {
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        })
        .await;
        self.emit(RemoteEditEvent::Closed { id: self.id, host: self.host.clone() });
    }

    /// A save: the server's copy is checked, then the copy uploaded over it; a server's copy
    /// that changed since is a conflict instead.
    async fn save(&mut self) {
        let Some(sftp) = self.sftp() else { return self.failed(EditStage::Upload, Error::ConnectionLost) };
        let (id, host) = (self.id, self.host.clone());
        self.emit(RemoteEditEvent::Uploading { id, host: host.clone() });
        match io::remote_existing(&sftp, &self.path).await {
            Ok(found) => {
                let server = found.map(|e| e.side).unwrap_or_default();
                if server != self.base {
                    self.wait = Wait::Asking;
                    let local = io::local_side(&self.local).await;
                    return self.emit(RemoteEditEvent::Conflict { id, host, server, local });
                }
            }
            Err(error) => return self.failed(EditStage::Upload, error),
        }
        self.upload(&sftp, true).await;
    }

    /// The conflict sheet's answer.
    async fn resolve(&mut self, choice: EditChoice) {
        let Some(sftp) = self.sftp() else {
            let stage = if choice == EditChoice::Discard { EditStage::Reload } else { EditStage::Upload };
            return self.failed(stage, Error::ConnectionLost);
        };
        match choice {
            EditChoice::Replace => self.upload(&sftp, true).await,
            EditChoice::KeepBoth => self.upload(&sftp, false).await,
            EditChoice::Discard => match self.fetch(&sftp, true).await {
                Ok(()) => {
                    self.wait = Wait::Watching;
                    self.emit(RemoteEditEvent::Reloaded { id: self.id, host: self.host.clone() });
                }
                Err(error) => self.failed(EditStage::Reload, error),
            },
        }
    }

    /// Uploads the copy to the edit's path through a part file: over it (`replace`), or, when
    /// something is there, under the next free name beside it (Keep both: the edit stays on its
    /// path, whose server copy is still the newer one, so the next save asks again).
    async fn upload(&mut self, sftp: &Sftp, replace: bool) {
        let mine = io::local_side(&self.local).await;
        // Never resumed: a part file an earlier save left holds other bytes.
        let plan = FilePlan::Upload {
            src: self.local.clone(),
            dst: self.path.clone(),
            size: u64::MAX,
            mtime: None,
            overwrite: replace,
        };
        match io::upload(sftp, &plan, CopyOptions::default(), |_| {}).await {
            Ok(to) => {
                if to == self.path {
                    // As the server has it now; what was sent, should it not say.
                    self.base = match io::remote_existing(sftp, &to).await {
                        Ok(Some(e)) => e.side,
                        _ => mine,
                    };
                }
                self.wait = Wait::Watching;
                self.emit(RemoteEditEvent::Saved { id: self.id, host: self.host.clone(), to });
            }
            Err(error) => self.failed(EditStage::Upload, error),
        }
    }

    /// Something failed at `stage`: the UI hears why. A failed upload waits for Retry, the next
    /// save, or (the connection lost) the session coming back; a failed reload leaves the
    /// conflict standing.
    fn failed(&mut self, stage: EditStage, error: Error) {
        if stage == EditStage::Upload {
            self.wait = Wait::Failed { lost: error == Error::ConnectionLost };
        }
        self.emit(RemoteEditEvent::Failed { id: self.id, host: self.host.clone(), stage, error });
    }

    /// Downloads the server's copy into the copy's place: straight there at first, through
    /// [`STAGING`] when it replaces a copy (`staged`), so the copy is either the old one or the
    /// whole new one. The copy is the user's alone and its stamp the watch's starting point;
    /// what the server has is the next save's base.
    async fn fetch(&mut self, sftp: &Sftp, staged: bool) -> Result<(), Error> {
        let found = io::remote_existing(sftp, &self.path).await?.ok_or_else(|| Error::NotFound(self.path.clone()))?;
        if found.is_dir {
            return Err(Error::Other(format!("{} is a folder", self.path)));
        }
        let staging = self.folder.0.join(STAGING);
        let into = if staged { staging.join(self.local.file_name().unwrap_or_default()) } else { self.local.clone() };
        if staged {
            let dir = staging.clone();
            blocking(move || {
                let _ = std::fs::remove_dir_all(&dir);
                private_dir(&dir)
            })
            .await?;
        }
        let plan = FilePlan::Download {
            src: self.path.clone(),
            dst: into,
            size: found.side.size.unwrap_or(0),
            mtime: found.side.mtime,
        };
        let got = io::download(sftp, &plan, CopyOptions::default(), |_| {}).await?;
        let local = self.local.clone();
        let now = blocking(move || {
            if got != local {
                std::fs::rename(&got, &local).map_err(|e| local_err("replace", &local, &e))?;
            }
            restrict(&local, 0o600)?;
            if staged {
                let _ = std::fs::remove_dir_all(&staging);
            }
            Ok(stamp(&local))
        })
        .await?;
        self.watch.settle(now);
        self.base = match io::remote_existing(sftp, &self.path).await {
            Ok(Some(e)) => e.side,
            _ => found.side,
        };
        Ok(())
    }
}
