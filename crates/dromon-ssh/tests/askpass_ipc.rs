//! Askpass round trips over a real Unix socket: the server in a tokio runtime, the helper logic
//! on a blocking thread (and once as a real process).

#![cfg(unix)]

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dromon_core::{HostId, PromptAnswer};
use dromon_ssh::{AskpassRequest, AskpassServer, run_helper_with};
use tokio::sync::mpsc;

const EXE: &str = "/opt/dromon/dromon";

struct Fixture {
    _tmp: tempfile::TempDir,
    server: AskpassServer,
    requests: mpsc::Receiver<AskpassRequest>,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let mut server = AskpassServer::bind_in(&tmp.path().join("rt")).unwrap();
    let requests = server.take_requests().unwrap();
    Fixture { _tmp: tmp, server, requests }
}

/// Runs the helper logic on a blocking thread; returns (exit code, stdout bytes).
fn helper(endpoint: String, token: String, prompt: &str, hint: Option<&str>) -> tokio::task::JoinHandle<(i32, Vec<u8>)> {
    let prompt = prompt.to_owned();
    let hint = hint.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let code = run_helper_with(&endpoint, &token, &prompt, hint.as_deref(), &mut out);
        (code, out)
    })
}

async fn next_request(rx: &mut mpsc::Receiver<AskpassRequest>) -> AskpassRequest {
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("request in time").expect("server alive")
}

#[tokio::test(flavor = "multi_thread")]
async fn answer_round_trip() {
    let mut f = fixture();
    let env = f.server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    assert_eq!(env.exe, PathBuf::from(EXE));
    assert_eq!(env.endpoint, f.server.endpoint());
    assert_eq!(env.token.len(), 64);

    let task = helper(env.endpoint.clone(), env.token.clone(), "deploy@10.0.0.4's password: ", None);
    let req = next_request(&mut f.requests).await;
    assert_eq!(req.host, HostId("prod".into()));
    assert_eq!(req.prompt, "deploy@10.0.0.4's password: ");
    assert_eq!(req.hint, None);
    req.reply.send(PromptAnswer::Text("hunter2".into())).unwrap();

    let (code, out) = task.await.unwrap();
    assert_eq!(code, 0);
    assert_eq!(out, b"hunter2\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn hint_and_multiline_prompt_arrive_intact() {
    let mut f = fixture();
    let env = f.server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    let prompt = "The authenticity of host 'prod (10.0.0.4)' can't be established.\nAre you sure (yes/no/[fingerprint])? ";
    let task = helper(env.endpoint.clone(), env.token.clone(), prompt, Some("confirm"));
    let req = next_request(&mut f.requests).await;
    assert_eq!(req.prompt, prompt);
    assert_eq!(req.hint.as_deref(), Some("confirm"));
    req.reply.send(PromptAnswer::Text("yes".into())).unwrap();
    assert_eq!(task.await.unwrap(), (0, b"yes\n".to_vec()));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_exits_1_with_no_output() {
    let mut f = fixture();
    let env = f.server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    let task = helper(env.endpoint.clone(), env.token.clone(), "Password: ", None);
    let req = next_request(&mut f.requests).await;
    req.reply.send(PromptAnswer::Cancel).unwrap();
    assert_eq!(task.await.unwrap(), (1, Vec::new()));
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_request_cancels() {
    let mut f = fixture();
    let env = f.server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    let task = helper(env.endpoint.clone(), env.token.clone(), "Password: ", None);
    drop(next_request(&mut f.requests).await);
    assert_eq!(task.await.unwrap(), (1, Vec::new()));
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_token_is_rejected_and_nothing_is_forwarded() {
    let mut f = fixture();
    let env = f.server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    let mut wrong = env.token.clone();
    wrong.replace_range(0..1, if wrong.starts_with('0') { "1" } else { "0" });
    for token in [wrong, String::new(), "x".repeat(64)] {
        let (code, out) = helper(env.endpoint.clone(), token, "Password: ", None).await.unwrap();
        assert_eq!(code, 1);
        assert!(out.is_empty());
    }
    assert!(f.requests.try_recv().is_err(), "no request may reach the app");
}

#[tokio::test(flavor = "multi_thread")]
async fn unregistered_token_is_rejected() {
    let mut f = fixture();
    let env = f.server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    f.server.unregister(&env.token);
    let (code, out) = helper(env.endpoint.clone(), env.token.clone(), "Password: ", None).await.unwrap();
    assert_eq!((code, out.is_empty()), (1, true));
    assert!(f.requests.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_map_to_their_own_hosts() {
    let mut f = fixture();
    let a = f.server.register(HostId("a".into()), Path::new(EXE)).unwrap();
    let b = f.server.register(HostId("b".into()), Path::new(EXE)).unwrap();
    assert_ne!(a.token, b.token);
    let ta = helper(a.endpoint.clone(), a.token.clone(), "pa", None);
    let ra = next_request(&mut f.requests).await;
    let tb = helper(b.endpoint.clone(), b.token.clone(), "pb", None);
    let rb = next_request(&mut f.requests).await;
    assert_eq!((ra.host.as_str(), ra.prompt.as_str()), ("a", "pa"));
    assert_eq!((rb.host.as_str(), rb.prompt.as_str()), ("b", "pb"));
    rb.reply.send(PromptAnswer::Text("B".into())).unwrap();
    ra.reply.send(PromptAnswer::Text("A".into())).unwrap();
    assert_eq!(ta.await.unwrap(), (0, b"A\n".to_vec()));
    assert_eq!(tb.await.unwrap(), (0, b"B\n".to_vec()));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_endpoint_fails_cleanly() {
    let f = fixture();
    let env = f.server.register(HostId("prod".into()), Path::new(EXE)).unwrap();
    let gone = format!("{}.gone", env.endpoint);
    assert_eq!(helper(gone, env.token.clone(), "p", None).await.unwrap(), (1, Vec::new()));
    assert_eq!(helper(String::new(), env.token.clone(), "p", None).await.unwrap(), (1, Vec::new()));
}

#[tokio::test(flavor = "multi_thread")]
async fn socket_dir_is_private_and_socket_is_removed_on_drop() {
    let f = fixture();
    let endpoint = PathBuf::from(f.server.endpoint());
    let dir = endpoint.parent().unwrap();
    assert_eq!(std::fs::metadata(dir).unwrap().mode() & 0o777, 0o700);
    assert_eq!(std::fs::symlink_metadata(&endpoint).unwrap().mode() & 0o777, 0o600);
    let name = endpoint.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with(&format!("askpass-{}-", std::process::id())) && name.ends_with(".sock"), "{name}");
    drop(f.server);
    assert!(!endpoint.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn default_bind_uses_a_private_dir() {
    let server = AskpassServer::bind().unwrap();
    let endpoint = PathBuf::from(server.endpoint());
    let dir = endpoint.parent().unwrap();
    let meta = std::fs::symlink_metadata(dir).unwrap();
    assert!(meta.is_dir());
    assert_eq!(meta.mode() & 0o777, 0o700);
    // `$XDG_RUNTIME_DIR/dromon` when that is usable, else `<temp>/dromon-<uid>`.
    let name = dir.file_name().unwrap().to_str().unwrap();
    assert!(name == "dromon" || name == format!("dromon-{}", meta.uid()), "{name}");
    drop(server);
    assert!(!endpoint.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_loose_dir_is_tightened() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("rt");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _server = AskpassServer::bind_in(&dir).unwrap();
    assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinked_or_file_dir_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let err = AskpassServer::bind_in(&link).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    let file = tmp.path().join("file");
    std::fs::write(&file, b"").unwrap();
    assert!(AskpassServer::bind_in(&file).is_err());
}

/// The helper binary (`src/bin/dromon-askpass-test-helper.rs`), which cargo builds for tests.
const HELPER: &str = env!("CARGO_BIN_EXE_dromon-askpass-test-helper");

#[tokio::test(flavor = "multi_thread")]
async fn real_helper_process_reads_env_and_argv() {
    let bin = PathBuf::from(HELPER);
    let mut f = fixture();
    let env = f.server.register(HostId("prod".into()), &bin).unwrap();
    let started = Instant::now();
    let child = tokio::process::Command::new(&bin)
        .arg("Enter passphrase for key '/k': ")
        .env_clear()
        .env("DROMON_ASKPASS_ENDPOINT", &env.endpoint)
        .env("DROMON_ASKPASS_TOKEN", &env.token)
        .env("SSH_ASKPASS_PROMPT", "none")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let req = next_request(&mut f.requests).await;
    let to_request = started.elapsed();
    assert_eq!(req.prompt, "Enter passphrase for key '/k': ");
    assert_eq!(req.hint.as_deref(), Some("none"));
    req.reply.send(PromptAnswer::Text("pp".into())).unwrap();
    let out = child.wait_with_output().await.unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stdout, b"pp\n");
    eprintln!("helper process start to request: {to_request:?}");
    assert!(to_request < Duration::from_secs(2), "{to_request:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn real_helper_process_without_env_exits_1() {
    let out = tokio::process::Command::new(HELPER).arg("p").env_clear().output().await.unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}
