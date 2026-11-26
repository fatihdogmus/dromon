//! Transfers end to end against the sshd fixture (`dromon-testkit`: the Docker image, or the
//! native sshd): the real service thread (`service::start`)
//! with its connection tasks and transfer engine, the system `ssh`, and SFTP to the fixture.
//! Commands go in through the service's channel as the UI sends them; the `Transfers`
//! snapshots coming back are what the popover would show.
//!
//! Each test writes its own ssh config (key-auth aliases `fx-key` and `fx-key-2` for the same
//! user, with a private `known_hosts` that accepts the fixture's key), downloads into its own
//! temporary folder and uploads into its own scratch folder on the server (in the key user's
//! home). Tests across hosts connect both aliases: two ssh connections, two `HostId`s, one
//! server.
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails).
//!
//! Windows: the same tests with the inbox `ssh.exe`, against the Docker image in WSL2 (CI); the
//! key copy gets a current-user-only ACL (`dromon_testkit::copy_private_key`) and the config
//! names `NUL` for "no file".

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard, mpsc};
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::{Backend, Command, ServiceEvent, TransferCommand};
use dromon_core::{EntryKind, HostId, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key, file_sha1};
use dromon_transfer::{ConflictChoice, JobId, JobKind, JobRequest, JobView, RemoteItem, Reveal, TaskState};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);

fn host() -> HostId {
    HostId("fx-key".into())
}

/// A second alias for the same server and user: "another host" with its own connection.
fn host2() -> HostId {
    HostId("fx-key-2".into())
}

/// [`Fixture::kill_sessions`] ends every session of the key user, every other test's too: the
/// tests that drop connections hold this for writing, every other harness for reading, so they
/// never run beside another test of this binary.
static SESSIONS: RwLock<()> = RwLock::new(());

#[allow(dead_code)] // held, never read
enum SessionsGuard {
    Shared(RwLockReadGuard<'static, ()>),
    Exclusive(RwLockWriteGuard<'static, ()>),
}

impl SessionsGuard {
    fn shared() -> SessionsGuard {
        SessionsGuard::Shared(SESSIONS.read().unwrap_or_else(|e| e.into_inner()))
    }

    fn exclusive() -> SessionsGuard {
        SessionsGuard::Exclusive(SESSIONS.write().unwrap_or_else(|e| e.into_inner()))
    }
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
    /// Every connection-log line (ssh's stderr among them) and prompt, for failure messages.
    log: Vec<String>,
    /// Declared last: released once the service is told to stop.
    _sessions: SessionsGuard,
}

impl Harness {
    /// Connected to `fx-key` only.
    fn new(name: &str) -> Option<Harness> {
        Harness::connected(name, &[host()])
    }

    /// Connected to every host in `hosts`.
    fn connected(name: &str, hosts: &[HostId]) -> Option<Harness> {
        Harness::start(name, hosts, SessionsGuard::shared())
    }

    /// [`Harness::connected`] for a test that drops the key user's sessions: no other test of
    /// this binary runs meanwhile.
    fn dropping(name: &str, hosts: &[HostId]) -> Option<Harness> {
        Harness::start(name, hosts, SessionsGuard::exclusive())
    }

    fn start(name: &str, hosts: &[HostId], sessions: SessionsGuard) -> Option<Harness> {
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
            hosts: vec![
                dromon_core::HostSpec::from_config_alias("fx-key"),
                dromon_core::HostSpec::from_config_alias("fx-key-2"),
            ],
            without_env: vec!["SSH_AUTH_SOCK".into()],
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let scratch = fx.path(Role::Key, &format!("scratch/transfers-{name}-{}", std::process::id()));
        // `exec` runs as root: the folder goes to the user the app signs in as.
        let make = format!("rm -rf '{0}' && mkdir -p '{0}' && chown {1} '{0}'", scratch.as_str(), fx.owner(Role::Key));
        fx.exec(&["sh", "-c", &make]).unwrap();
        let mut h = Harness { fx, dir, backend, events, jobs: Vec::new(), scratch, log: Vec::new(), _sessions: sessions };
        for want in hosts {
            h.backend.send(Command::Connect { host: want.clone() });
            h.wait_for("the connection to open", |ev, _| matches!(ev, ServiceEvent::Opened { host, .. } if host == want));
        }
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
            let ev = self.events.recv_timeout(left).unwrap_or_else(|_| {
                panic!("timed out waiting for {what}; jobs: {:#?}\nconnection log:\n{}", self.jobs, self.log.join("\n"))
            });
            match &ev {
                ServiceEvent::Transfers(jobs) => self.jobs = jobs.clone(),
                ServiceEvent::Log { host, line } => self.log.push(format!("[{}] {line}", host.0)),
                // Key auth and accept-new: nothing should ask. Kept for the failure message.
                ServiceEvent::Prompt(p) => self.log.push(format!("unexpected prompt: {p:?}")),
                ServiceEvent::Status { status: dromon_core::ConnStatus::Failed(why), .. } => {
                    panic!("the connection failed: {why}\nconnection log:\n{}", self.log.join("\n"))
                }
                _ => {}
            }
            if done(&ev, &self.jobs) {
                return;
            }
        }
    }

    /// Waits until the jobs satisfy `done`, checking the latest snapshot first: the engine sends
    /// a snapshot only when something changed, so a state that already arrived (a small job
    /// done while the test still waited for it to appear) never comes again.
    fn wait_jobs(&mut self, what: &str, done: impl Fn(&[JobView]) -> bool) {
        if !done(&self.jobs) {
            self.wait_for(what, |_, jobs| done(jobs));
        }
    }

    /// Waits until job `id` has finished, and returns it.
    fn finished(&mut self, id: JobId) -> JobView {
        self.wait_jobs(&format!("job {} to finish", id.0), |jobs| jobs.iter().any(|j| j.id == id && j.state.is_finished()));
        self.job(id)
    }

    fn job(&self, id: JobId) -> JobView {
        self.jobs.iter().find(|j| j.id == id).cloned().expect("the job is listed")
    }

    /// The newest job's id once it shows up (jobs are newest first).
    fn newest(&mut self, after: Option<JobId>) -> JobId {
        self.wait_jobs("a new job", |jobs| jobs.first().is_some_and(|j| Some(j.id) != after));
        self.jobs[0].id
    }

    fn remote(&self, cmd: &str) -> String {
        self.fx.exec(&["sh", "-c", cmd]).unwrap()
    }

    /// The seed's mixed folder in the key user's home.
    fn mixed(&self) -> String {
        self.fx.path(Role::Key, "fixture/mixed").as_str().to_owned()
    }
}

fn write_ssh_config(fx: &Fixture, dir: &Path) -> PathBuf {
    let key = dir.join("id_ed25519");
    copy_private_key(&fx.key(), &key).unwrap();
    let mut config = String::new();
    for alias in ["fx-key", "fx-key-2"] {
        writeln!(config, "Host {alias}\n    HostName {}\n    Port {}\n    User {}", fx.host(), fx.port(), fx.user(Role::Key)).unwrap();
        writeln!(config, "    IdentityFile {}\n    IdentitiesOnly yes\n    PreferredAuthentications publickey", key.display()).unwrap();
        writeln!(config, "    UserKnownHostsFile {}\n    GlobalKnownHostsFile {NULL_DEVICE}", dir.join("known_hosts").display()).unwrap();
        writeln!(config, "    StrictHostKeyChecking accept-new\n    UpdateHostKeys no").unwrap();
    }
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
    let mixed = h.mixed();
    let items = vec![item(&format!("{mixed}/file-001.txt"), EntryKind::File), item(&format!("{mixed}/dir-01"), EntryKind::Dir)];
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
    let items = vec![item(&format!("{mixed}/file-001.txt"), EntryKind::File)];
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
    h.wait_jobs("the name clash", |jobs| jobs.iter().any(|j| j.id == again && j.conflict.is_some()));
    let clash = h.job(again).conflict.unwrap();
    assert_eq!((clash.name.as_str(), clash.existing_is_dir, clash.more), ("hello.txt", false, 0));
    // Both sides are real: the server's file from the same lstat that found the clash, the
    // local one from its metadata. The upload kept the local date, so the dates agree.
    let local_mtime = std::fs::metadata(local.join("hello.txt")).unwrap().modified().unwrap();
    let local_mtime = local_mtime.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert_eq!(clash.existing.size, Some("hello from dromon\n".len() as u64));
    assert_eq!(clash.incoming.size, Some(std::fs::metadata(local.join("hello.txt")).unwrap().len()));
    assert_eq!(clash.incoming.mtime, Some(local_mtime));
    assert_eq!(clash.existing.mtime, Some(h.mtime_of("hello.txt")), "the server's date, as the shell sees it");
    assert_eq!(clash.existing.mtime, clash.incoming.mtime);
    h.transfer(TransferCommand::Resolve { job: again, choice: ConflictChoice::KeepBoth, apply_all: false });
    assert_eq!(h.finished(again).state, TaskState::Done);
    assert_eq!(h.remote(&format!("cat '{dir}/hello (1).txt'")), "hello from dromon\n");
}

#[test]
fn a_dropped_connection_resumes_the_download_where_it_stopped() {
    let Some(mut h) = Harness::dropping("resume", &[host()]) else { return };
    let src = h.scratch.join("big.bin");
    h.remote(&format!("head -c 67108864 /dev/urandom > '{}'", src.as_str()));
    // SHA-1 on the server and here (no hashing tool needed on the host: macOS has no sha256sum).
    let want = h.fx.sha1(src.as_str()).unwrap();

    let into = h.downloads();
    let items = vec![RemoteItem { path: src.clone(), kind: EntryKind::File, size: 64 << 20 }];
    h.transfer(TransferCommand::Start(JobRequest::Download { host: host(), items, into: into.clone() }));
    let id = h.newest(None);
    // Some bytes in, the server drops every session of the user; the connection comes back by
    // itself (key auth needs nobody) and the file carries on from its part file.
    h.wait_jobs("the first bytes", |jobs| jobs.iter().any(|j| j.id == id && j.progress.done > 0));
    let before = h.job(id).progress.done;
    assert!(h.fx.kill_sessions(Role::Key).unwrap() > 0);
    h.wait_jobs("the job to wait for the connection", |jobs| jobs.iter().any(|j| j.id == id && j.waiting_for_connection));
    h.wait_jobs("the job to move again", |jobs| jobs.iter().any(|j| j.id == id && !j.waiting_for_connection));
    let job = h.finished(id);
    assert_eq!(job.state, TaskState::Done, "{job:#?}");
    assert!(before < 64 << 20, "the drop came before the end");

    assert_eq!(file_sha1(&into.join("big.bin")).unwrap(), want);
}

impl Harness {
    /// Runs `script` on the server with `$D` this test's scratch folder, then hands everything
    /// in it to the user the app signs in as.
    fn seed(&self, script: &str) {
        let d = self.scratch.as_str();
        self.remote(&format!("D='{d}'; set -e; {script}; chown -R {} \"$D\"", self.fx.owner(Role::Key)));
    }

    /// A path in the scratch folder.
    fn at(&self, rel: &str) -> RemotePath {
        self.scratch.join(rel)
    }

    /// What `cat` says of scratch paths.
    fn cat(&self, rels: &[&str]) -> String {
        let files: Vec<String> = rels.iter().map(|r| format!("'{}'", self.at(r))).collect();
        self.remote(&format!("cat {}", files.join(" ")))
    }

    /// Whether a scratch path exists.
    fn there(&self, rel: &str) -> bool {
        self.remote(&format!("test -e '{}' && echo yes || echo no", self.at(rel))).trim() == "yes"
    }

    /// A scratch path's mtime, in seconds (perl's `stat`: the same on Linux and macOS).
    fn mtime_of(&self, rel: &str) -> i64 {
        let out = self.remote(&format!("perl -e 'print((stat shift)[9])' '{}'", self.at(rel)));
        out.trim().parse().unwrap_or_else(|_| panic!("mtime of {rel}: {out:?}"))
    }

    /// Starts `req` and waits for the new job to finish (or to ask about a clash, with
    /// `clash`); returns it.
    fn run(&mut self, req: JobRequest, clash: bool) -> JobView {
        let before = self.jobs.first().map(|j| j.id);
        self.transfer(TransferCommand::Start(req));
        let id = self.newest(before);
        if clash {
            self.wait_jobs("the name clash", |jobs| jobs.iter().any(|j| j.id == id && j.conflict.is_some()));
            return self.job(id);
        }
        self.finished(id)
    }
}

fn ritem(h: &Harness, rel: &str, kind: EntryKind) -> RemoteItem {
    RemoteItem { path: h.at(rel), kind, size: 0 }
}

#[test]
fn copies_and_moves_on_one_host_keep_bytes_and_dates_and_a_clash_can_replace() {
    let Some(mut h) = Harness::new("same-host") else { return };
    h.seed(
        "mkdir -p \"$D/src/tree/sub\" \"$D/src/tree/empty\" \"$D/dst\"; \
         printf 'alpha\\n' >\"$D/src/a.txt\"; printf 'bee\\n' >\"$D/src/tree/sub/b.txt\"; \
         TZ=UTC0 touch -m -t 202009131226.40 \"$D/src/a.txt\" \"$D/src/tree/sub/b.txt\"",
    );
    let into = h.at("dst");

    // Copy (copy-data on the fixture): bytes, mtimes and the empty folder arrive.
    let items = vec![ritem(&h, "src/a.txt", EntryKind::File), ritem(&h, "src/tree", EntryKind::Dir)];
    let job = h.run(JobRequest::RemoteCopy { host: host(), items, into: into.clone() }, false);
    assert_eq!(job.state, TaskState::Done, "{job:#?}");
    assert_eq!((job.kind, job.files_done, job.reveal.clone()), (JobKind::RemoteCopy, 2, Some(Reveal::Remote(host(), into.join("a.txt")))));
    assert_eq!(h.cat(&["dst/a.txt", "dst/tree/sub/b.txt"]), "alpha\nbee\n");
    assert_eq!((h.mtime_of("dst/a.txt"), h.mtime_of("dst/tree/sub/b.txt")), (1_600_000_000, 1_600_000_000));
    assert!(h.there("dst/tree/empty") && h.there("src/a.txt"), "the empty folder came along; the source stays");

    // A copy into its own folder keeps both, without asking.
    let job = h.run(JobRequest::RemoteCopy { host: host(), items: vec![ritem(&h, "src/a.txt", EntryKind::File)], into: h.at("src") }, false);
    assert_eq!((&job.state, job.conflict.as_ref()), (&TaskState::Done, None));
    assert_eq!(h.cat(&["src/a (1).txt"]), "alpha\n");

    // Move onto the copies: the clash is answered Replace; the file is replaced, the folder merged.
    h.seed("printf 'new alpha\\n' >\"$D/src/a.txt\"; printf 'cee\\n' >\"$D/src/tree/sub/c.txt\"");
    // The picked item carries its listed size (the helper's is 0).
    let new_alpha = RemoteItem { size: "new alpha\n".len() as u64, ..ritem(&h, "src/a.txt", EntryKind::File) };
    let items = vec![new_alpha, ritem(&h, "src/tree", EntryKind::Dir)];
    let job = h.run(JobRequest::Move { host: host(), items, into: into.clone() }, true);
    let clash = job.conflict.unwrap();
    assert_eq!((clash.name.as_str(), clash.existing_is_dir, clash.more, job.kind), ("a.txt", false, 1, JobKind::Move));
    // The copy at the destination ("alpha\n", dated 2020) against the file about to replace it.
    assert_eq!((clash.existing.size, clash.existing.mtime), (Some(6), Some(1_600_000_000)));
    assert_eq!(clash.incoming.size, Some("new alpha\n".len() as u64));
    assert!(clash.incoming.mtime.is_some_and(|t| t > 1_600_000_000), "{clash:?}");
    h.transfer(TransferCommand::Resolve { job: job.id, choice: ConflictChoice::Replace, apply_all: true });
    let job = h.finished(job.id);
    assert_eq!((&job.state, job.files_done, job.files_total), (&TaskState::Done, 2, 2), "{job:#?}");
    assert_eq!(h.cat(&["dst/a.txt", "dst/tree/sub/b.txt", "dst/tree/sub/c.txt"]), "new alpha\nbee\ncee\n");
    assert!(!h.there("src/a.txt") && !h.there("src/tree"), "moved away");
    assert_eq!(h.remote(&format!("find '{}' -name '*.dromon-part' | wc -l", h.scratch)).trim(), "0");

    // A move into its own folder does nothing.
    let job = h.run(JobRequest::Move { host: host(), items: vec![ritem(&h, "dst/a.txt", EntryKind::File)], into: into.clone() }, false);
    assert_eq!((&job.state, job.skipped.as_slice()), (&TaskState::Done, &["a.txt: already there".to_owned()][..]));
}

#[test]
fn copies_and_moves_between_two_hosts_stream_through_the_app() {
    let Some(mut h) = Harness::connected("cross-host", &[host(), host2()]) else { return };
    h.seed(
        "mkdir -p \"$D/a/dir/inner\" \"$D/a/gone/empty\" \"$D/b\"; head -c 3145728 /dev/urandom >\"$D/a/x.bin\"; \
         printf 'one\\n' >\"$D/a/dir/inner/one.txt\"; printf 'two\\n' >\"$D/a/gone/two.txt\"; \
         TZ=UTC0 touch -m -t 202009131226.40 \"$D/a/x.bin\"",
    );
    let want = h.fx.sha1(h.at("a/x.bin").as_str()).unwrap();
    let into = h.at("b");

    // Copy from fx-key to fx-key-2: listed under the destination host.
    let items = vec![ritem(&h, "a/x.bin", EntryKind::File), ritem(&h, "a/dir", EntryKind::Dir)];
    let job = h.run(JobRequest::CrossHostCopy { from: host(), items, to: host2(), into: into.clone() }, false);
    assert_eq!(job.state, TaskState::Done, "{job:#?}");
    assert_eq!((job.kind, &job.host, job.files_done), (JobKind::CrossHostCopy, &host2(), 2));
    assert_eq!(job.reveal, Some(Reveal::Remote(host2(), into.join("x.bin"))));
    assert_eq!(h.fx.sha1(h.at("b/x.bin").as_str()).unwrap(), want);
    assert_eq!(h.mtime_of("b/x.bin"), 1_600_000_000);
    assert_eq!(h.cat(&["b/dir/inner/one.txt"]), "one\n");
    assert!(h.there("a/x.bin"), "a copy keeps the source");

    // Move: each file checked on the destination, then deleted; the emptied folders go too.
    let job = h.run(JobRequest::CrossHostMove { from: host(), items: vec![ritem(&h, "a/gone", EntryKind::Dir)], to: host2(), into: into.clone() }, false);
    assert_eq!((&job.state, job.kind), (&TaskState::Done, JobKind::CrossHostMove), "{job:#?}");
    assert_eq!(h.cat(&["b/gone/two.txt"]), "two\n");
    assert!(h.there("b/gone/empty"));
    assert!(!h.there("a/gone"), "the source folder is gone with its empty subfolder");

    // Moving x.bin again clashes with the copy; Keep both lands it beside, then the source goes.
    let job = h.run(JobRequest::CrossHostMove { from: host(), items: vec![ritem(&h, "a/x.bin", EntryKind::File)], to: host2(), into }, true);
    h.transfer(TransferCommand::Resolve { job: job.id, choice: ConflictChoice::KeepBoth, apply_all: false });
    assert_eq!(h.finished(job.id).state, TaskState::Done);
    assert_eq!(h.fx.sha1(h.at("b/x (1).bin").as_str()).unwrap(), want);
    assert!(!h.there("a/x.bin"));
    assert_eq!(h.remote(&format!("find '{}' -name '*.dromon-part' | wc -l", h.scratch)).trim(), "0");
}

/// Stress: rounds of jobs of every kind on one host and across two (clashes answered Keep
/// both, folders to create and to tidy), with every session of the key user dropped at a
/// different moment of each round; every job must still end, and end well. Rounds:
/// `DROMON_DROP_ROUNDS` (default 12).
#[test]
#[ignore = "stress, about a minute: cargo test -p dromon-app --test transfers_e2e -- --ignored"]
fn every_job_ends_when_the_connections_drop_at_any_moment() {
    let Some(mut h) = Harness::dropping("drops", &[host(), host2()]) else { return };
    let rounds: u64 = std::env::var("DROMON_DROP_ROUNDS").ok().and_then(|n| n.parse().ok()).unwrap_or(12);
    let local = h.dir.path().join("out");
    std::fs::create_dir_all(local.join("tree/sub")).unwrap();
    std::fs::write(local.join("up.txt"), "up\n").unwrap();
    std::fs::write(local.join("tree/sub/deep.txt"), "deep\n").unwrap();
    let mut next = 1;
    for round in 0..rounds {
        let r = format!("r{round}");
        h.seed(&format!(
            "R=\"$D/{r}\"; mkdir -p \"$R/a/dir/inner\" \"$R/a/gone/empty\" \"$R/b\" \"$R/c\"; \
             head -c 1048576 /dev/urandom >\"$R/a/x.bin\"; cp \"$R/a/x.bin\" \"$R/b/x.bin\"; \
             head -c 524288 /dev/urandom >\"$R/a/m.bin\"; cp \"$R/a/m.bin\" \"$R/b/m.bin\"; \
             printf 'one\\n' >\"$R/a/dir/inner/one.txt\"; printf 'two\\n' >\"$R/a/gone/two.txt\""
        ));
        let at = |rel: &str, kind| ritem(&h, &format!("{r}/{rel}"), kind);
        let (b, c) = (h.at(&format!("{r}/b")), h.at(&format!("{r}/c")));
        let reqs = vec![
            JobRequest::CrossHostCopy {
                from: host(),
                items: vec![at("a/x.bin", EntryKind::File), at("a/dir", EntryKind::Dir)],
                to: host2(),
                into: b.clone(),
            },
            JobRequest::CrossHostMove { from: host(), items: vec![at("a/m.bin", EntryKind::File)], to: host2(), into: b.clone() },
            JobRequest::CrossHostMove { from: host(), items: vec![at("a/gone", EntryKind::Dir)], to: host2(), into: b },
            JobRequest::RemoteCopy { host: host(), items: vec![at("a/dir", EntryKind::Dir)], into: c.clone() },
            JobRequest::Upload { host: host2(), items: vec![local.join("up.txt"), local.join("tree")], into: c },
            JobRequest::Download {
                host: host(),
                items: vec![at("a/x.bin", EntryKind::File), at("a/dir", EntryKind::Dir)],
                into: h.downloads().join(&r),
            },
        ];
        let ids: Vec<JobId> = (next..next + reqs.len() as u64).map(JobId).collect();
        next += reqs.len() as u64;
        for req in reqs {
            h.transfer(TransferCommand::Start(req));
        }
        std::thread::sleep(Duration::from_millis(round * 53 % 700));
        h.fx.kill_sessions(Role::Key).unwrap();

        let deadline = Instant::now() + WAIT;
        loop {
            let ours: Vec<&JobView> = h.jobs.iter().filter(|j| ids.contains(&j.id)).collect();
            if ours.len() == ids.len() && ours.iter().all(|j| j.state.is_finished()) {
                break;
            }
            let clashes: Vec<JobId> = ours.iter().filter(|j| j.conflict.is_some()).map(|j| j.id).collect();
            for job in clashes {
                h.transfer(TransferCommand::Resolve { job, choice: ConflictChoice::KeepBoth, apply_all: true });
            }
            assert!(Instant::now() < deadline, "round {round}: jobs {ids:?} didn't end; jobs: {:#?}\nconnection log:\n{}", h.jobs, h.log.join("\n"));
            h.wait_for(&format!("round {round}: jobs {ids:?} to end"), |ev, _| matches!(ev, ServiceEvent::Transfers(_)));
        }
        for id in &ids {
            let job = h.job(*id);
            assert_eq!(job.state, TaskState::Done, "round {round}: {job:#?}\nconnection log:\n{}", h.log.join("\n"));
        }
        // Everything once, in place, no part files left.
        let tree = h.remote(&format!("cd '{}' && find . | LC_ALL=C sort", h.at(&r)));
        let want = [
            ".", "./a", "./a/dir", "./a/dir/inner", "./a/dir/inner/one.txt", "./a/x.bin", "./b", "./b/dir", "./b/dir/inner",
            "./b/dir/inner/one.txt", "./b/gone", "./b/gone/empty", "./b/gone/two.txt", "./b/m (1).bin", "./b/m.bin",
            "./b/x (1).bin", "./b/x.bin", "./c", "./c/dir", "./c/dir/inner", "./c/dir/inner/one.txt", "./c/tree",
            "./c/tree/sub", "./c/tree/sub/deep.txt", "./c/up.txt",
        ];
        assert_eq!(tree.lines().collect::<Vec<_>>(), want, "round {round}");
        let downloaded: Vec<PathBuf> = walk(&h.downloads().join(&r)).into_iter().collect();
        assert_eq!(downloaded.len(), 4, "round {round}: {downloaded:?}");
    }
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
