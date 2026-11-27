//! Live updates (§11 "Live mode") end to end against the sshd fixture (`dromon-testkit`): the
//! real service thread with its second ssh session per host, the system `ssh`, the `dromon`
//! binary as askpass helper, `inotifywait` on the server (the trixie image has it), and the app's
//! own state on top, applying every event as the window would. Changes are made on the server
//! with `fx.exec`; the tests send no command after them.
//!
//! - A change in the folder on screen (made, renamed, deleted) shows within a second, far below
//!   the poll interval (2 s focused, 8 s while live); navigation moves the watch and hiding the
//!   window stops it.
//! - A password host's watch signs in from memory without a second prompt; the TOTP (`kbd`)
//!   host's needs a code, so it gives up silently and the host is polled.
//! - A killed watch falls back to polling.
//! - The matrix: every server either goes live or falls back to polls cleanly (bullseye and
//!   alpine have no `inotifywait`).
//!
//! Servers without `inotifywait` (the native backend) skip the live checks with a line saying
//! so. Each test has its own ssh config and `known_hosts`, and its own scratch folder.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dromon_app::service::{self, ChannelBackend, Config, Sink};
use dromon_app::state::{Action, App, Backend, Command, HostDirectory, Listing, Live, LiveUnavailable, ServiceEvent};
use dromon_core::{ConnStatus, HostId, HostSpec, PromptAnswer, PromptKind, RemotePath};
use dromon_testkit::matrix::{self, Outcome};
use dromon_testkit::{Feature, Fixture, NULL_DEVICE, PASSWORD, Role, copy_private_key, totp_now};

const HELPER: &str = env!("CARGO_BIN_EXE_dromon");
const WAIT: Duration = Duration::from_secs(60);
/// What "at once" means here: well under the shortest poll interval.
const AT_ONCE: Duration = Duration::from_secs(1);
/// The focused interval for a small folder (§4.5), and what a loaded runner may add to it.
const INTERVAL: Duration = Duration::from_secs(2);
const SLACK: Duration = Duration::from_secs(3);
const ALIAS: &str = "fx";

fn host() -> HostId {
    HostId(ALIAS.into())
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
    role: Role,
    _dir: tempfile::TempDir,
    app: App,
    /// Another end of the app's command channel, for answering prompts as a sheet would.
    answers: ChannelBackend,
    events: mpsc::Receiver<ServiceEvent>,
    scratch: RemotePath,
    /// Prompts seen so far.
    prompts: Vec<PromptKind>,
    log: Vec<String>,
    /// What was typed, checked against every event.
    secrets: Vec<String>,
}

impl Harness {
    /// Signs in as `role` (answering its password and code), with a fresh scratch folder on
    /// screen in a focused window.
    fn new(fx: &'static Fixture, role: Role, name: &str) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let config = write_ssh_config(fx, role, dir.path());
        let (tx, events) = mpsc::channel();
        let (backend, commands) = service::channel();
        let answers = backend.for_window(dromon_app::state::WindowId::FIRST);
        let spec = HostSpec::from_config_alias(ALIAS);
        let cfg = Config {
            ssh: dromon_ssh::ssh_program(),
            helper_exe: PathBuf::from(HELPER),
            verbose: false,
            config_file: Some(config),
            discover: None,
            hosts_file: None,
            hosts: vec![spec.clone()],
            without_env: vec!["SSH_AUTH_SOCK".into()],
            editing_root: None,
        };
        service::start(commands, TestSink(tx), cfg).expect("start the service thread");
        let scratch = fx.path(role, &format!("scratch/live-{name}-{}", std::process::id()));
        let make = format!("rm -rf '{0}' && mkdir -p '{0}' && chown {1} '{0}'", scratch.as_str(), fx.owner(role));
        fx.exec(&["sh", "-c", &make]).unwrap();
        let app = App::new(HostDirectory { hosts: vec![spec] }, 0, Box::new(backend));
        let mut h = Harness {
            fx,
            role,
            _dir: dir,
            app,
            answers,
            events,
            scratch,
            prompts: Vec::new(),
            log: Vec::new(),
            secrets: Vec::new(),
        };
        h.app.act(Action::SelectHost(host()));
        h.pump("the connection to open", |app, _| {
            app.conn().is_some_and(|c| c.opened && matches!(c.listing, Listing::Ready(_)))
        });
        h.app.window_focus(true);
        let scratch = h.scratch.clone();
        h.go(&scratch);
        h
    }

    fn pump(&mut self, what: &str, mut done: impl FnMut(&App, &ServiceEvent) -> bool) -> ServiceEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = self
                .next(left)
                .unwrap_or_else(|| panic!("timed out waiting for {what}\nconnection log:\n{}", self.log.join("\n")));
            if done(&self.app, &ev) {
                return ev;
            }
        }
    }

    /// The next event within `wait`, applied to the app; a prompt is answered as the user would.
    fn next(&mut self, wait: Duration) -> Option<ServiceEvent> {
        let ev = self.events.recv_timeout(wait).ok()?;
        for secret in &self.secrets {
            assert!(!format!("{ev:?}").contains(secret.as_str()), "a secret reached an event: {ev:?}");
        }
        match &ev {
            ServiceEvent::Log { host, line } => self.log.push(format!("[{}] {line}", host.0)),
            ServiceEvent::Status { status: ConnStatus::Failed(why), .. } => {
                panic!("the connection failed: {why}\nconnection log:\n{}", self.log.join("\n"))
            }
            _ => {}
        }
        self.app.apply(ev.clone());
        if let ServiceEvent::Prompt(p) = &ev {
            self.prompts.push(p.kind.clone());
            let answer = match p.kind {
                PromptKind::Password => PASSWORD.to_owned(),
                PromptKind::Other => totp_now(),
                ref kind => panic!("an unexpected prompt: {kind:?}"),
            };
            self.secrets.push(answer.clone());
            self.answers.send(Command::AnswerPrompt { id: p.id, answer: PromptAnswer::Text(answer) });
        }
        Some(ev)
    }

    fn go(&mut self, path: &RemotePath) {
        self.app.act(Action::GoTo(path.clone()));
        let p = path.clone();
        self.pump("the folder to list", |app, ev| {
            matches!(ev, ServiceEvent::ListingPage { path, done: true, .. } if *path == p)
                && app.conn().is_some_and(|c| c.cwd == p)
        });
    }

    fn live(&mut self, want: Live) {
        self.pump(&format!("{want:?}"), |app, _| app.conn().is_some_and(|c| c.live == want));
    }

    /// A refresh of the folder on screen where `seen` holds; how long it took since `since`.
    fn refresh_where(&mut self, since: Instant, what: &str, seen: impl Fn(&[String]) -> bool) -> Duration {
        let path = self.scratch_or_cwd();
        self.pump(what, |_, ev| match ev {
            ServiceEvent::Refreshed { path: p, entries, .. } if *p == path => {
                seen(&entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>())
            }
            _ => false,
        });
        since.elapsed()
    }

    fn scratch_or_cwd(&self) -> RemotePath {
        self.app.conn().map_or_else(|| self.scratch.clone(), |c| c.cwd.clone())
    }

    /// Runs `cmd` on the server (as root) in the scratch folder, then hands it to the user.
    fn remote(&self, cmd: &str) {
        let s = self.scratch.as_str();
        let script = format!("cd '{s}' && {cmd} && chown -R {} '{s}'", self.fx.owner(self.role));
        self.fx.exec(&["sh", "-c", &script]).unwrap();
    }

    /// How many `inotifywait` run on the server in this test's folder (other tests share users).
    fn watches(&self) -> usize {
        let out = self.fx.exec(&[
            "sh",
            "-c",
            "pgrep -u \"$1\" -f \"$2\"; exit 0",
            "sh",
            self.fx.user(self.role),
            &self.watch_pattern(),
        ]);
        out.unwrap().lines().filter(|l| !l.trim().is_empty()).count()
    }

    /// Ends this test's `inotifywait` on the server, as an admin or a crash would.
    fn kill_watch(&self) {
        let script = "pkill -u \"$1\" -f \"$2\"; exit 0";
        self.fx.exec(&["sh", "-c", script, "sh", self.fx.user(self.role), &self.watch_pattern()]).unwrap();
    }

    /// `inotifywait`'s command line in this test's folder (not the shell that started it).
    fn watch_pattern(&self) -> String {
        format!("^inotifywait .*{}", self.scratch.as_str())
    }

    /// Whether anything matching `f` arrives within `wait`.
    fn saw_within(&mut self, wait: Duration, f: impl Fn(&ServiceEvent) -> bool) -> bool {
        let deadline = Instant::now() + wait;
        while let Some(ev) = self.next(deadline.saturating_duration_since(Instant::now())) {
            if f(&ev) {
                return true;
            }
        }
        false
    }
}

/// Whether the server has `inotifywait`; prints a skip line for `test` when it hasn't.
fn has_inotifywait(fx: &Fixture, test: &str) -> bool {
    let has = server_has_inotifywait(fx);
    if !has {
        println!("skipping {test}: the {} server has no inotifywait", fx.variant().name());
    }
    has
}

fn server_has_inotifywait(fx: &Fixture) -> bool {
    let found = fx.exec(&["sh", "-c", "command -v inotifywait; exit 0"]).unwrap_or_default();
    !found.trim().is_empty()
}

fn write_ssh_config(fx: &Fixture, role: Role, dir: &Path) -> PathBuf {
    let mut config = String::new();
    writeln!(config, "Host {ALIAS}\n    HostName {}\n    Port {}\n    User {}", fx.host(), fx.port(), fx.user(role))
        .unwrap();
    match role {
        Role::Key => {
            let key = dir.join("id_ed25519");
            copy_private_key(&fx.key(), &key).unwrap();
            writeln!(config, "    IdentityFile {}\n    PreferredAuthentications publickey", key.display()).unwrap();
        }
        Role::Password => {
            writeln!(config, "    PubkeyAuthentication no\n    PreferredAuthentications password").unwrap()
        }
        Role::Kbd => {
            writeln!(config, "    PubkeyAuthentication no\n    PreferredAuthentications keyboard-interactive").unwrap()
        }
        other => panic!("{other:?} isn't used here"),
    }
    writeln!(config, "    IdentitiesOnly yes\n    NumberOfPasswordPrompts 1").unwrap();
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

fn fixture(test: &str) -> Option<&'static Fixture> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(dromon_testkit::fixture(test))
}

fn has(names: &[String], name: &str) -> bool {
    names.iter().any(|n| n == name)
}

#[test]
fn changes_on_the_server_show_within_a_second() {
    let test = "changes_on_the_server_show_within_a_second";
    let Some(fx) = fixture(test) else { return };
    if !has_inotifywait(fx, test) {
        return;
    }
    let mut h = Harness::new(fx, Role::Key, "changes");
    let scratch = h.scratch.clone();
    h.live(Live::On(scratch.clone()));
    assert!(h.app.conn().unwrap().is_live(), "the status bar's mark");
    // The gap between the listing and the watch is polled once; let it go by.
    let _ = h.saw_within(Duration::from_millis(500), |_| false);

    let t = Instant::now();
    h.remote("printf hello > made.txt");
    let took = h.refresh_where(t, "made.txt", |n| has(n, "made.txt"));
    println!("made: {took:?}");
    assert!(took < AT_ONCE, "made.txt took {took:?}");

    let t = Instant::now();
    h.remote("mv made.txt renamed.txt");
    let took = h.refresh_where(t, "the rename", |n| has(n, "renamed.txt") && !has(n, "made.txt"));
    println!("renamed: {took:?}");
    assert!(took < AT_ONCE, "the rename took {took:?}");

    let t = Instant::now();
    h.remote("rm renamed.txt");
    let took = h.refresh_where(t, "the delete", |n| !has(n, "renamed.txt"));
    println!("deleted: {took:?}");
    assert!(took < AT_ONCE, "the delete took {took:?}");
    let c = h.app.conn().unwrap();
    assert_eq!(c.cwd, scratch, "refreshes, never a navigation");
    assert_eq!(h.prompts, [], "a key host is asked nothing");

    // Navigation moves the watch.
    h.remote("mkdir sub");
    let sub = scratch.join("sub");
    h.go(&sub);
    h.live(Live::On(sub.clone()));
    let t = Instant::now();
    h.remote("touch sub/inner");
    let took = h.refresh_where(t, "sub/inner", |n| has(n, "inner"));
    assert!(took < AT_ONCE, "sub/inner took {took:?}");
    assert_eq!(h.watches(), 1, "one watch, on the folder on screen");

    // Hiding the window stops the watch on the server; showing it starts one again.
    h.app.window_visibility(false);
    h.live(Live::Off);
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.watches() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(h.watches(), 0, "the watch ended on the server");
    h.app.window_visibility(true);
    h.live(Live::On(sub));
}

#[test]
fn a_password_host_goes_live_without_a_second_prompt() {
    let test = "a_password_host_goes_live_without_a_second_prompt";
    let Some(fx) = fixture(test) else { return };
    if !has_inotifywait(fx, test) {
        return;
    }
    let mut h = Harness::new(fx, Role::Password, "password");
    let scratch = h.scratch.clone();
    h.live(Live::On(scratch));
    assert_eq!(h.prompts, [PromptKind::Password], "the session's password only");
    let t = Instant::now();
    h.remote("touch pw.txt");
    let took = h.refresh_where(t, "pw.txt", |n| has(n, "pw.txt"));
    assert!(took < AT_ONCE, "took {took:?}");
    assert!(!h.saw_within(Duration::from_secs(1), |ev| matches!(ev, ServiceEvent::Prompt(_))));
    assert_eq!(h.prompts.len(), 1);
}

#[test]
fn a_totp_host_stays_on_polling_without_a_prompt() {
    let test = "a_totp_host_stays_on_polling_without_a_prompt";
    let Some(fx) = fixture(test) else { return };
    if !fx.supports(Feature::Totp, test) || !has_inotifywait(fx, test) {
        return;
    }
    let mut h = Harness::new(fx, Role::Kbd, "totp");
    h.live(Live::Unavailable(LiveUnavailable::SignIn));
    assert_eq!(h.prompts, [PromptKind::Password, PromptKind::Other], "the session's password and code only");
    assert!(
        !h.saw_within(Duration::from_secs(2), |ev| matches!(ev, ServiceEvent::Prompt(_) | ServiceEvent::Live { .. }))
    );
    // Polled as before.
    let t = Instant::now();
    h.remote("touch polled.txt");
    let took = h.refresh_where(t, "polled.txt", |n| has(n, "polled.txt"));
    assert!(took <= INTERVAL + SLACK, "took {took:?}");
    assert_eq!(h.watches(), 0);
    assert_eq!(h.prompts.len(), 2);
}

#[test]
fn a_killed_watch_falls_back_to_polling() {
    let test = "a_killed_watch_falls_back_to_polling";
    let Some(fx) = fixture(test) else { return };
    if !has_inotifywait(fx, test) {
        return;
    }
    let mut h = Harness::new(fx, Role::Key, "killed");
    let scratch = h.scratch.clone();
    h.live(Live::On(scratch.clone()));
    assert_eq!(h.watches(), 1);
    h.kill_watch();
    h.live(Live::Off);
    assert!(h.log.iter().any(|l| l.contains("Live updates stopped")), "{:#?}", h.log);
    assert_eq!(h.app.conn().unwrap().status, ConnStatus::Ready, "the session is untouched");
    let t = Instant::now();
    h.remote("touch after-kill.txt");
    let took = h.refresh_where(t, "after-kill.txt", |n| has(n, "after-kill.txt"));
    println!("polled after {took:?}");
    assert!(took <= INTERVAL + SLACK, "took {took:?}");
    assert!(!h.app.conn().unwrap().is_live(), "not watched again on the same folder");
}

/// One server: live with a change at once, or polls with the reason said.
fn live_or_fallback(fx: &'static Fixture) -> Outcome {
    let name = format!("matrix-{}", fx.variant().name());
    let live = server_has_inotifywait(fx);
    let mut h = Harness::new(fx, Role::Key, &name);
    let scratch = h.scratch.clone();
    if live {
        h.live(Live::On(scratch));
        let _ = h.saw_within(Duration::from_millis(500), |_| false);
        let t = Instant::now();
        h.remote("touch live.txt");
        // The toxic link adds its latency to the watch's line and the poll.
        let took = h.refresh_where(t, "live.txt", |n| has(n, "live.txt"));
        assert!(took < INTERVAL, "took {took:?}");
        Outcome::Ok(format!("live, a change in {:.2} s", took.as_secs_f64()))
    } else {
        h.live(Live::Unavailable(LiveUnavailable::NoInotify));
        let t = Instant::now();
        h.remote("touch polled.txt");
        let took = h.refresh_where(t, "polled.txt", |n| has(n, "polled.txt"));
        assert!(took <= INTERVAL + SLACK, "took {took:?}");
        assert_eq!(h.prompts, []);
        Outcome::Ok(format!("no inotifywait: polled, a change in {:.2} s", took.as_secs_f64()))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_server_goes_live_or_falls_back_to_polls() {
    matrix::run("live updates", |fx| async move {
        tokio::task::spawn_blocking(move || live_or_fallback(fx))
            .await
            .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
    })
    .await;
}
