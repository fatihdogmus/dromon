//! The Properties sheet's data end to end against the sshd fixture (`dromon-testkit`: the
//! Docker image, or the native sshd): the owners a real listing carries (from the server's
//! `ls -l` longnames), and Calculate's folder walk (`Command::FolderSize`) on the real service
//! thread, the system `ssh` and SFTP, on the seeded `fixture/mixed` and `fixture/many`
//! (`tests/docker/seed.sh`).
//!
//! Commands go in through the service's channel as the UI sends them; the events coming back
//! are what the sheet would show. One key-auth alias, `fx-key`, with a private `known_hosts`.
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{Backend, Command, ServiceEvent, SizeCommand, SizeEvent, SizeId, SizeTotal};
use dromon_core::{ConnStatus, Entry, Error, HostId, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);

fn host() -> HostId {
    HostId("fx-key".into())
}

#[derive(Clone)]
struct TestSink(mpsc::Sender<ServiceEvent>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send(ev);
    }
}

struct Harness {
    fx: &'static Fixture,
    _dir: tempfile::TempDir,
    backend: service::ChannelBackend,
    events: mpsc::Receiver<ServiceEvent>,
    /// Every connection-log line (ssh's stderr among them) and prompt, for failure messages.
    log: Vec<String>,
}

impl Harness {
    /// The service; connected when `connect`.
    fn new(name: &str, connect: bool) -> Option<Harness> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let fx = rt.block_on(dromon_testkit::fixture(name))?;
        let dir = tempfile::tempdir().unwrap();
        let config = write_ssh_config(fx, dir.path());
        let (tx, events) = mpsc::channel();
        let (backend, commands) = service::channel();
        let cfg = Config {
            ssh: dromon_ssh::ssh_program(),
            helper_exe: PathBuf::from(HELPER),
            verbose: false,
            config_file: Some(config),
            discover: None,
            hosts_file: None,
            hosts: vec![dromon_core::HostSpec::from_config_alias("fx-key")],
            without_env: vec!["SSH_AUTH_SOCK".into()],
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let mut h = Harness { fx, _dir: dir, backend, events, log: Vec::new() };
        if connect {
            h.backend.send(Command::Connect { host: host() });
            h.wait_for("the connection to open", |ev| matches!(ev, ServiceEvent::Opened { .. }).then_some(()));
        }
        Some(h)
    }

    /// The first event `pick` takes, skipping others.
    fn wait_for<T>(&mut self, what: &str, mut pick: impl FnMut(&ServiceEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = self
                .events
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("timed out waiting for {what}\nconnection log:\n{}", self.log.join("\n")));
            match &ev {
                ServiceEvent::Log { host, line } => self.log.push(format!("[{}] {line}", host.0)),
                ServiceEvent::Prompt(p) => self.log.push(format!("unexpected prompt: {p:?}")),
                ServiceEvent::Status { status: ConnStatus::Failed(why), .. } => {
                    panic!("the connection failed: {why}\nconnection log:\n{}", self.log.join("\n"))
                }
                _ => {}
            }
            if let Some(found) = pick(&ev) {
                return found;
            }
        }
    }

    /// The whole listing of `path`.
    fn list(&mut self, path: &RemotePath) -> Vec<Entry> {
        self.backend.send(Command::List { host: host(), path: path.clone() });
        let mut all = Vec::new();
        loop {
            let (entries, done) = self.wait_for("a listing page", |ev| match ev {
                ServiceEvent::ListingPage { path: p, entries, done, .. } if p == path => Some((entries.clone(), *done)),
                ServiceEvent::ListFailed { path: p, error, .. } if p == path => panic!("listing {p} failed: {error}"),
                _ => None,
            });
            all.extend(entries);
            if done {
                return all;
            }
        }
    }

    /// Calculate on `path`: how the walk ended, and the progress it reported before.
    fn folder_size(&mut self, id: u64, path: &RemotePath) -> (Result<SizeTotal, (SizeTotal, Error)>, Vec<SizeTotal>) {
        let id = SizeId(id);
        self.backend.send(Command::FolderSize(SizeCommand::Start { id, host: host(), path: path.clone() }));
        let mut progress = Vec::new();
        let end = self.wait_for("the walk to end", |ev| match ev {
            ServiceEvent::FolderSize(SizeEvent::Progress { id: i, total, .. }) if *i == id => {
                progress.push(*total);
                None
            }
            ServiceEvent::FolderSize(SizeEvent::Done { id: i, total, .. }) if *i == id => Some(Ok(*total)),
            ServiceEvent::FolderSize(SizeEvent::Failed { id: i, total, error, .. }) if *i == id => {
                Some(Err((*total, error.clone())))
            }
            _ => None,
        });
        (end, progress)
    }
}

fn write_ssh_config(fx: &Fixture, dir: &Path) -> PathBuf {
    let key = dir.join("id_ed25519");
    copy_private_key(&fx.key(), &key).unwrap();
    let mut config = String::new();
    writeln!(
        config,
        "Host fx-key\n    HostName {}\n    Port {}\n    User {}",
        fx.host(),
        fx.port(),
        fx.user(Role::Key)
    )
    .unwrap();
    writeln!(
        config,
        "    IdentityFile {}\n    IdentitiesOnly yes\n    PreferredAuthentications publickey",
        key.display()
    )
    .unwrap();
    writeln!(
        config,
        "    UserKnownHostsFile {}\n    GlobalKnownHostsFile {NULL_DEVICE}",
        dir.join("known_hosts").display()
    )
    .unwrap();
    writeln!(config, "    StrictHostKeyChecking accept-new\n    UpdateHostKeys no").unwrap();
    let path = dir.join("ssh_config");
    std::fs::write(&path, config).unwrap();
    path
}

#[test]
fn a_listing_names_owners_and_calculate_totals_a_seeded_folder() {
    let Some(mut h) = Harness::new("folder_size", true) else { return };
    let mixed = h.fx.path(Role::Key, "fixture/mixed");

    // Owners: the names from the server's `ls -l` line, for every entry.
    let entries = h.list(&mixed);
    let file = entries.iter().find(|e| e.name == "file-001.txt").expect("file-001.txt");
    let owner = format!("{}:{}", file.owner.as_deref().unwrap_or("?"), file.group.as_deref().unwrap_or("?"));
    assert_eq!(owner, h.fx.owner(Role::Key), "{file:?}");
    assert!(entries.iter().all(|e| e.owner.is_some() && e.group.is_some()), "every entry has an owner");

    // fixture/mixed (seed.sh): files of 100 × i bytes for i in 1..=80, dir-01/inside.txt
    // ("inside\n"), empty.txt, "space name.txt" ("spaces\n"), unicode-ğüş.txt ("unicode\n"),
    // .hidden ("secret\n"), three symlinks counted by their own size (the length of their
    // target: "dir-01", "file-001.txt", "does-not-exist", never followed) and a FIFO; folders
    // dir-01..dir-10, .config and locked (mode 000: it can't be read).
    let bytes = 100 * (80 * 81 / 2) + 7 + 7 + 8 + 7 + 6 + 12 + 14;
    let files = 80 + 1 + 3 + 1 + 3 + 1;
    let (end, progress) = h.folder_size(1, &mixed);
    assert_eq!(end, Ok(SizeTotal { bytes, files, folders: 12, unreadable: 1 }), "progress: {progress:?}");
    assert!(progress.windows(2).all(|w| w[0].items() <= w[1].items()), "{progress:?}");

    // fixture/many: 10,000 empty files.
    let many = h.fx.path(Role::Key, "fixture/many");
    let (end, progress) = h.folder_size(2, &many);
    assert_eq!(end, Ok(SizeTotal { bytes: 0, files: 10_000, folders: 0, unreadable: 0 }));
    println!("walk of 10,000 entries: {} progress events", progress.len());
}

#[test]
fn without_a_connection_calculate_fails_at_once() {
    let Some(mut h) = Harness::new("folder_size_offline", false) else { return };
    let path = h.fx.path(Role::Key, "fixture/mixed");
    assert_eq!(h.folder_size(1, &path), (Err((SizeTotal::default(), Error::ConnectionLost)), Vec::new()));
}
