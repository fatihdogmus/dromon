use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

fn sh(script: &str) -> SshCommand {
    SshCommand {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), script.into()],
        env: vec![(OsString::from("DROMON_TEST_VAR"), OsString::from("hello"))],
        env_remove: vec![],
    }
}

async fn next_line(rx: &mut mpsc::Receiver<String>) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("stderr line in time")
}

#[tokio::test]
async fn stream_carries_stdin_and_stdout() {
    let mut child = spawn(&sh("printf 'ready\\n'; head -c 5")).await.unwrap();
    let mut stream = child.take_stream().unwrap();
    assert!(child.take_stream().is_none());

    let mut ready = [0u8; 6];
    stream.read_exact(&mut ready).await.unwrap();
    assert_eq!(&ready, b"ready\n");
    stream.write_all(b"hello").await.unwrap();
    stream.flush().await.unwrap();
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).await.unwrap();
    assert_eq!(echoed, b"hello");
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn stderr_arrives_line_by_line_with_env() {
    let script = "printf 'one\\r\\n' >&2; echo \"$DROMON_TEST_VAR\" >&2; printf 'bad \\377 utf8\\n' >&2; printf 'no newline' >&2";
    let mut child = spawn(&sh(script)).await.unwrap();
    let mut rx = child.take_stderr().unwrap();
    assert_eq!(next_line(&mut rx).await.as_deref(), Some("one"));
    assert_eq!(next_line(&mut rx).await.as_deref(), Some("hello"));
    assert_eq!(next_line(&mut rx).await.as_deref(), Some("bad \u{fffd} utf8"));
    assert_eq!(next_line(&mut rx).await.as_deref(), Some("no newline"));
    assert_eq!(next_line(&mut rx).await, None);
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn exit_status_is_reported() {
    let mut child = spawn(&sh("exit 255")).await.unwrap();
    assert_eq!(child.wait().await.unwrap().code(), Some(255));
}

#[tokio::test]
async fn kill_stops_a_running_child() {
    let mut child = spawn(&sh("exec sleep 30")).await.unwrap();
    assert!(child.id().is_some());
    assert!(child.try_wait().unwrap().is_none());
    tokio::time::timeout(Duration::from_secs(5), child.kill()).await.unwrap().unwrap();
    assert!(child.try_wait().unwrap().is_some());
}

#[tokio::test]
async fn unread_stderr_never_blocks_the_child() {
    // 5000 lines is more than the channel holds and more than a pipe buffer.
    let mut child = spawn(&sh("i=0; while [ $i -lt 5000 ]; do echo \"line $i\" >&2; i=$((i+1)); done")).await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(20), child.wait()).await.unwrap().unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn missing_program_fails_to_spawn() {
    let cmd = SshCommand { program: PathBuf::from("/nonexistent/ssh"), args: vec![], env: vec![], env_remove: vec![] };
    assert!(spawn(&cmd).await.is_err());
}

#[tokio::test]
async fn removed_variables_are_not_inherited_unless_set() {
    async fn child_env(cmd: SshCommand) -> Vec<String> {
        let mut child = spawn(&cmd).await.unwrap();
        let mut out = Vec::new();
        child.take_stream().unwrap().read_to_end(&mut out).await.unwrap();
        assert!(child.wait().await.unwrap().success());
        String::from_utf8(out).unwrap().lines().map(str::to_owned).collect()
    }
    let env_cmd = || SshCommand {
        program: PathBuf::from("/usr/bin/env"),
        args: vec![],
        env: vec![(OsString::from("DROMON_TEST_VAR"), OsString::from("hello"))],
        env_remove: vec![],
    };

    // PATH and HOME are inherited by every test process; one is removed, one removed and set.
    assert!(std::env::var_os("PATH").is_some() && std::env::var_os("HOME").is_some());
    let mut cmd = env_cmd().without_env("PATH").without_env("HOME").without_env("DROMON_TEST_VAR");
    cmd.env.push((OsString::from("HOME"), OsString::from("/set-again")));
    let env = child_env(cmd).await;
    assert!(!env.iter().any(|l| l.starts_with("PATH=")), "{env:?}");
    assert!(env.iter().any(|l| l == "HOME=/set-again"), "{env:?}");
    assert!(env.iter().any(|l| l == "DROMON_TEST_VAR=hello"), "{env:?}");

    // Nothing is removed by default.
    let env = child_env(env_cmd()).await;
    assert!(env.iter().any(|l| l.starts_with("PATH=")) && env.iter().any(|l| l.starts_with("HOME=")), "{env:?}");
}
