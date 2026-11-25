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
//!   connection the way a network drop does.
//! - `hostkey`: asks the unknown-host question; anything but `yes` is "Host key verification
//!   failed."
//! - `vanish`: asks for a code, then exits (once the test saw the prompt) before it's answered.
//!
//! Skipped (with a note) where `sftp-server` or `python3` is missing. The Docker-based tests
//! against a real sshd are `tests/e2e.rs`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dromon_core::{ConnStatus, HostId, HostSpec, Prompt, PromptAnswer, PromptKind, RemotePath};
use tokio::sync::mpsc;

use super::{Config, Sink, run};
use crate::state::{Command, ServiceEvent};

const SFTP_SERVERS: [&str; 2] = ["/usr/lib/openssh/sftp-server", "/usr/lib/ssh/sftp-server"];
const WAIT: Duration = Duration::from_secs(20);

const FAKE_SSH: &str = r#"#!/bin/bash
# A fake ssh for Dromon's service tests; see service/tests.rs.
dir="$(cd "$(dirname "$0")" && pwd)"
alias=""; prev=""
for a in "$@"; do [ "$prev" = "-s" ] && alias="$a"; prev="$a"; done
echo "debug1: fake ssh for $alias" >&2
case "$alias" in
  pw)
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
  sleep 0.1
done
wait "$pid"
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

#[derive(Clone)]
struct TestSink(mpsc::UnboundedSender<ServiceEvent>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send(ev);
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<ServiceEvent>,
    /// Everything seen so far, for failure messages.
    seen: Vec<ServiceEvent>,
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn host(alias: &str) -> HostId {
    HostId(alias.into())
}

impl Fixture {
    /// The service running against the fake, or `None` (skip) without its tools.
    fn start() -> Option<Fixture> {
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
            hosts: ["pw", "hostkey", "vanish"].iter().map(|a| HostSpec::from_config_alias(a)).collect(),
            without_env: Vec::new(),
        };
        let (commands, rx) = mpsc::unbounded_channel();
        let (sink, events) = mpsc::unbounded_channel();
        tokio::spawn(run(rx, TestSink(sink), cfg));
        Some(Fixture { dir, commands, events, seen: Vec::new() })
    }

    fn root(&self) -> RemotePath {
        RemotePath::new(&self.dir.path().join("root").canonicalize().unwrap().to_string_lossy())
    }

    fn send(&self, cmd: Command) {
        self.commands.send(cmd).unwrap();
    }

    /// Waits for the first event `pick` takes, skipping others.
    async fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&ServiceEvent) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(ev)) => {
                    let found = pick(&ev);
                    self.seen.push(ev);
                    if let Some(found) = found {
                        return found;
                    }
                }
                _ => panic!("timed out waiting for {what}; saw:\n{:#?}", self.seen),
            }
        }
    }

    async fn status(&mut self, h: &str, want: ConnStatus) {
        let h = host(h);
        self.until(&format!("{want:?}"), |ev| match ev {
            ServiceEvent::Status { host, status } if *host == h && *status == want => Some(()),
            _ => None,
        })
        .await
    }

    async fn prompt(&mut self) -> Prompt {
        self.until("a prompt", |ev| match ev {
            ServiceEvent::Prompt(p) => Some(p.clone()),
            _ => None,
        })
        .await
    }

    /// The names of a whole listing of `path` (pages until `done`).
    async fn listing(&mut self, path: &RemotePath) -> Vec<String> {
        let mut names = Vec::new();
        loop {
            let (entries, done) = self
                .until("a listing page", |ev| match ev {
                    ServiceEvent::ListingPage { path: p, entries, done, .. } if p == path => Some((entries.clone(), *done)),
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

    fn answer(&self, p: &Prompt, text: &str) {
        self.send(Command::AnswerPrompt { id: p.id, answer: PromptAnswer::Text(text.into()) });
    }

    /// Since `from`, whether any event matched.
    fn saw_since(&self, from: usize, f: impl Fn(&ServiceEvent) -> bool) -> bool {
        self.seen[from..].iter().any(f)
    }
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
    fx.until("ListFailed", |ev| matches!(ev, ServiceEvent::ListFailed { path, .. } if *path == missing).then_some(())).await;
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
            ServiceEvent::ListingPage { path, first: true, .. } if *path == missing || *path == sub || *path == start => Some(path.clone()),
            _ => None,
        })
        .await;
    assert_eq!(relisted, sub, "the last folder asked for is listed again");
    assert!(!fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Prompt(_))), "a reconnect doesn't ask again");
    assert!(fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Log { line, .. } if line.contains("answered from memory"))));

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
    fx.until("PromptClosed", |ev| matches!(ev, ServiceEvent::PromptClosed(closed) if *closed == id).then_some(())).await;
    fx.status("vanish", ConnStatus::Failed("Connection closed by 127.0.0.1 port 22".into())).await;
    // A late answer to it goes nowhere.
    fx.answer(&p, "123456");
    let log = fx.seen.iter().filter(|ev| matches!(ev, ServiceEvent::Log { .. })).count();
    assert!(log > 0);
}
