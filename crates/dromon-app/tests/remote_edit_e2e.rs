//! Editing in a local editor (`Command::RemoteEdit`) end to end against the sshd fixture
//! (`dromon-testkit`: the Docker image, or the native sshd): the real service thread with its
//! connection tasks, the system `ssh`, and SFTP to the fixture. Commands go in through the
//! service's channel as the UI sends them; no editor runs: the test writes the local copy the
//! `Opened` event names, as an editor's save would, and reads the server with `exec`.
//!
//! Each test writes its own ssh config (one key-auth alias, `fx-key`, with a private
//! `known_hosts`) and works in its own scratch folder on the server. The local copies go in the
//! test's own temporary folder (`Config::editing_root`), never the user's cache folder.
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{
    Backend, Command, EditChoice, EditId, EditStage, RemoteEditCommand, RemoteEditEvent, ServiceEvent,
};
use dromon_core::{ConnStatus, Error, HostId, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);
const ID: EditId = EditId(1);

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
    /// The service's editing folder, in `_dir`.
    editing: PathBuf,
    backend: service::ChannelBackend,
    events: mpsc::Receiver<ServiceEvent>,
    /// This test's folder on the server, holding notes.txt ("hello\n").
    scratch: RemotePath,
    /// Every connection-log line (ssh's stderr among them) and prompt, for failure messages.
    log: Vec<String>,
}

impl Harness {
    /// The service with a fresh scratch folder; connected when `connect`.
    fn new(name: &str, connect: bool) -> Option<Harness> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let fx = rt.block_on(dromon_testkit::fixture(name))?;
        let dir = tempfile::tempdir().unwrap();
        let config = write_ssh_config(fx, dir.path());
        let editing = dir.path().join("editing");
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
            editing_root: Some(editing.clone()),
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let scratch = fx.path(Role::Key, &format!("scratch/edit-{name}-{}", std::process::id()));
        // `exec` runs as root: the files go to the user the app signs in as.
        let make = format!(
            "rm -rf '{d}' && mkdir -p '{d}' && printf 'hello\\n' > '{d}/notes.txt' && chown -R {owner} '{d}'",
            d = scratch.as_str(),
            owner = fx.owner(Role::Key),
        );
        fx.exec(&["sh", "-c", &make]).unwrap();
        let mut h = Harness { fx, _dir: dir, editing, backend, events, scratch, log: Vec::new() };
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

    /// The next edit event, other than an upload starting.
    fn next_edit(&mut self, what: &str) -> RemoteEditEvent {
        self.wait_for(what, |ev| match ev {
            ServiceEvent::RemoteEdit(RemoteEditEvent::Uploading { .. }) => None,
            ServiceEvent::RemoteEdit(e) => Some(e.clone()),
            _ => None,
        })
    }

    fn edit(&mut self, cmd: RemoteEditCommand) {
        self.backend.send(Command::RemoteEdit(cmd));
    }

    /// Starts editing the scratch file `name`: its local copy, in the test's editing folder.
    fn start(&mut self, name: &str) -> PathBuf {
        let path = self.scratch.join(name);
        self.edit(RemoteEditCommand::Start { id: ID, host: host(), path: path.clone() });
        let local = match self.next_edit("the copy to open") {
            RemoteEditEvent::Opened { id, path: p, local, .. } if id == ID && p == path => local,
            other => panic!("not opened: {other:?}"),
        };
        assert!(local.starts_with(&self.editing), "{} isn't under {}", local.display(), self.editing.display());
        local
    }

    /// Stop editing: the edit closes and its copy's folder goes.
    fn stop(&mut self, local: &Path) {
        self.edit(RemoteEditCommand::Stop { id: ID, host: host() });
        assert_eq!(self.next_edit("the edit to stop"), RemoteEditEvent::Closed { id: ID, host: host() });
        assert!(!local.parent().unwrap().exists(), "the local copy goes with the edit");
    }

    fn resolve(&mut self, choice: EditChoice) {
        self.edit(RemoteEditCommand::Resolve { id: ID, host: host(), choice });
    }

    /// The scratch file `name` as the server has it.
    fn server(&self, name: &str) -> String {
        self.fx.exec(&["cat", self.scratch.join(name).as_str()]).unwrap()
    }

    /// Someone else changes the scratch file `name` on the server.
    fn elsewhere(&self, name: &str, text: &str) {
        let script = format!("printf '{text}' > '{}'", self.scratch.join(name).as_str());
        self.fx.exec(&["sh", "-c", &script]).unwrap();
    }
}

/// What an editor's save does: the new bytes in place of the copy's, through a temporary file
/// renamed over it.
fn save(local: &Path, text: &str) {
    let tmp = local.with_file_name(".save.tmp");
    std::fs::write(&tmp, text).unwrap();
    std::fs::rename(&tmp, local).unwrap();
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

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn an_edit_downloads_a_private_copy_and_each_save_reaches_the_server() {
    let Some(mut h) = Harness::new("edit_save", true) else { return };
    let local = h.start("notes.txt");
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "hello\n");
    assert_eq!(local.file_name().and_then(|n| n.to_str()), Some("notes.txt"), "the file's own name");
    #[cfg(unix)]
    {
        let folder = local.parent().unwrap();
        assert_eq!(mode(&local), 0o600, "the copy is the user's alone");
        assert_eq!(mode(folder), 0o700);
        assert_eq!(mode(folder.parent().unwrap()), 0o700, "so is the run's folder");
    }
    save(&local, "hello, world\n");
    assert_eq!(
        h.next_edit("the save"),
        RemoteEditEvent::Saved { id: ID, host: host(), to: h.scratch.join("notes.txt") }
    );
    assert_eq!(h.server("notes.txt"), "hello, world\n");
    // Once: nothing more uploads until the next save, which goes up too.
    save(&local, "third\n");
    assert!(matches!(h.next_edit("the next save"), RemoteEditEvent::Saved { .. }));
    assert_eq!(h.server("notes.txt"), "third\n");
    // No part file is left beside it.
    let listed = h.fx.exec(&["ls", "-A", h.scratch.as_str()]).unwrap();
    assert_eq!(listed.trim(), "notes.txt");
    h.stop(&local);
}

#[test]
fn a_change_on_the_server_brings_a_conflict_keep_both_and_discard_answer_it() {
    let Some(mut h) = Harness::new("edit_conflict", true) else { return };
    let local = h.start("notes.txt");
    h.elsewhere("notes.txt", "theirs, longer\\n");
    save(&local, "mine\n");
    let RemoteEditEvent::Conflict { server, local: mine, .. } = h.next_edit("the conflict") else {
        panic!("no conflict")
    };
    assert_eq!((server.size, mine.size), (Some(15), Some(5)));
    assert_eq!(h.server("notes.txt"), "theirs, longer\n", "nothing uploaded over it");
    // Keep both: yours beside theirs, under the next free name.
    h.resolve(EditChoice::KeepBoth);
    let to = h.scratch.join("notes (1).txt");
    assert_eq!(h.next_edit("keep both"), RemoteEditEvent::Saved { id: ID, host: host(), to });
    assert_eq!((h.server("notes.txt"), h.server("notes (1).txt")), ("theirs, longer\n".into(), "mine\n".into()));
    // The edit stays on notes.txt, still theirs: the next save asks again.
    save(&local, "mine, again\n");
    assert!(matches!(h.next_edit("the conflict again"), RemoteEditEvent::Conflict { .. }));
    // Discard my changes: theirs comes down over the copy, and saves go on from it.
    h.resolve(EditChoice::Discard);
    assert_eq!(h.next_edit("discard"), RemoteEditEvent::Reloaded { id: ID, host: host() });
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "theirs, longer\n");
    save(&local, "theirs, edited\n");
    assert!(matches!(h.next_edit("a save after discarding"), RemoteEditEvent::Saved { .. }));
    assert_eq!(h.server("notes.txt"), "theirs, edited\n");
    h.stop(&local);
}

#[test]
fn replace_uploads_over_the_servers_change() {
    let Some(mut h) = Harness::new("edit_replace", true) else { return };
    let local = h.start("notes.txt");
    h.elsewhere("notes.txt", "theirs\\n");
    save(&local, "mine wins\n");
    assert!(matches!(h.next_edit("the conflict"), RemoteEditEvent::Conflict { .. }));
    h.resolve(EditChoice::Replace);
    assert_eq!(
        h.next_edit("replace"),
        RemoteEditEvent::Saved { id: ID, host: host(), to: h.scratch.join("notes.txt") }
    );
    assert_eq!(h.server("notes.txt"), "mine wins\n");
    save(&local, "and again\n");
    assert!(matches!(h.next_edit("the next save"), RemoteEditEvent::Saved { .. }), "no conflict after replacing");
    h.stop(&local);
}

#[test]
fn disconnecting_ends_the_edit_and_without_a_connection_none_starts() {
    let Some(mut h) = Harness::new("edit_disconnect", true) else { return };
    let local = h.start("notes.txt");
    h.backend.send(Command::Disconnect { host: host() });
    assert_eq!(h.next_edit("the edit to close"), RemoteEditEvent::Closed { id: ID, host: host() });
    assert!(!local.exists(), "its copy goes");
    let Some(mut h) = Harness::new("edit_offline", false) else { return };
    h.edit(RemoteEditCommand::Start { id: ID, host: host(), path: h.scratch.join("notes.txt") });
    let lost =
        RemoteEditEvent::Failed { id: ID, host: host(), stage: EditStage::Download, error: Error::ConnectionLost };
    assert_eq!(h.next_edit("the start to fail"), lost);
}
