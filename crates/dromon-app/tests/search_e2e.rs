//! Search in subfolders end to end against the sshd fixture (`dromon-testkit`: the Docker
//! image, or the native sshd): `Command::Search` on the real service thread, the system `ssh`
//! and SFTP, walking the seeded `fixture/mixed` and `fixture/many` (`tests/docker/seed.sh`).
//!
//! Commands go in through the service's channel as the UI sends them; the events coming back
//! are what the results would show. One key-auth alias, `fx-key`, with a private `known_hosts`.
//!
//! When the backend can't run every test prints a skip line and passes
//! (`DROMON_REQUIRE_DOCKER=1` fails).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, Config, Sink};
use dromon_app::state::search::CAP;
use dromon_app::state::{Backend, Command, Found, SearchCommand, SearchEvent, SearchId, ServiceEvent};
use dromon_core::{ConnStatus, Error, HostId, RemotePath};
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

/// How a search ended, and what it found on the way.
#[derive(Debug)]
struct Searched {
    found: Vec<Found>,
    /// The folder counts its batches reported, in order.
    progress: Vec<u64>,
    end: ServiceEvent,
}

impl Searched {
    /// `folder/name` of each result, sorted (the walk's order depends on the server's timing).
    fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = self.found.iter().map(|f| format!("{}/{}", f.folder, f.entry.name)).collect();
        out.sort();
        out
    }
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
            editing_root: None,
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

    /// Searches `root` for `query`, to the end.
    fn search(&mut self, id: u64, root: &RemotePath, query: &str, show_hidden: bool) -> Searched {
        let id = SearchId(id);
        let cmd = SearchCommand::Start { id, host: host(), root: root.clone(), query: query.into(), show_hidden };
        self.backend.send(Command::Search(cmd));
        let (mut found, mut progress) = (Vec::new(), Vec::new());
        let end = self.wait_for("the search to end", |ev| match ev {
            ServiceEvent::Search(SearchEvent::Found { id: i, entries, folders, .. }) if *i == id => {
                found.extend(entries.iter().cloned());
                progress.push(*folders);
                None
            }
            ServiceEvent::Search(SearchEvent::Done { id: i, .. } | SearchEvent::Failed { id: i, .. }) if *i == id => {
                Some(ev.clone())
            }
            _ => None,
        });
        Searched { found, progress, end }
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

fn done(folders: u64, unreadable: u64, capped: bool, id: u64) -> ServiceEvent {
    ServiceEvent::Search(SearchEvent::Done { id: SearchId(id), host: host(), folders, unreadable, capped })
}

#[test]
fn a_search_of_the_seeded_folder_finds_names_below_it_and_notes_the_locked_folder() {
    let Some(mut h) = Harness::new("search_mixed", true) else { return };
    let mixed = h.fx.path(Role::Key, "fixture/mixed");

    // fixture/mixed (seed.sh): dir-01..dir-10 (dir-01/inside.txt), .config, locked (mode 000:
    // it can't be read), links to a folder and a file (matched, never walked into), .hidden.
    let s = h.search(1, &mixed, "INSIDE", false);
    assert_eq!(s.names(), ["dir-01/inside.txt"]);
    assert_eq!(s.end, done(11, 1, false, 1), "mixed and its ten dir-NN; .config is hidden");

    let s = h.search(2, &mixed, "link", false);
    assert_eq!(s.names(), ["./link-dir", "./link-file"]);

    // A glob matches whole names: 80 numbered files, empty, spaces, unicode, and inside.txt.
    let s = h.search(3, &mixed, "*.txt", false);
    assert_eq!(s.found.len(), 84, "{:?}", s.names());
    assert!(s.names().contains(&"./unicode-ğüş.txt".to_owned()));
    assert!(s.progress.windows(2).all(|w| w[0] <= w[1]), "{:?}", s.progress);

    // Hidden files shown: .hidden matches and .config is searched too.
    let s = h.search(4, &mixed, "hidden", true);
    assert_eq!(s.names(), ["./.hidden"]);
    assert_eq!(s.end, done(12, 1, false, 4));
}

#[test]
fn a_search_of_ten_thousand_files_finishes_and_one_past_the_cap_stops_there() {
    let Some(mut h) = Harness::new("search_many", true) else { return };
    let many = h.fx.path(Role::Key, "fixture/many");

    let started = Instant::now();
    let s = h.search(1, &many, "f10000", false);
    let took = started.elapsed();
    assert_eq!(s.names(), ["./f10000"]);
    assert_eq!(s.end, done(1, 0, false, 1));
    println!("search of 10,000 entries: {took:?}, {} batches", s.progress.len());

    let started = Instant::now();
    let s = h.search(2, &many, "f*", false);
    println!("search stopped at the cap: {:?}", started.elapsed());
    assert_eq!(s.found.len(), CAP);
    assert_eq!(s.end, done(1, 0, true, 2));
}

#[test]
fn without_a_connection_a_search_fails_at_once() {
    let Some(mut h) = Harness::new("search_offline", false) else { return };
    let path = h.fx.path(Role::Key, "fixture/mixed");
    let s = h.search(1, &path, "x", false);
    assert!(s.found.is_empty());
    let lost = SearchEvent::Failed { id: SearchId(1), host: host(), error: Error::ConnectionLost };
    assert_eq!(s.end, ServiceEvent::Search(lost));
}
