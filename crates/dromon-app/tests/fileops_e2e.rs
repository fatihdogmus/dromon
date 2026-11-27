//! File operations (new folder, rename, delete) end to end against the sshd fixture
//! (`dromon-testkit`: the Docker image, or the native sshd): the real service thread
//! (`service::start`) with its connection tasks, the system `ssh`, and SFTP to the fixture.
//! Commands go in through the service's channel as the UI sends them; the `FileOp*` events and
//! listings coming back are what the UI would show.
//!
//! Each test writes its own ssh config (one key-auth alias, `fx-key`, with a private
//! `known_hosts` that accepts the fixture's key) and works in its own scratch folder on the
//! server (in the key user's home), with the folder on screen listed first.
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails).
//!
//! Windows: the same tests with the inbox `ssh.exe`, against the Docker image in WSL2 (CI); the
//! key copy gets a current-user-only ACL (`dromon_testkit::copy_private_key`) and the config
//! names `NUL` for "no file".

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{Backend, Command, FileOp, FileOpEvent, OpId, ServiceEvent};
use dromon_core::{ConnStatus, Error, HostId, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);

/// One `FileOpEvent::Progress`: `(done, total)`.
type Progress = (u64, Option<u64>);

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
    /// This test's folder on the server.
    scratch: RemotePath,
    /// Every connection-log line (ssh's stderr among them) and prompt, for failure messages.
    log: Vec<String>,
}

impl Harness {
    /// The service with a fresh scratch folder; connected (and the scratch folder on screen)
    /// when `connect`.
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
            editing_root: None,
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let scratch = fx.path(Role::Key, &format!("scratch/fileops-{name}-{}", std::process::id()));
        // `exec` runs as root: the folder goes to the user the app signs in as.
        let make = format!("rm -rf '{0}' && mkdir -p '{0}' && chown {1} '{0}'", scratch.as_str(), fx.owner(Role::Key));
        fx.exec(&["sh", "-c", &make]).unwrap();
        let mut h = Harness { fx, _dir: dir, backend, events, scratch, log: Vec::new() };
        if connect {
            h.backend.send(Command::Connect { host: host() });
            h.wait_for("the connection to open", |ev| matches!(ev, ServiceEvent::Opened { .. }).then_some(()));
            let scratch = h.scratch.clone();
            h.list(&scratch);
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
                // Key auth and accept-new: nothing should ask. Kept for the failure message.
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

    /// The sorted names of the next whole listing of `path`.
    fn listing(&mut self, path: &RemotePath) -> Vec<String> {
        let mut names = Vec::new();
        loop {
            let (entries, done) = self.wait_for("a listing page", |ev| match ev {
                ServiceEvent::ListingPage { path: p, entries, done, .. } if p == path => Some((entries.clone(), *done)),
                ServiceEvent::ListFailed { path: p, error, .. } if p == path => panic!("listing {p} failed: {error}"),
                _ => None,
            });
            names.extend(entries.into_iter().map(|e| e.name));
            if done {
                names.sort();
                return names;
            }
        }
    }

    /// Lists `path`, making it the folder on screen.
    fn list(&mut self, path: &RemotePath) -> Vec<String> {
        self.backend.send(Command::List { host: host(), path: path.clone() });
        self.listing(path)
    }

    fn op(&mut self, id: u64, op: FileOp) {
        self.backend.send(Command::FileOp { id: OpId(id), host: host(), op });
    }

    /// How operation `id` ended, and the progress events before that.
    fn end(&mut self, id: u64) -> (Result<(), Error>, Vec<Progress>) {
        let id = OpId(id);
        let mut progress = Vec::new();
        let result = self.wait_for("the operation to end", |ev| match ev {
            ServiceEvent::FileOp(FileOpEvent::Progress { id: i, done, total, .. }) if *i == id => {
                progress.push((*done, *total));
                None
            }
            ServiceEvent::FileOp(FileOpEvent::Done { id: i, .. }) if *i == id => Some(Ok(())),
            ServiceEvent::FileOp(FileOpEvent::Failed { id: i, error, .. }) if *i == id => Some(Err(error.clone())),
            _ => None,
        });
        (result, progress)
    }

    fn remote(&self, cmd: &str) -> String {
        self.fx.exec(&["sh", "-c", cmd]).unwrap()
    }

    fn at(&self, rel: &str) -> RemotePath {
        self.scratch.join(rel)
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
fn a_new_folder_and_a_rename_and_their_refusals() {
    let Some(mut h) = Harness::new("mkdir_rename", true) else { return };
    let scratch = h.scratch.clone();

    h.op(1, FileOp::Mkdir { path: h.at("new") });
    assert_eq!(h.end(1).0, Ok(()));
    assert_eq!(h.listing(&scratch), ["new"], "the folder on screen again");
    assert_eq!(h.remote(&format!("test -d '{}' && echo yes", h.at("new").as_str())).trim(), "yes");

    h.op(2, FileOp::Mkdir { path: h.at("new") });
    assert_eq!(h.end(2).0, Err(Error::AlreadyExists(h.at("new"))));

    h.op(3, FileOp::Rename { from: h.at("new"), to: h.at("renamed") });
    assert_eq!(h.end(3).0, Ok(()));
    assert_eq!(h.listing(&scratch), ["renamed"]);

    // Onto a name that's taken (a file, and a folder): refused, nothing replaced.
    h.remote(&format!(
        "printf hi > '{0}/taken.txt' && chown {1} '{0}/taken.txt'",
        scratch.as_str(),
        h.fx.owner(Role::Key)
    ));
    h.op(4, FileOp::Rename { from: h.at("renamed"), to: h.at("taken.txt") });
    assert_eq!(h.end(4).0, Err(Error::AlreadyExists(h.at("taken.txt"))));
    h.op(5, FileOp::Rename { from: h.at("taken.txt"), to: h.at("renamed") });
    assert_eq!(h.end(5).0, Err(Error::AlreadyExists(h.at("renamed"))));
    assert_eq!(h.remote(&format!("cat '{}'", h.at("taken.txt").as_str())), "hi");
    assert_eq!(h.list(&scratch), ["renamed", "taken.txt"]);
}

#[test]
fn deleting_a_tree_reports_progress_and_lists_its_parent_again() {
    let Some(mut h) = Harness::new("delete_tree", true) else { return };
    let scratch = h.scratch.clone();
    // tree/a: 4000 files, tree/a/b: 1000, tree/c empty, tree/link → ../target (a folder outside).
    let s = scratch.as_str();
    h.remote(&format!(
        "mkdir -p '{s}/tree/a/b' '{s}/tree/c' '{s}/target' && touch '{s}/target/keep.txt' \
         && for i in $(seq 1 4000); do : > '{s}/tree/a/f'$i; done \
         && for i in $(seq 1 1000); do : > '{s}/tree/a/b/g'$i; done \
         && chown -R {owner} '{s}/tree' '{s}/target' && ln -s ../target '{s}/tree/link'",
        owner = h.fx.owner(Role::Key)
    ));
    assert_eq!(h.list(&scratch), ["target", "tree"]);

    h.op(1, FileOp::Delete { paths: vec![h.at("tree")] });
    let (result, progress) = h.end(1);
    assert_eq!(result, Ok(()));
    // tree, a, 4000 files, b, 1000 files, c and the link.
    let total = 5005;
    assert!(!progress.is_empty());
    assert_eq!(progress[0].1, None, "counting comes first: {progress:?}");
    let split = progress.iter().position(|(_, t)| t.is_some()).unwrap_or(progress.len());
    assert!(progress[..split].windows(2).all(|w| w[0].0 <= w[1].0), "{progress:?}");
    assert!(progress[split..].iter().all(|&(done, t)| t == Some(total) && done <= total), "{progress:?}");
    assert!(progress[split..].windows(2).all(|w| w[0].0 < w[1].0), "{progress:?}");
    println!("delete of {total} entries: {} progress events", progress.len());

    assert_eq!(h.listing(&scratch), ["target"], "the parent on screen, without it");
    assert_eq!(h.remote(&format!("ls '{s}/target'")).trim(), "keep.txt", "the link's target stays");

    // Deleting the folder on screen: done, and it isn't listed again (the next listing is
    // the one asked for next).
    h.remote(&format!("mkdir -p '{s}/target/inner' && chown -R {} '{s}/target'", h.fx.owner(Role::Key)));
    assert_eq!(h.list(&h.at("target")), ["inner", "keep.txt"]);
    h.op(2, FileOp::Delete { paths: vec![h.at("target")] });
    assert_eq!(h.end(2).0, Ok(()));
    h.backend.send(Command::List { host: host(), path: scratch.clone() });
    let next = h.wait_for("the next listing", |ev| match ev {
        ServiceEvent::ListingPage { path, .. } | ServiceEvent::ListFailed { path, .. } => Some(path.clone()),
        _ => None,
    });
    assert_eq!(next, scratch);
}

#[test]
fn without_a_connection_an_operation_fails_at_once() {
    let Some(mut h) = Harness::new("no_connection", false) else { return };
    h.op(1, FileOp::Delete { paths: vec![h.at("anything")] });
    assert_eq!(h.end(1), (Err(Error::ConnectionLost), Vec::new()));
}
