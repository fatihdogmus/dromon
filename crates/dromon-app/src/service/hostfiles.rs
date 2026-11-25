//! Saved hosts and the ssh config on the service thread (§3.8, §4.3).
//!
//! One task owns the files: it reads `hosts.conf` at startup, discovers the SSH Config hosts
//! (`hosts.rs`), and every [`WATCH_EVERY`] compares the modification times and sizes of every
//! config file discovery read (Includes too, the top-level files even when missing, and their
//! folders, where a new included file would appear) and of `hosts.conf`; a change discovers or
//! reads again. Saves, removals and "Add to SSH config" run one at a time in the same task, so
//! two writes never race; all file I/O is on blocking threads.
//!
//! The task tells the manager ([`Update`]); the manager keeps its table of known hosts in step
//! ([`Book::update`]) and only then tells the UI, so a host the UI shows can be connected.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dromon_core::{HostId, HostSource, HostSpec};
use dromon_ssh::{ConfigSources, append_block, backup_stamp, display_path, host_block, planned_backup, read_hosts_conf, write_hosts_conf};
use tokio::sync::mpsc;

use super::{Config, Sink, hosts};
use crate::state::hostbook::{ConfigPreview, upsert};
use crate::state::{HostBookCommand, HostBookEvent, ServiceEvent};

#[cfg(test)]
#[path = "hostfiles_tests.rs"]
mod tests;

/// How often the config files are looked at.
pub const WATCH_EVERY: Duration = Duration::from_secs(2);

/// What the files task tells the manager.
#[derive(Debug)]
pub enum Update {
    /// Discovery round `round`'s hosts: the aliases first (`resolved: false`), then what
    /// `ssh -G` said about them.
    Config { round: u64, specs: Vec<HostSpec>, resolved: bool },
    /// The saved hosts, in order.
    Saved(Vec<HostSpec>),
    /// An event for the UI, after the updates sent before it.
    Emit(ServiceEvent),
}

/// The manager's end: commands go to the task, updates come back.
pub struct Book {
    tx: mpsc::UnboundedSender<HostBookCommand>,
    /// The newest discovery round seen.
    round: u64,
    /// The SSH Config hosts the UI was last told about.
    config: Option<Vec<HostSpec>>,
}

impl Book {
    /// Starts the files task for `cfg`'s files.
    pub fn start(cfg: &Config) -> (Book, mpsc::UnboundedReceiver<Update>) {
        let (tx, cmds) = mpsc::unbounded_channel();
        let (out, updates) = mpsc::unbounded_channel();
        let task = Task {
            hosts_file: cfg.hosts_file.clone(),
            sources: cfg.discover.clone(),
            ssh: cfg.ssh.clone(),
            config_file: cfg.config_file.clone(),
            out,
            saved: Vec::new(),
            round: 0,
            watched: Vec::new(),
            hosts_stamp: None,
        };
        tokio::spawn(run(task, cmds));
        (Book { tx, round: 0, config: None }, updates)
    }

    /// A command from the UI. The recent hosts are the manager's own; the rest go to the task.
    pub fn command(&self, cmd: HostBookCommand, known: &mut HashMap<HostId, HostSpec>) {
        match cmd {
            HostBookCommand::Recent(list) => set_section(known, HostSource::Recent, &list),
            other => {
                let _ = self.tx.send(other);
            }
        }
    }

    /// An update from the task: `known` follows it, then the UI hears of it.
    pub fn update<S: Sink>(&mut self, u: Update, known: &mut HashMap<HostId, HostSpec>, sink: &S) {
        match u {
            Update::Config { round, specs, resolved } => {
                if round < self.round {
                    return;
                }
                self.round = round;
                // A new round's bare aliases keep what the last one resolved, so the sidebar
                // doesn't lose the addresses until `ssh -G` has run again.
                let specs = if resolved { specs } else { keep_resolved(specs, self.config.as_deref().unwrap_or_default()) };
                set_section(known, HostSource::SshConfig, &specs);
                if self.config.as_ref() != Some(&specs) {
                    self.config = Some(specs.clone());
                    sink.emit(ServiceEvent::Hosts(specs));
                }
            }
            Update::Saved(list) => {
                set_section(known, HostSource::Saved, &list);
                sink.emit(ServiceEvent::HostBook(HostBookEvent::Saved(list)));
            }
            Update::Emit(ev) => sink.emit(ev),
        }
    }
}

/// Makes `specs` the hosts of `source` in `known`. An id another section has is that section's
/// when it ranks higher (SSH Config, then Saved, then Recent), as in the sidebar.
pub fn set_section(known: &mut HashMap<HostId, HostSpec>, source: HostSource, specs: &[HostSpec]) {
    let rank = |s: HostSource| match s {
        HostSource::SshConfig => 0,
        HostSource::Saved => 1,
        HostSource::Recent => 2,
    };
    known.retain(|_, h| h.source != source);
    for h in specs.iter().filter(|h| h.source == source) {
        if known.get(&h.id).is_some_and(|k| rank(k.source) < rank(source)) {
            continue;
        }
        known.insert(h.id.clone(), h.clone());
    }
}

/// `specs` (bare aliases) with what `last` resolved for the same aliases.
pub fn keep_resolved(specs: Vec<HostSpec>, last: &[HostSpec]) -> Vec<HostSpec> {
    specs
        .into_iter()
        .map(|s| match last.iter().find(|l| l.alias == s.alias) {
            Some(l) => HostSpec { hostname: l.hostname.clone(), user: l.user.clone(), port: l.port, proxy_jump: l.proxy_jump.clone(), ..s },
            None => s,
        })
        .collect()
}

/// A file's modification time and size; `None` when it isn't there.
type Stamp = Option<(SystemTime, u64)>;

fn stamp(p: &Path) -> Stamp {
    std::fs::metadata(p).ok().map(|m| (m.modified().unwrap_or(UNIX_EPOCH), m.len()))
}

/// What to watch for a discovery that read `files`: those files, the top-level config files
/// (even when missing), and the folders all of them are in.
pub fn watch_list(src: &ConfigSources, files: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: &Path| {
        if !out.iter().any(|o| o == p) {
            out.push(p.to_path_buf());
        }
    };
    let top = src.user.iter().chain(&src.system);
    for f in files.iter().chain(top) {
        add(f);
        if let Some(dir) = f.parent().filter(|d| !d.as_os_str().is_empty()) {
            add(dir);
        }
    }
    out
}

/// The backup's time stamp for now, on the user's clock.
fn now_stamp() -> String {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let t = rune::time::LocalTime::at(ms);
    backup_stamp(t.year, t.month, t.day, t.hour, t.minute)
}

/// Runs `f` on a blocking thread (file I/O).
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| panic!("dromon: host files: {e}"))
}

struct Task {
    hosts_file: Option<PathBuf>,
    sources: Option<ConfigSources>,
    ssh: PathBuf,
    config_file: Option<PathBuf>,
    out: mpsc::UnboundedSender<Update>,
    saved: Vec<HostSpec>,
    round: u64,
    /// What discovery read (and where new files would appear), as they were then.
    watched: Vec<(PathBuf, Stamp)>,
    hosts_stamp: Stamp,
}

async fn run(mut t: Task, mut cmds: mpsc::UnboundedReceiver<HostBookCommand>) {
    t.load_saved(true).await;
    t.discover().await;
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + WATCH_EVERY, WATCH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(cmd) => t.command(cmd).await,
                // The manager is gone: the app is quitting.
                None => return,
            },
            _ = tick.tick() => t.check().await,
        }
    }
}

impl Task {
    fn emit(&self, ev: HostBookEvent) {
        let _ = self.out.send(Update::Emit(ServiceEvent::HostBook(ev)));
    }

    fn send_saved(&self) {
        let _ = self.out.send(Update::Saved(self.saved.clone()));
    }

    /// Reads `hosts.conf`; tells the manager when it says something new (or `always`).
    async fn load_saved(&mut self, always: bool) {
        let Some(path) = self.hosts_file.clone() else { return };
        let (read, st) = blocking(move || (read_hosts_conf(&path), stamp(&path))).await;
        self.hosts_stamp = st;
        match read {
            Ok(list) if always || list != self.saved => {
                self.saved = list;
                self.send_saved();
            }
            Ok(_) => {}
            // Unreadable: keep what we had; saving will say why.
            Err(e) => eprintln!("dromon: saved hosts: {e}"),
        }
    }

    /// One discovery round: the aliases now (and what to watch), then `ssh -G` in the background.
    async fn discover(&mut self) {
        let Some(src) = self.sources.clone() else { return };
        self.round += 1;
        let round = self.round;
        let (specs, watched) = blocking(move || {
            let (specs, files) = hosts::aliases_and_files(&src);
            let watched: Vec<(PathBuf, Stamp)> = watch_list(&src, &files).into_iter().map(|p| (p.clone(), stamp(&p))).collect();
            (specs, watched)
        })
        .await;
        self.watched = watched;
        let _ = self.out.send(Update::Config { round, specs: specs.clone(), resolved: false });
        if specs.is_empty() {
            return;
        }
        let (ssh, file, out) = (self.ssh.clone(), self.config_file.clone(), self.out.clone());
        tokio::spawn(async move {
            let specs = hosts::resolve_all(&ssh, file.as_deref(), specs).await;
            let _ = out.send(Update::Config { round, specs, resolved: true });
        });
    }

    /// Looks at the watched files: discovers again when a config file changed, reads
    /// `hosts.conf` again when it did.
    async fn check(&mut self) {
        let paths: Vec<PathBuf> = self.watched.iter().map(|(p, _)| p.clone()).collect();
        let hosts_file = self.hosts_file.clone();
        let (now, hosts_now) = blocking(move || (paths.iter().map(|p| stamp(p)).collect::<Vec<_>>(), hosts_file.as_deref().and_then(stamp))).await;
        if now.iter().zip(&self.watched).any(|(n, (_, was))| n != was) {
            self.discover().await;
        }
        if self.hosts_file.is_some() && hosts_now != self.hosts_stamp {
            self.load_saved(false).await;
        }
    }

    async fn command(&mut self, cmd: HostBookCommand) {
        match cmd {
            // The manager's.
            HostBookCommand::Recent(_) => {}
            HostBookCommand::Save { spec, replaces } => {
                let mut list = self.saved.clone();
                upsert(&mut list, spec, replaces.as_ref());
                self.write(list).await;
            }
            HostBookCommand::Remove(id) => {
                let list = self.saved.iter().filter(|h| h.id != id).cloned().collect();
                self.write(list).await;
            }
            HostBookCommand::PreviewAddToConfig(host) => {
                let preview = self.preview(&host).await;
                self.emit(HostBookEvent::Preview { host, preview });
            }
            HostBookCommand::AddToConfig(host) => self.add_to_config(host).await,
        }
    }

    /// Writes `list` as `hosts.conf`; the manager hears the saved hosts either way.
    async fn write(&mut self, list: Vec<HostSpec>) -> bool {
        let Some(path) = self.hosts_file.clone() else {
            self.emit(HostBookEvent::SaveFailed("there's no folder to keep them in".into()));
            self.send_saved();
            return false;
        };
        let written = list.clone();
        let (result, st) = blocking(move || {
            let r = write_hosts_conf(&path, &written);
            (r, stamp(&path))
        })
        .await;
        let ok = result.is_ok();
        match result {
            Ok(()) => {
                self.saved = list;
                self.hosts_stamp = st;
            }
            Err(e) => self.emit(HostBookEvent::SaveFailed(e.to_string())),
        }
        self.send_saved();
        ok
    }

    /// The user's ssh config and the block for saved host `host`.
    fn plan(&self, host: &HostId) -> Result<(PathBuf, String), String> {
        let spec = self.saved.iter().find(|h| &h.id == host).ok_or_else(|| format!("“{host}” isn't a saved host any more."))?;
        let target = self.sources.as_ref().and_then(|s| s.user.clone()).ok_or("Dromon doesn't know where your ssh config is.")?;
        let block = host_block(spec).map_err(|e| format!("Its settings can't go in an ssh config: {e}"))?;
        Ok((target, block))
    }

    /// The home folder paths are shown under (`~`).
    fn home(&self) -> Option<PathBuf> {
        self.sources.as_ref().and_then(|s| s.user_ssh_dir.parent().map(Path::to_path_buf)).filter(|h| !h.as_os_str().is_empty())
    }

    async fn preview(&self, host: &HostId) -> Result<ConfigPreview, String> {
        let (target, block) = self.plan(host)?;
        let stamp = now_stamp();
        let t = target.clone();
        let backup = blocking(move || planned_backup(&t, &stamp)).await;
        let home = self.home();
        Ok(ConfigPreview {
            target: display_path(&target, home.as_deref()),
            backup: backup.map(|b| display_path(&b, home.as_deref())),
            block,
        })
    }

    /// Backs up and appends to the ssh config, discovers (the host is under SSH Config now),
    /// then takes the host out of `hosts.conf`.
    async fn add_to_config(&mut self, host: HostId) {
        let (target, block) = match self.plan(&host) {
            Ok(p) => p,
            Err(error) => return self.emit(HostBookEvent::AddFailed { host, error }),
        };
        let stamp = now_stamp();
        if let Err(e) = blocking(move || append_block(&target, &block, &stamp)).await {
            return self.emit(HostBookEvent::AddFailed { host, error: e.to_string() });
        }
        self.discover().await;
        let list = self.saved.iter().filter(|h| h.id != host).cloned().collect();
        self.write(list).await;
        self.emit(HostBookEvent::Added { host });
    }
}
