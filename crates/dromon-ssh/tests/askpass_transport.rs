//! Askpass round trips through the platform's own transport, on every platform: the server from
//! `AskpassServer::bind()` (a Unix socket, or on Windows a named pipe) in a tokio runtime, the
//! helper's `ask()` on blocking threads, the helper binary as a real process, and finally the
//! system's OpenSSH (`ssh-keygen`) running that binary as its `SSH_ASKPASS`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use dromon_core::{HostId, PromptAnswer};
use dromon_ssh::{AskpassRequest, AskpassServer, ask, ssh_program};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const EXE: &str = "/opt/dromon/dromon";
/// The helper binary (`src/bin/dromon-askpass-test-helper.rs`), which cargo builds for tests.
const HELPER: &str = env!("CARGO_BIN_EXE_dromon-askpass-test-helper");

fn bind() -> (AskpassServer, mpsc::Receiver<AskpassRequest>) {
    let mut server = AskpassServer::bind().expect("bind the askpass endpoint");
    let requests = server.take_requests().unwrap();
    (server, requests)
}

/// `ask()` on a blocking thread, the answer copied out of its `Zeroizing` for comparison.
fn ask_on_thread(endpoint: &str, token: &str, prompt: &str) -> JoinHandle<io::Result<Option<String>>> {
    let (endpoint, token, prompt) = (endpoint.to_owned(), token.to_owned(), prompt.to_owned());
    tokio::task::spawn_blocking(move || ask(&endpoint, &token, &prompt, None).map(|a| a.map(|s| s.as_str().to_owned())))
}

async fn next_request(rx: &mut mpsc::Receiver<AskpassRequest>) -> AskpassRequest {
    tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("request in time").expect("server alive")
}

#[tokio::test(flavor = "multi_thread")]
async fn answer_reaches_the_helper() {
    let (server, mut requests) = bind();
    let env = server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    assert_eq!(env.endpoint, server.endpoint());
    let helper = ask_on_thread(&env.endpoint, &env.token, "deploy@prod's password: ");
    let req = next_request(&mut requests).await;
    assert_eq!(req.host, HostId("prod".into()));
    assert_eq!(req.prompt, "deploy@prod's password: ");
    assert_eq!(req.hint, None);
    req.reply.send(PromptAnswer::Text("hunter2".into())).unwrap();
    assert_eq!(helper.await.unwrap().unwrap().as_deref(), Some("hunter2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_and_a_dropped_request_give_none() {
    let (server, mut requests) = bind();
    let env = server.register(HostId("prod".into()), Path::new(EXE)).unwrap();

    let helper = ask_on_thread(&env.endpoint, &env.token, "Password: ");
    next_request(&mut requests).await.reply.send(PromptAnswer::Cancel).unwrap();
    assert_eq!(helper.await.unwrap().unwrap(), None);

    let helper = ask_on_thread(&env.endpoint, &env.token, "Password: ");
    drop(next_request(&mut requests).await);
    assert_eq!(helper.await.unwrap().unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_token_is_refused_and_nothing_is_forwarded() {
    let (server, mut requests) = bind();
    let env = server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    let mut wrong = env.token.clone();
    wrong.replace_range(0..1, if wrong.starts_with('0') { "1" } else { "0" });
    for token in [wrong, String::new(), "x".repeat(64)] {
        assert!(ask_on_thread(&env.endpoint, &token, "Password: ").await.unwrap().is_err());
    }
    server.unregister(&env.token);
    assert!(ask_on_thread(&env.endpoint, &env.token, "Password: ").await.unwrap().is_err());
    assert!(requests.try_recv().is_err(), "no request may reach the app");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_helpers_at_once_are_both_served() {
    let (server, mut requests) = bind();
    let a = server.register(HostId("a".into()), Path::new(EXE)).unwrap();
    let b = server.register(HostId("b".into()), Path::new(EXE)).unwrap();
    let ta = ask_on_thread(&a.endpoint, &a.token, "pa");
    let tb = ask_on_thread(&b.endpoint, &b.token, "pb");
    // Both wait at the same time; answer in the opposite order of arrival.
    let first = next_request(&mut requests).await;
    let second = next_request(&mut requests).await;
    let mut seen = [
        (first.host.as_str().to_owned(), first.prompt.clone()),
        (second.host.as_str().to_owned(), second.prompt.clone()),
    ];
    seen.sort();
    assert_eq!(seen, [("a".to_owned(), "pa".to_owned()), ("b".to_owned(), "pb".to_owned())]);
    for req in [second, first] {
        let answer = req.prompt.to_uppercase();
        req.reply.send(PromptAnswer::Text(answer)).unwrap();
    }
    assert_eq!(ta.await.unwrap().unwrap().as_deref(), Some("PA"));
    assert_eq!(tb.await.unwrap().unwrap().as_deref(), Some("PB"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_helpers_is_served() {
    const N: usize = 16;
    let (server, mut requests) = bind();
    let env = server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    let helpers: Vec<_> = (0..N).map(|i| ask_on_thread(&env.endpoint, &env.token, &format!("p{i}"))).collect();
    let mut pending = Vec::new();
    for _ in 0..N {
        pending.push(next_request(&mut requests).await);
    }
    for req in pending {
        let answer = format!("a-{}", req.prompt);
        req.reply.send(PromptAnswer::Text(answer)).unwrap();
    }
    for (i, helper) in helpers.into_iter().enumerate() {
        assert_eq!(helper.await.unwrap().unwrap(), Some(format!("a-p{i}")));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_bind_gets_its_own_endpoint() {
    let (a, _ra) = bind();
    let (b, _rb) = bind();
    assert_ne!(a.endpoint(), b.endpoint());
    #[cfg(windows)]
    for endpoint in [a.endpoint(), b.endpoint()] {
        let prefix = format!(r"\\.\pipe\dromon-askpass-{}-", std::process::id());
        let random = endpoint.strip_prefix(&prefix).unwrap_or_else(|| panic!("{endpoint}"));
        assert!(random.len() == 16 && random.bytes().all(|c| c.is_ascii_hexdigit()), "{endpoint}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn helper_fails_once_the_server_is_dropped() {
    let (server, mut requests) = bind();
    let env = server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    // One round trip first, so the endpoint is known to work.
    let helper = ask_on_thread(&env.endpoint, &env.token, "p");
    next_request(&mut requests).await.reply.send(PromptAnswer::Text("x".into())).unwrap();
    assert_eq!(helper.await.unwrap().unwrap().as_deref(), Some("x"));

    drop(server);
    let started = Instant::now();
    let result = ask_on_thread(&env.endpoint, &env.token, "p").await.unwrap();
    eprintln!("helper after drop: {:?} in {:?}", result.as_ref().map_err(io::Error::kind), started.elapsed());
    assert!(result.is_err());
    assert!(requests.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_or_missing_endpoint_fails() {
    let (server, _requests) = bind();
    let env = server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    assert!(ask_on_thread("", &env.token, "p").await.unwrap().is_err());
    assert!(ask_on_thread(&format!("{}-gone", env.endpoint), &env.token, "p").await.unwrap().is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn real_helper_process_answers() {
    let (server, mut requests) = bind();
    let env = server.register(HostId("prod".into()), Path::new(HELPER)).unwrap();
    let started = Instant::now();
    let child = tokio::process::Command::new(HELPER)
        .arg("Enter passphrase for key 'k': ")
        .env("DROMON_ASKPASS_ENDPOINT", &env.endpoint)
        .env("DROMON_ASKPASS_TOKEN", &env.token)
        .env("SSH_ASKPASS_PROMPT", "none")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let req = next_request(&mut requests).await;
    let to_request = started.elapsed();
    assert_eq!(req.prompt, "Enter passphrase for key 'k': ");
    assert_eq!(req.hint.as_deref(), Some("none"));
    req.reply.send(PromptAnswer::Text("pp".into())).unwrap();
    let out = child.wait_with_output().await.unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stdout, b"pp\n");
    eprintln!("helper process start to request: {to_request:?}");
}

/// The answer ends in a bare `\n`, on Windows too: Windows' OpenSSH keeps a `\r` as part of the
/// answer, so `yes\r\n` to the host-key question loops on "Please type 'yes', 'no' or the
/// fingerprint" (seen on the windows-2025 runner).
#[tokio::test(flavor = "multi_thread")]
async fn helper_answer_has_no_carriage_return() {
    let (server, mut requests) = bind();
    let env = server.register(HostId("prod".into()), Path::new(HELPER)).unwrap();
    let prompt = "Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
    let child = tokio::process::Command::new(HELPER)
        .arg(prompt)
        .env("DROMON_ASKPASS_ENDPOINT", &env.endpoint)
        .env("DROMON_ASKPASS_TOKEN", &env.token)
        .env_remove("SSH_ASKPASS_PROMPT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let req = next_request(&mut requests).await;
    assert_eq!((req.prompt.as_str(), req.hint.as_deref()), (prompt, None));
    req.reply.send(PromptAnswer::Text("yes".into())).unwrap();
    let out = child.wait_with_output().await.unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(!out.stdout.contains(&b'\r'), "{:?}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(out.stdout, b"yes\n");
}

/// The `ssh-keygen` next to the `ssh` Dromon runs (Windows: the inbox OpenSSH if present).
fn ssh_keygen() -> PathBuf {
    ssh_program().with_file_name(if cfg!(windows) { "ssh-keygen.exe" } else { "ssh-keygen" })
}

/// OpenSSH's `read_passphrase` uses `SSH_ASKPASS` for every prompt under
/// `SSH_ASKPASS_REQUIRE=force`, ssh-keygen's included, so reading the public half of an
/// encrypted key asks our helper for the passphrase without any SSH server.
#[tokio::test(flavor = "multi_thread")]
async fn openssh_asks_for_a_passphrase_through_the_helper() {
    let tmp = tempfile::tempdir().unwrap();
    let key = tmp.path().join("k");
    let keygen = ssh_keygen();
    let made = std::process::Command::new(&keygen)
        .args(["-q", "-t", "ed25519", "-N", "s3cret", "-C", "dromon-test", "-f"])
        .arg(&key)
        .stdin(Stdio::null())
        .output();
    let made = match made {
        // On CI the runners ship OpenSSH: a missing ssh-keygen there is a failure, not a skip,
        // so a green Windows run means Windows' own OpenSSH really asked our helper.
        Err(e) if e.kind() == io::ErrorKind::NotFound && std::env::var_os("CI").is_none() => {
            eprintln!("skipped: no {}", keygen.display());
            return;
        }
        other => other.unwrap(),
    };
    assert!(made.status.success(), "ssh-keygen: {}", String::from_utf8_lossy(&made.stderr));

    let (server, mut requests) = bind();
    let env = server.register(HostId("local".into()), Path::new(HELPER)).unwrap();
    let mut child = tokio::process::Command::new(&keygen)
        .arg("-y")
        .arg("-f")
        .arg(&key)
        .envs(env.vars())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let req = match tokio::time::timeout(Duration::from_secs(30), requests.recv()).await {
        Ok(Some(req)) => req,
        _ => {
            let _ = child.start_kill();
            let out = child.wait_with_output().await.unwrap();
            panic!(
                "no askpass request from {} ({:?}); stderr: {}",
                keygen.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    };
    assert_eq!(req.host, HostId("local".into()));
    assert!(req.prompt.to_lowercase().contains("passphrase"), "{:?}", req.prompt);
    req.reply.send(PromptAnswer::Text("s3cret".into())).unwrap();

    let out = child.wait_with_output().await.unwrap();
    assert!(out.status.success(), "{:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    let printed = String::from_utf8_lossy(&out.stdout);
    let public = std::fs::read_to_string(tmp.path().join("k.pub")).unwrap();
    let fields = |s: &str| s.split_whitespace().take(2).map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(fields(&printed), fields(&public));
}
