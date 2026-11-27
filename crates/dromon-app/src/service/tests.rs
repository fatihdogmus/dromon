//! The real service (manager, connection tasks, askpass server, SFTP) against a fake ssh: a
//! shell script that asks its questions through `$SSH_ASKPASS` exactly as ssh does, then
//! serves SFTP with OpenSSH's own `sftp-server` on its stdio. The askpass helper is a small
//! Python script speaking the helper protocol (a unit-test binary can't be ssh's helper; the
//! real system ssh, sshd and `dromon` binary as helper run in `tests/e2e.rs`).
//!
//! The fake picks its behaviour from the host alias (`-s <alias> sftp`):
//!
//! - `pw`: asks `x@y's password: ` until it hears `secret` (three tries); a cancel is
//!   "Permission denied". While serving, creating the file `drop` next to it ends the
//!   connection the way a network drop does, and `freeze` stops `sftp-server` (a network gone
//!   silent: ssh still runs, nothing answers). While the file `down` exists it refuses to
//!   connect.
//! - With `-v` it says so on stderr (`debug1: verbose …`).
//! - `hostkey`: asks the unknown-host question; anything but `yes` is "Host key verification
//!   failed."
//! - `vanish`: asks for a code, then exits (once the test saw the prompt) before it's answered.
//! - Without `-s` (live updates' watch, `<alias> -- <command>`): `pw` asks its password again
//!   and `hostkey` its question, then the command runs in a local shell, which has a fake
//!   `inotifywait` (a line per change of the folder's listing, ending when `live-stop` appears)
//!   while the file `inotify` exists, and none otherwise.
//!
//! Skipped (with a note) where `sftp-server` or `python3` is missing. The Docker-based tests
//! against a real sshd are `tests/e2e.rs`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dromon_core::{ConnStatus, Error, HostId, HostSpec, Prompt, PromptAnswer, PromptKind, RemotePath};
use tokio::sync::mpsc;

use super::{Config, Sink, serve};
use crate::state::{Command, FileOp, FileOpEvent, OpId, ServiceEvent, WindowId};

const SFTP_SERVERS: [&str; 2] = ["/usr/lib/openssh/sftp-server", "/usr/lib/ssh/sftp-server"];
const WAIT: Duration = Duration::from_secs(20);

const FAKE_SSH: &str = r#"#!/bin/bash
# A fake ssh for Dromon's service tests; see service/tests.rs.
dir="$(cd "$(dirname "$0")" && pwd)"
alias=""; prev=""
for a in "$@"; do [ "$prev" = "-s" ] && alias="$a"; prev="$a"; done
if [ -z "$alias" ]; then
  # A remote command (live updates' watch): `<options> <alias> -- <command>`.
  for a in "$@"; do [ "$a" = "--" ] && host="$last"; last="$a"; done
  if [ "$host" = "pw" ]; then
    answer="$("$SSH_ASKPASS" "x@y's password: ")" || exit 255
    [ "$answer" = "secret" ] || { echo "x@y: Permission denied (password)." >&2; exit 255; }
  fi
  if [ "$host" = "hostkey" ]; then
    "$SSH_ASKPASS" "The authenticity of host 'hostkey (127.0.0.1)' can't be established.
ED25519 key fingerprint is SHA256:fakefingerprint.
Are you sure you want to continue connecting (yes/no/[fingerprint])? " >/dev/null || exit 255
  fi
  # The server has inotifywait (the fake one in bin/) while the file `inotify` exists.
  [ -e "$dir/inotify" ] && exec env PATH="$dir/bin" /bin/sh -c "$last"
  exec env PATH=/nonexistent /bin/sh -c "$last"
fi
echo "debug1: fake ssh for $alias" >&2
for a in "$@"; do [ "$a" = "-v" ] && echo "debug1: verbose output from the fake ssh" >&2; done
case "$alias" in
  pw)
    [ -e "$dir/down" ] && { echo "ssh: connect to host y port 22: Connection refused" >&2; exit 255; }
    for try in 1 2 3; do
      answer="$("$SSH_ASKPASS" "x@y's password: ")" || { echo "x@y: Permission denied (password)." >&2; exit 255; }
      [ "$answer" = "secret" ] && break
      [ "$try" = 3 ] && { echo "x@y: Permission denied (password)." >&2; exit 255; }
    done ;;
  hostkey)
    answer="$("$SSH_ASKPASS" "The authenticity of host 'hostkey (127.0.0.1)' can't be established.
ED25519 key fingerprint is SHA256:fakefingerprint.
Are you sure you want to continue connecting (yes/no/[fingerprint])? ")"
    [ "$answer" = "yes" ] || { echo "Host key verification failed." >&2; exit 255; } ;;
  vanish)
    "$SSH_ASKPASS" "Verification code: " >/dev/null &
    # Exit once the test has seen the prompt (it creates `go`), or after 15 s.
    for _ in $(seq 150); do [ -e "$dir/go" ] && break; sleep 0.1; done
    echo "Connection closed by 127.0.0.1 port 22" >&2
    exit 255 ;;
esac
cd "$dir/root" || exit 1
exec 3<&0
"$SFTP_SERVER" <&3 &
pid=$!
while kill -0 "$pid" 2>/dev/null; do
  if [ -e "$dir/drop" ]; then
    rm -f "$dir/drop"; kill "$pid"
    echo "Connection to y closed by remote host." >&2
    exit 255
  fi
  if [ -e "$dir/freeze" ]; then
    rm -f "$dir/freeze"; kill -STOP "$pid"
    # Once nobody waits for it any more.
    ( sleep 30; kill -KILL "$pid" ) >/dev/null 2>&1 &
  fi
  sleep 0.1
done
wait "$pid"
"#;

/// A fake `inotifywait -m … <folder>`: a line whenever the folder's listing changes; it ends when
/// the file `live-stop` appears (a killed watch).
const FAKE_INOTIFYWAIT: &str = r#"#!/bin/bash
export PATH=/usr/bin:/bin
dir="$(cd "$(dirname "$0")/.." && pwd)"
for a in "$@"; do folder="$a"; done
seen="$(ls -la "$folder" | cksum)"
while :; do
  [ -e "$dir/live-stop" ] && { rm -f "$dir/live-stop"; exit 1; }
  now="$(ls -la "$folder" | cksum)"
  [ "$now" != "$seen" ] && { seen="$now"; echo CREATE; }
  sleep 0.05
done
"#;

const FAKE_ASKPASS: &str = r#"#!/usr/bin/env python3
# The askpass helper protocol: one JSON line each way over the endpoint socket.
import json, os, socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(os.environ["DROMON_ASKPASS_ENDPOINT"])
req = {"token": os.environ["DROMON_ASKPASS_TOKEN"], "prompt": sys.argv[1] if len(sys.argv) > 1 else "",
       "hint": os.environ.get("SSH_ASKPASS_PROMPT")}
s.sendall((json.dumps(req) + "\n").encode())
reply = json.loads(s.makefile("rb").readline() or b"{}")
if "answer" in reply:
    sys.stdout.write(reply["answer"] + "\n")
    sys.exit(0)
sys.exit(1)
"#;

/// Each event with the window it's for (`None`: every window).
#[derive(Clone)]
struct TestSink(mpsc::UnboundedSender<(Option<WindowId>, ServiceEvent)>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send((None, ev));
    }
    fn emit_to(&self, window: WindowId, ev: ServiceEvent) {
        let _ = self.0.send((Some(window), ev));
    }
}

pub(super) struct Fixture {
    pub(super) dir: tempfile::TempDir,
    commands: mpsc::UnboundedSender<(WindowId, Command)>,
    /// Tells the service the computer slept this long (instead of the clocks' watcher).
    pub(super) wake: mpsc::UnboundedSender<Duration>,
    events: mpsc::UnboundedReceiver<(Option<WindowId>, ServiceEvent)>,
    /// Everything seen so far, for failure messages.
    pub(super) seen: Vec<ServiceEvent>,
    /// The window each event in `seen` was for (`None`: every window).
    pub(super) to: Vec<Option<WindowId>>,
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

pub(super) fn host(alias: &str) -> HostId {
    HostId(alias.into())
}

impl Fixture {
    /// The service running against the fake, or `None` (skip) without its tools.
    pub(super) fn start() -> Option<Fixture> {
        let Some(sftp_server) = SFTP_SERVERS.iter().map(PathBuf::from).find(|p| p.is_file()) else {
            eprintln!("skipped: no sftp-server at {SFTP_SERVERS:?}");
            return None;
        };
        if std::process::Command::new("python3").arg("-c").arg("pass").status().map_or(true, |s| !s.success()) {
            eprintln!("skipped: no python3 for the fake askpass helper");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join("ssh");
        write_exe(&ssh, &FAKE_SSH.replace("\"$SFTP_SERVER\"", &format!("\"{}\"", sftp_server.display())));
        let helper = dir.path().join("askpass.py");
        write_exe(&helper, FAKE_ASKPASS);
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        write_exe(&dir.path().join("bin/inotifywait"), FAKE_INOTIFYWAIT);
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), "hello").unwrap();
        std::fs::write(root.join(".hidden"), "").unwrap();

        let cfg = Config {
            ssh,
            helper_exe: helper,
            verbose: false,
            config_file: None,
            discover: None,
            hosts_file: None,
            hosts: ["pw", "hostkey", "vanish"].iter().map(|a| HostSpec::from_config_alias(a)).collect(),
            without_env: Vec::new(),
        };
        let (commands, rx) = mpsc::unbounded_channel();
        let (sink, events) = mpsc::unbounded_channel();
        let (wake, wakes) = mpsc::unbounded_channel();
        tokio::spawn(serve(rx, TestSink(sink), cfg, wakes));
        Some(Fixture { dir, commands, wake, events, seen: Vec::new(), to: Vec::new() })
    }

    pub(super) fn root(&self) -> RemotePath {
        RemotePath::new(&self.dir.path().join("root").canonicalize().unwrap().to_string_lossy())
    }

    /// `cmd` from the first window.
    pub(super) fn send(&self, cmd: Command) {
        self.send_from(WindowId::FIRST, cmd);
    }

    pub(super) fn send_from(&self, window: WindowId, cmd: Command) {
        self.commands.send((window, cmd)).unwrap();
    }

    /// Waits for the first event `pick` takes, skipping others.
    pub(super) async fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&ServiceEvent) -> Option<T>) -> T {
        self.until_for(what, |_, ev| pick(ev)).await
    }

    /// [`Fixture::until`], with the window each event is for (`None`: every window).
    pub(super) async fn until_for<T>(
        &mut self,
        what: &str,
        mut pick: impl FnMut(Option<WindowId>, &ServiceEvent) -> Option<T>,
    ) -> T {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some((to, ev))) => {
                    let found = pick(to, &ev);
                    self.seen.push(ev);
                    self.to.push(to);
                    if let Some(found) = found {
                        return found;
                    }
                }
                _ => panic!("timed out waiting for {what}; saw:\n{:#?}", self.seen),
            }
        }
    }

    pub(super) async fn status(&mut self, h: &str, want: ConnStatus) {
        let h = host(h);
        self.until(&format!("{want:?}"), |ev| match ev {
            ServiceEvent::Status { host, status } if *host == h && *status == want => Some(()),
            _ => None,
        })
        .await
    }

    pub(super) async fn prompt(&mut self) -> Prompt {
        self.until("a prompt", |ev| match ev {
            ServiceEvent::Prompt(p) => Some(p.clone()),
            _ => None,
        })
        .await
    }

    /// The names of a whole listing of `path` (pages until `done`).
    pub(super) async fn listing(&mut self, path: &RemotePath) -> Vec<String> {
        let mut names = Vec::new();
        loop {
            let (entries, done) = self
                .until("a listing page", |ev| match ev {
                    ServiceEvent::ListingPage { path: p, entries, done, .. } if p == path => {
                        Some((entries.clone(), *done))
                    }
                    _ => None,
                })
                .await;
            names.extend(entries.into_iter().map(|e| e.name));
            if done {
                names.sort();
                return names;
            }
        }
    }

    pub(super) fn answer(&self, p: &Prompt, text: &str) {
        self.send(Command::AnswerPrompt { id: p.id, answer: PromptAnswer::Text(text.into()) });
    }

    /// Since `from`, whether any event matched.
    pub(super) fn saw_since(&self, from: usize, f: impl Fn(&ServiceEvent) -> bool) -> bool {
        self.seen[from..].iter().any(f)
    }

    /// Connects `pw` (answering its password) and returns the start folder.
    pub(super) async fn sign_in(&mut self) -> RemotePath {
        self.send(Command::Connect { host: host("pw") });
        let p = self.prompt().await;
        self.answer(&p, "secret");
        self.until("Opened", |ev| match ev {
            ServiceEvent::Opened { start, .. } => Some(start.clone()),
            _ => None,
        })
        .await
    }

    pub(super) fn op(&self, id: u64, op: FileOp) {
        self.send(Command::FileOp { id: OpId(id), host: host("pw"), op });
    }

    /// How operation `id` ended.
    pub(super) async fn op_end(&mut self, id: u64) -> Result<(), Error> {
        let id = OpId(id);
        self.until("the operation to end", |ev| match ev {
            ServiceEvent::FileOp(FileOpEvent::Done { id: i, .. }) if *i == id => Some(Ok(())),
            ServiceEvent::FileOp(FileOpEvent::Failed { id: i, error, .. }) if *i == id => Some(Err(error.clone())),
            _ => None,
        })
        .await
    }

    /// True when nothing matching `f` arrives for `quiet`.
    pub(super) async fn none_for(&mut self, quiet: Duration, f: impl Fn(&ServiceEvent) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + quiet;
        while let Ok(Some((to, ev))) = tokio::time::timeout_at(deadline, self.events.recv()).await {
            let hit = f(&ev);
            self.seen.push(ev);
            self.to.push(to);
            if hit {
                return false;
            }
        }
        true
    }

    /// `rel` under the served root.
    pub(super) fn local(&self, rel: &str) -> PathBuf {
        self.dir.path().join("root").join(rel)
    }
}

pub(super) fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_ops_run_on_the_session_and_list_the_folder_on_screen_again() {
    let Some(mut fx) = Fixture::start() else { return };
    // Not connected (no connection at all): fails at once.
    fx.op(1, FileOp::Mkdir { path: p("/nope") });
    assert_eq!(fx.op_end(1).await, Err(Error::ConnectionLost));

    let start = fx.sign_in().await;
    fx.send(Command::List { host: host("pw"), path: start.clone() });
    assert_eq!(fx.listing(&start).await, [".hidden", "a.txt", "sub"]);

    // A new folder, then the folder on screen again with it; the same name again is taken.
    fx.op(2, FileOp::Mkdir { path: start.join("new") });
    assert_eq!(fx.op_end(2).await, Ok(()));
    assert_eq!(fx.listing(&start).await, [".hidden", "a.txt", "new", "sub"]);
    assert!(fx.local("new").is_dir());
    fx.op(3, FileOp::Mkdir { path: start.join("new") });
    assert_eq!(fx.op_end(3).await, Err(Error::AlreadyExists(start.join("new"))));
    fx.op(4, FileOp::Mkdir { path: start.join("missing/new") });
    assert_eq!(fx.op_end(4).await, Err(Error::NotFound(start.join("missing/new"))));

    // Rename; onto a name that's taken it refuses and nothing changes.
    fx.op(5, FileOp::Rename { from: start.join("a.txt"), to: start.join("b.txt") });
    assert_eq!(fx.op_end(5).await, Ok(()));
    assert_eq!(fx.listing(&start).await, [".hidden", "b.txt", "new", "sub"]);
    fx.op(6, FileOp::Rename { from: start.join("b.txt"), to: start.join("sub") });
    assert_eq!(fx.op_end(6).await, Err(Error::AlreadyExists(start.join("sub"))));
    assert_eq!(std::fs::read_to_string(fx.local("b.txt")).unwrap(), "hello");

    // Delete a tree (a link in it to a folder outside stays a link: its target is kept) and a
    // folder, with progress; then the folder on screen without them.
    std::fs::create_dir_all(fx.local("tree/x/y")).unwrap();
    std::fs::write(fx.local("tree/x/y/z.txt"), "z").unwrap();
    std::fs::write(fx.local("tree/f.txt"), "f").unwrap();
    std::os::unix::fs::symlink("../sub", fx.local("tree/link")).unwrap();
    std::fs::write(fx.local("sub/keep.txt"), "keep").unwrap();
    let mark = fx.seen.len();
    fx.op(7, FileOp::Delete { paths: vec![start.join("tree"), start.join("new")] });
    assert_eq!(fx.op_end(7).await, Ok(()));
    assert!(fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::FileOp(FileOpEvent::Progress { id: OpId(7), .. }))));
    assert_eq!(fx.listing(&start).await, [".hidden", "b.txt", "sub"]);
    assert!(fx.local("sub/keep.txt").is_file());

    // Deleting the folder on screen: it's gone, so it isn't listed again.
    std::fs::create_dir_all(fx.local("gone/inner")).unwrap();
    let gone = start.join("gone");
    fx.send(Command::List { host: host("pw"), path: gone.clone() });
    assert_eq!(fx.listing(&gone).await, ["inner"]);
    fx.op(8, FileOp::Delete { paths: vec![gone.clone()] });
    assert_eq!(fx.op_end(8).await, Ok(()));
    let listed = |ev: &ServiceEvent| matches!(ev, ServiceEvent::ListingPage { path, .. } | ServiceEvent::ListFailed { path, .. } if path.starts_with(&start));
    assert!(fx.none_for(Duration::from_millis(500), listed).await, "nothing is listed again");

    // Two deletes at once run one after the other, in order.
    for name in ["d1", "d2"] {
        std::fs::create_dir_all(fx.local(&format!("{name}/a/b"))).unwrap();
    }
    fx.send(Command::List { host: host("pw"), path: start.clone() });
    fx.listing(&start).await;
    fx.op(9, FileOp::Delete { paths: vec![start.join("d1")] });
    fx.op(10, FileOp::Delete { paths: vec![start.join("d2"), start.join("missing")] });
    let order = fx
        .until("the first delete to end", |ev| match ev {
            ServiceEvent::FileOp(FileOpEvent::Done { id, .. } | FileOpEvent::Failed { id, .. }) => Some(*id),
            _ => None,
        })
        .await;
    assert_eq!(order, OpId(9));
    assert_eq!(fx.op_end(10).await, Err(Error::NotFound(start.join("missing"))));
    assert!(fx.local("d2").is_dir(), "the count stops at the missing path before removing anything");

    // Disconnected: fails at once again.
    fx.send(Command::Disconnect { host: host("pw") });
    fx.status("pw", ConnStatus::Disconnected).await;
    fx.op(11, FileOp::Delete { paths: vec![start.join("d2")] });
    assert_eq!(fx.op_end(11).await, Err(Error::ConnectionLost));
    assert!(fx.local("d2").is_dir());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_dropped_mid_delete_fails_it_with_connection_lost() {
    let Some(mut fx) = Fixture::start() else { return };
    // Enough files that removing them takes a while.
    for d in 0..40 {
        let dir = fx.local(&format!("big/d{d:02}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..1000 {
            std::fs::write(dir.join(format!("f{f:04}")), "").unwrap();
        }
    }
    let start = fx.sign_in().await;
    fx.op(1, FileOp::Delete { paths: vec![start.join("big")] });
    // Once removing has begun, the network drops.
    fx.until("removal progress", |ev| {
        matches!(ev, ServiceEvent::FileOp(FileOpEvent::Progress { total: Some(_), .. })).then_some(())
    })
    .await;
    // (Here the whole delete takes about a second; the drop ends it within ~0.1 s.)
    std::fs::write(fx.dir.path().join("drop"), "").unwrap();
    assert_eq!(fx.op_end(1).await, Err(Error::ConnectionLost));
    assert!(fx.local("big").is_dir(), "partly deleted");
    // The connection comes back by itself; nothing resumes.
    fx.status("pw", ConnStatus::Ready).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_sign_in_listing_silent_reconnect_and_disconnect() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.send(Command::Connect { host: host("pw") });
    fx.status("pw", ConnStatus::Connecting).await;

    // Asked; a wrong answer is asked again with the error.
    let first = fx.prompt().await;
    assert_eq!((first.host.clone(), first.kind.clone(), first.error.clone()), (host("pw"), PromptKind::Password, None));
    assert_eq!(first.text, "x@y's password: ");
    fx.status("pw", ConnStatus::NeedsInput).await;
    fx.answer(&first, "wrong");
    let retry = fx.prompt().await;
    assert_ne!(retry.id, first.id);
    assert_eq!(retry.error.as_deref(), Some(dromon_ssh::WRONG_PASSWORD));
    fx.answer(&first, "stale ids are ignored");
    fx.answer(&retry, "secret");

    // Up: the start folder is the login folder, and it lists.
    let start = fx
        .until("Opened", |ev| match ev {
            ServiceEvent::Opened { host, start } if *host == self::host("pw") => Some(start.clone()),
            _ => None,
        })
        .await;
    assert_eq!(start, fx.root());
    fx.send(Command::List { host: host("pw"), path: start.clone() });
    assert_eq!(fx.listing(&start).await, [".hidden", "a.txt", "sub"]);
    let missing = start.join("nope");
    fx.send(Command::List { host: host("pw"), path: missing.clone() });
    fx.until("ListFailed", |ev| matches!(ev, ServiceEvent::ListFailed { path, .. } if *path == missing).then_some(()))
        .await;
    let sub = start.join("sub");
    fx.send(Command::List { host: host("pw"), path: sub.clone() });
    assert_eq!(fx.listing(&sub).await, Vec::<String>::new());

    // The connection drops: it reconnects on its own, answering the password from memory, and
    // lists the folder on screen again.
    let mark = fx.seen.len();
    std::fs::write(fx.dir.path().join("drop"), "").unwrap();
    fx.status("pw", ConnStatus::Reconnecting { attempt: 1 }).await;
    fx.status("pw", ConnStatus::Ready).await;
    let relisted = fx
        .until("the folder listed again", |ev| match ev {
            ServiceEvent::ListingPage { path, first: true, .. }
                if *path == missing || *path == sub || *path == start =>
            {
                Some(path.clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(relisted, sub, "the last folder asked for is listed again");
    assert!(!fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Prompt(_))), "a reconnect doesn't ask again");
    assert!(
        fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Log { line, .. } if line.contains("answered from memory")))
    );

    // Disconnect forgets the password: connecting again asks, and a cancel fails for good.
    fx.send(Command::Disconnect { host: host("pw") });
    fx.status("pw", ConnStatus::Disconnected).await;
    fx.send(Command::Connect { host: host("pw") });
    let again = fx.prompt().await;
    assert_eq!(again.error, None);
    fx.send(Command::AnswerPrompt { id: again.id, answer: PromptAnswer::Cancel });
    fx.status("pw", ConnStatus::Failed("Permission denied (password).".into())).await;

    // Nothing ever logged a secret.
    let leaked = fx.seen.iter().any(|ev| format!("{ev:?}").contains("secret") || format!("{ev:?}").contains("wrong"));
    assert!(!leaked, "an answer reached an event");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_host_key_is_trusted_with_yes_or_refused_on_cancel() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.send(Command::Connect { host: host("hostkey") });
    let p = fx.prompt().await;
    let fingerprint = Some("SHA256:fakefingerprint".to_owned());
    assert_eq!(p.kind, PromptKind::HostKey { key_type: Some("ED25519".into()), fingerprint });
    fx.answer(&p, "yes");
    fx.until("Opened", |ev| matches!(ev, ServiceEvent::Opened { .. }).then_some(())).await;

    fx.send(Command::Disconnect { host: host("hostkey") });
    fx.status("hostkey", ConnStatus::Disconnected).await;
    fx.send(Command::Connect { host: host("hostkey") });
    let p = fx.prompt().await;
    fx.send(Command::AnswerPrompt { id: p.id, answer: PromptAnswer::Cancel });
    fx.status("hostkey", ConnStatus::Failed("Host key verification failed.".into())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_closes_when_ssh_exits_without_its_answer() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.send(Command::Connect { host: host("vanish") });
    let p = fx.prompt().await;
    assert_eq!(p.kind, PromptKind::Other);
    std::fs::write(fx.dir.path().join("go"), "").unwrap();
    let id = p.id;
    fx.until("PromptClosed", |ev| matches!(ev, ServiceEvent::PromptClosed(closed) if *closed == id).then_some(()))
        .await;
    fx.status("vanish", ConnStatus::Failed("Connection closed by 127.0.0.1 port 22".into())).await;
    // A late answer to it goes nowhere.
    fx.answer(&p, "123456");
    let log = fx.seen.iter().filter(|ev| matches!(ev, ServiceEvent::Log { .. })).count();
    assert!(log > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_folder_on_screen_is_polled_only_while_shown_and_not_while_reconnecting() {
    let Some(mut fx) = Fixture::start() else { return };
    let start = fx.sign_in().await;
    fx.send(Command::List { host: host("pw"), path: start.clone() });
    fx.listing(&start).await;
    let root = fx.root();
    let about_start = |ev: &ServiceEvent| matches!(ev, ServiceEvent::ListingPage { path, .. } | ServiceEvent::Refreshed { path, .. } | ServiceEvent::ListFailed { path, .. } if path.starts_with(&root));

    // Until the window says it shows, nothing polls.
    std::fs::write(fx.local("early.txt"), "").unwrap();
    assert!(fx.none_for(Duration::from_secs(3), about_start).await, "no window state yet: no polls");

    // Shown and focused: the change arrives as a refresh, with no command.
    fx.send(Command::Window { visible: true, focused: true });
    let names = fx
        .until("a refresh", |ev| match ev {
            ServiceEvent::Refreshed { path, entries, .. } if *path == start => {
                Some(entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>())
            }
            _ => None,
        })
        .await;
    assert!(names.contains(&"early.txt".to_owned()), "{names:?}");
    // Nothing new: nothing sent.
    assert!(fx.none_for(Duration::from_millis(4500), about_start).await, "unchanged polls are quiet");

    // The connection drops: nothing polls until it's back, then the folder lists once.
    let mark = fx.seen.len();
    std::fs::write(fx.dir.path().join("drop"), "").unwrap();
    fx.status("pw", ConnStatus::Reconnecting { attempt: 1 }).await;
    std::fs::write(fx.local("while-away.txt"), "").unwrap();
    fx.status("pw", ConnStatus::Ready).await;
    let between = fx.seen[mark..].iter().filter(|ev| matches!(ev, ServiceEvent::Refreshed { .. })).count();
    assert_eq!(between, 0, "no polls while reconnecting");
    assert_eq!(
        fx.listing(&start).await.iter().filter(|n| *n == "while-away.txt").count(),
        1,
        "relisted once after the reconnect"
    );
    assert!(
        fx.none_for(Duration::from_millis(2500), about_start).await,
        "and not again: the next polls find nothing new"
    );

    // Hidden: changes wait until it shows again.
    fx.send(Command::Window { visible: false, focused: false });
    std::fs::write(fx.local("while-hidden.txt"), "").unwrap();
    assert!(fx.none_for(Duration::from_secs(5), about_start).await, "nothing while hidden");
    fx.send(Command::Window { visible: true, focused: false });
    fx.until("the refresh once shown", |ev| match ev {
        ServiceEvent::Refreshed { entries, .. } if entries.iter().any(|e| e.name == "while-hidden.txt") => Some(()),
        _ => None,
    })
    .await;
}
