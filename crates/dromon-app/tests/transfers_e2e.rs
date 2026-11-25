//! Transfers end to end against the Docker sshd: the real service thread (`service::start`)
//! with its connection tasks and transfer engine, the system `ssh`, and SFTP to the fixture.
//! Commands go in through the service's channel as the UI sends them; the `Transfers`
//! snapshots coming back are what the popover would show.
//!
//! Each test writes its own ssh config (one key-auth alias, `fx-key`, with a private
//! `known_hosts` that accepts the fixture's key), downloads into its own temporary folder and
//! uploads into its own scratch folder on the server.
//!
//! Without Docker every test prints a skip line and passes (`DROMON_REQUIRE_DOCKER=1` fails).

#![cfg(unix)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{Backend, Command, ServiceEvent, TransferCommand};
use dromon_core::{EntryKind, HostId, RemotePath};
use dromon_testkit::{Fixture, USER_KEY};
use dromon_transfer::{ConflictChoice, JobId, JobKind, JobRequest, JobView, RemoteItem, Reveal, TaskState};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);
const MIXED: &str = "/home/key/fixture/mixed";

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
    dir: tempfile::TempDir,
    backend: service::ChannelBackend,
    events: mpsc::Receiver<ServiceEvent>,
    /// The engine's latest snapshot.
    jobs: Vec<JobView>,
    /// This test's folder on the server.
    scratch: RemotePath,
}

impl Harness {
    fn new(name: &str) -> Option<Harness> {
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
            hosts: vec![dromon_core::HostSpec::from_config_alias("fx-key")],
            without_env: vec!["SSH_AUTH_SOCK".into()],
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let scratch = RemotePath::new(&format!("/home/key/scratch/transfers-{name}-{}", std::process::id()));
        // `exec` runs as root: the folder goes to the user the app signs in as.
        let make = format!("rm -rf '{0}' && mkdir -p '{0}' && chown {USER_KEY}:{USER_KEY} '{0}'", scratch.as_str());
        fx.exec(&["sh", "-c", &make]).unwrap();
        let mut h = Harness { fx, dir, backend, events, jobs: Vec::new(), scratch };
        h.backend.send(Command::Connect { host: host() });
        h.wait_for("the connection to open", |ev, _| matches!(ev, ServiceEvent::Opened { .. }));
        Some(h)
    }

    fn downloads(&self) -> PathBuf {
        self.dir.path().join("Downloads")
    }

    fn transfer(&mut self, cmd: TransferCommand) {
        self.backend.send(Command::Transfer(cmd));
    }

    /// Applies events until `done` says so; every `Transfers` snapshot is kept in `jobs`.
    fn wait_for(&mut self, what: &str, mut done: impl FnMut(&ServiceEvent, &[JobView]) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = self.events.recv_timeout(left).unwrap_or_else(|_| panic!("timed out waiting for {what}; jobs: {:#?}", self.jobs));
            if let ServiceEvent::Transfers(jobs) = &ev {
                self.jobs = jobs.clone();
            }
            if let ServiceEvent::Status { status: dromon_core::ConnStatus::Failed(why), .. } = &ev {
                panic!("the connection failed: {why}");
            }
            if done(&ev, &self.jobs) {
                return;
            }
        }
    }

    /// Waits until job `id` has finished, and returns it.
    fn finished(&mut self, id: JobId) -> JobView {
        self.wait_for(&format!("job {} to finish", id.0), |_, jobs| jobs.iter().any(|j| j.id == id && j.state.is_finished()));
        self.job(id)
    }

    fn job(&self, id: JobId) -> JobView {
        self.jobs.iter().find(|j| j.id == id).cloned().expect("the job is listed")
    }

    /// The newest job's id once it shows up (jobs are newest first).
    fn newest(&mut self, after: Option<JobId>) -> JobId {
        self.wait_for("a new job", |_, jobs| jobs.first().is_some_and(|j| Some(j.id) != after));
        self.jobs[0].id
    }

    fn remote(&self, cmd: &str) -> String {
        self.fx.exec(&["sh", "-c", cmd]).unwrap()
    }
}

fn write_ssh_config(fx: &Fixture, dir: &Path) -> PathBuf {
    let key = dir.join("id_ed25519");
    std::fs::copy(fx.key(), &key).unwrap();
    let mut config = String::new();
    writeln!(config, "Host fx-key\n    HostName {}\n    Port {}\n    User {USER_KEY}", fx.host(), fx.port()).unwrap();
    writeln!(config, "    IdentityFile {}\n    IdentitiesOnly yes\n    PreferredAuthentications publickey", key.display()).unwrap();
    writeln!(config, "    UserKnownHostsFile {}\n    GlobalKnownHostsFile /dev/null", dir.join("known_hosts").display()).unwrap();
    writeln!(config, "    StrictHostKeyChecking accept-new\n    UpdateHostKeys no").unwrap();
    let path = dir.join("ssh_config");
    std::fs::write(&path, config).unwrap();
    path
}

fn item(path: &str, kind: EntryKind) -> RemoteItem {
    RemoteItem { path: RemotePath::new(path), kind, size: 0 }
}

fn mtime(path: &Path) -> i64 {
    let m = std::fs::metadata(path).unwrap().modified().unwrap();
    m.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

#[test]
fn downloads_land_in_the_folder_with_their_bytes_and_dates() {
    let Some(mut h) = Harness::new("downloads") else { return };
    let into = h.downloads();
    let items = vec![item(&format!("{MIXED}/file-001.txt"), EntryKind::File), item(&format!("{MIXED}/dir-01"), EntryKind::Dir)];
    h.transfer(TransferCommand::Start(JobRequest::Download { host: host(), items, into: into.clone() }));
    let id = h.newest(None);
    let job = h.finished(id);
    assert_eq!(job.state, TaskState::Done, "{job:#?}");
    assert_eq!((job.kind, job.title.as_str(), job.files_done), (JobKind::Download, "2 items", 2));
    assert_eq!(job.reveal, Some(Reveal::Local(into.join("file-001.txt"))));

    let file = into.join("file-001.txt");
    assert_eq!(std::fs::read(&file).unwrap(), vec![0u8; 100]);
    assert_eq!(mtime(&file), 1_577_934_245, "the server's date is kept");
    assert_eq!(std::fs::read_to_string(into.join("dir-01/inside.txt")).unwrap(), "inside\n");
    let leftovers: Vec<_> = walk(&into).into_iter().filter(|p| p.to_string_lossy().ends_with(".dromon-part")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // The same file again: Chrome-style name, the first one untouched.
    let items = vec![item(&format!("{MIXED}/file-001.txt"), EntryKind::File)];
    h.transfer(TransferCommand::Start(JobRequest::Download { host: host(), items, into: into.clone() }));
    let again = h.newest(Some(id));
    assert_eq!(h.finished(again).state, TaskState::Done);
    assert_eq!(std::fs::read(into.join("file-001 (1).txt")).unwrap(), vec![0u8; 100]);
}

#[test]
fn uploads_land_on_the_server_and_a_clash_can_keep_both() {
    let Some(mut h) = Harness::new("uploads") else { return };
    let local = h.dir.path().join("out");
    std::fs::create_dir_all(local.join("tree/a")).unwrap();
    std::fs::create_dir_all(local.join("tree/empty")).unwrap();
    std::fs::write(local.join("hello.txt"), "hello from dromon\n").unwrap();
    std::fs::write(local.join("tree/a/b.txt"), "bee\n").unwrap();
    let into = h.scratch.clone();

    let items = vec![local.join("hello.txt"), local.join("tree")];
    h.transfer(TransferCommand::Start(JobRequest::Upload { host: host(), items: items.clone(), into: into.clone() }));
    let id = h.newest(None);
    let job = h.finished(id);
    assert_eq!(job.state, TaskState::Done, "{job:#?}");
    assert_eq!(job.reveal, Some(Reveal::Remote(host(), into.join("hello.txt"))));
    let dir = into.as_str();
    assert_eq!(h.remote(&format!("cat '{dir}/hello.txt' '{dir}/tree/a/b.txt'")), "hello from dromon\nbee\n");
    assert_eq!(h.remote(&format!("test -d '{dir}/tree/empty' && echo yes")).trim(), "yes");
    assert_eq!(h.remote(&format!("find '{dir}' -name '*.dromon-part' | wc -l")).trim(), "0");

    // hello.txt again: the job waits on the clash; Keep both uploads it as "hello (1).txt".
    let items = vec![local.join("hello.txt")];
    h.transfer(TransferCommand::Start(JobRequest::Upload { host: host(), items, into: into.clone() }));
    let again = h.newest(Some(id));
    h.wait_for("the name clash", |_, jobs| jobs.iter().any(|j| j.id == again && j.conflict.is_some()));
    let clash = h.job(again).conflict.unwrap();
    assert_eq!((clash.name.as_str(), clash.existing_is_dir, clash.more), ("hello.txt", false, 0));
    h.transfer(TransferCommand::Resolve { job: again, choice: ConflictChoice::KeepBoth, apply_all: false });
    assert_eq!(h.finished(again).state, TaskState::Done);
    assert_eq!(h.remote(&format!("cat '{dir}/hello (1).txt'")), "hello from dromon\n");
}

#[test]
fn a_dropped_connection_resumes_the_download_where_it_stopped() {
    let Some(mut h) = Harness::new("resume") else { return };
    let src = h.scratch.join("big.bin");
    h.remote(&format!("head -c 67108864 /dev/urandom > '{}'", src.as_str()));
    let want = h.remote(&format!("sha256sum '{}' | cut -c1-64", src.as_str())).trim().to_owned();

    let into = h.downloads();
    let items = vec![RemoteItem { path: src.clone(), kind: EntryKind::File, size: 64 << 20 }];
    h.transfer(TransferCommand::Start(JobRequest::Download { host: host(), items, into: into.clone() }));
    let id = h.newest(None);
    // Some bytes in, the server drops every session of the user; the connection comes back by
    // itself (key auth needs nobody) and the file carries on from its part file.
    h.wait_for("the first bytes", |_, jobs| jobs.iter().any(|j| j.id == id && j.progress.done > 0));
    let before = h.job(id).progress.done;
    assert!(h.fx.kill_sessions(USER_KEY).unwrap() > 0);
    h.wait_for("the job to wait for the connection", |_, jobs| jobs.iter().any(|j| j.id == id && j.waiting_for_connection));
    h.wait_for("the job to move again", |_, jobs| jobs.iter().any(|j| j.id == id && !j.waiting_for_connection));
    let job = h.finished(id);
    assert_eq!(job.state, TaskState::Done, "{job:#?}");
    assert!(before < 64 << 20, "the drop came before the end");

    let got = std::process::Command::new("sha256sum").arg(into.join("big.bin")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&got.stdout)[..64], want);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}
