//! Quick preview reads (`Command::Preview`) end to end against the sshd fixture
//! (`dromon-testkit`: the Docker image, or the native sshd): the real service thread with its
//! connection tasks, the system `ssh`, and SFTP to the fixture. Commands go in through the
//! service's channel as the UI sends them; the `Preview` events coming back are what the panel
//! would show.
//!
//! Each test writes its own ssh config (one key-auth alias, `fx-key`, with a private
//! `known_hosts`) and works in its own scratch folder on the server, whose files it makes with
//! known contents: a text file's head and a log's tail come back byte for byte, and a large
//! file gives only the range asked for.
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::preview::TEXT_LIMIT;
use dromon_app::state::{Backend, Command, PreviewId, PreviewRange, ServiceEvent};
use dromon_core::{ConnStatus, Error, HostId, RemotePath};
use dromon_testkit::{Fixture, NULL_DEVICE, Role, copy_private_key};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);

/// Lines in the made-up text files: "line 0000001\n" (13 bytes each), 520,000 bytes in all,
/// more than [`TEXT_LIMIT`].
const LINES: usize = 40_000;
/// The large file: 64 MiB of nothing (sparse), then this.
const BIG: u64 = 64 * 1024 * 1024;
const END: &[u8] = b"THE END\n";

fn host() -> HostId {
    HostId("fx-key".into())
}

/// What the made-up text files hold.
fn lines() -> Vec<u8> {
    (1..=LINES).map(|i| format!("line {i:07}\n")).collect::<String>().into_bytes()
}

#[derive(Clone)]
struct TestSink(mpsc::Sender<ServiceEvent>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send(ev);
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    backend: service::ChannelBackend,
    events: mpsc::Receiver<ServiceEvent>,
    /// This test's folder on the server.
    scratch: RemotePath,
    /// Every connection-log line (ssh's stderr among them) and prompt, for failure messages.
    log: Vec<String>,
}

impl Harness {
    /// The service with a fresh scratch folder holding the test's files; connected when
    /// `connect`.
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
        let scratch = fx.path(Role::Key, &format!("scratch/preview-{name}-{}", std::process::id()));
        // `exec` runs as root: the files go to the user the app signs in as. `dd … seek=`
        // makes the large file sparse, on every backend's sh.
        let make = format!(
            "rm -rf '{d}' && mkdir -p '{d}' && cd '{d}' \
             && awk 'BEGIN {{ for (i = 1; i <= {LINES}; i++) printf \"line %07d\\n\", i }}' > notes.txt \
             && cp notes.txt app.log && printf 'hello\\n' > short.txt \
             && dd if=/dev/zero of=big.bin bs=1 count=0 seek={BIG} 2>/dev/null && printf 'THE END\\n' >> big.bin \
             && chown -R {owner} '{d}'",
            d = scratch.as_str(),
            owner = fx.owner(Role::Key),
        );
        fx.exec(&["sh", "-c", &make]).unwrap();
        let mut h = Harness { _dir: dir, backend, events, scratch, log: Vec::new() };
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

    /// Previews `range` of the scratch file `name`: the bytes and the file's size, or why not.
    fn preview(&mut self, id: u64, name: &str, range: PreviewRange) -> Result<(Vec<u8>, u64), Error> {
        let id = PreviewId(id);
        let path = self.scratch.join(name);
        self.backend.send(Command::Preview { id, host: host(), path, range });
        self.wait_for("the preview", |ev| match ev {
            ServiceEvent::Preview { id: i, bytes, total, .. } if *i == id => Some(Ok((bytes.0.to_vec(), *total))),
            ServiceEvent::PreviewFailed { id: i, error, .. } if *i == id => Some(Err(error.clone())),
            _ => None,
        })
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
fn a_text_files_head_and_a_logs_tail_come_back_byte_for_byte() {
    let Some(mut h) = Harness::new("preview_text", true) else { return };
    let all = lines();
    let limit = TEXT_LIMIT as usize;
    let (head, total) = h.preview(1, "notes.txt", PreviewRange::Head(TEXT_LIMIT)).unwrap();
    assert_eq!(total, all.len() as u64);
    assert!(head == all[..limit], "the first {limit} bytes, exactly ({} came)", head.len());
    let (tail, total) = h.preview(2, "app.log", PreviewRange::Tail(TEXT_LIMIT)).unwrap();
    assert_eq!(total, all.len() as u64);
    assert!(tail == all[all.len() - limit..], "the last {limit} bytes, exactly ({} came)", tail.len());
    // A file shorter than the limit comes whole, either way.
    assert_eq!(h.preview(3, "short.txt", PreviewRange::Head(TEXT_LIMIT)), Ok((b"hello\n".to_vec(), 6)));
    assert_eq!(h.preview(4, "short.txt", PreviewRange::Tail(TEXT_LIMIT)), Ok((b"hello\n".to_vec(), 6)));
}

#[test]
fn a_large_file_gives_only_the_range_asked_for() {
    let Some(mut h) = Harness::new("preview_large", true) else { return };
    let size = BIG + END.len() as u64;
    let (tail, total) = h.preview(1, "big.bin", PreviewRange::Tail(TEXT_LIMIT)).unwrap();
    assert_eq!((tail.len() as u64, total), (TEXT_LIMIT, size), "the last 256 KB of 64 MB read, no more");
    assert!(tail.ends_with(END) && tail[..tail.len() - END.len()].iter().all(|b| *b == 0));
    let (head, total) = h.preview(2, "big.bin", PreviewRange::Head(TEXT_LIMIT)).unwrap();
    assert_eq!((head.len() as u64, total), (TEXT_LIMIT, size));
    assert!(head.iter().all(|b| *b == 0));
}

#[test]
fn a_missing_file_says_so_and_without_a_connection_it_fails_at_once() {
    let Some(mut h) = Harness::new("preview_missing", true) else { return };
    let missing = h.preview(1, "gone.txt", PreviewRange::Head(TEXT_LIMIT));
    assert!(matches!(missing, Err(Error::NotFound(_))), "{missing:?}");
    let Some(mut h) = Harness::new("preview_offline", false) else { return };
    assert_eq!(h.preview(1, "notes.txt", PreviewRange::Head(TEXT_LIMIT)), Err(Error::ConnectionLost));
}
