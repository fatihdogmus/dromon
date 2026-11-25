use super::*;

// Trimmed real output of `ssh -F /dev/null -G -J a@b:2222,c foo -p 2200 -l bob` (OpenSSH 10.2).
const REAL: &str = "\
user bob
hostname foo
port 2200
addressfamily any
batchmode no
identityfile ~/.ssh/id_rsa
identityfile ~/.ssh/id_ecdsa
identityfile ~/.ssh/id_ecdsa_sk
identityfile ~/.ssh/id_ed25519
identityfile ~/.ssh/id_ed25519_sk
proxyjump a@b:2222,c
userknownhostsfile /home/u/.ssh/known_hosts /home/u/.ssh/known_hosts2
";

#[test]
fn parses_real_output() {
    let r = parse_ssh_g(REAL);
    assert_eq!(r.hostname.as_deref(), Some("foo"));
    assert_eq!(r.user.as_deref(), Some("bob"));
    assert_eq!(r.port, Some(2200));
    assert_eq!(r.proxy_jump.as_deref(), Some("a@b:2222,c"));
    assert_eq!(
        r.identity_files,
        ["~/.ssh/id_rsa", "~/.ssh/id_ecdsa", "~/.ssh/id_ecdsa_sk", "~/.ssh/id_ed25519", "~/.ssh/id_ed25519_sk"]
    );
}

#[test]
fn proxyjump_none_means_no_jump_host() {
    assert_eq!(parse_ssh_g("proxyjump none\n").proxy_jump, None);
    assert_eq!(parse_ssh_g("proxyjump NONE\n").proxy_jump, None);
}

#[test]
fn missing_keys_stay_none() {
    assert_eq!(parse_ssh_g(""), Resolved::default());
    assert_eq!(parse_ssh_g("compression no\n"), Resolved::default());
}

#[test]
fn tolerates_crlf_bad_port_and_blank_values() {
    let r = parse_ssh_g("hostname h\r\nport notanumber\r\nuser \r\nport\r\n");
    assert_eq!(r.hostname.as_deref(), Some("h"));
    assert_eq!(r.port, None);
    assert_eq!(r.user, None);
}

#[test]
fn identity_file_with_spaces_is_kept_whole() {
    let r = parse_ssh_g("identityfile /home/u/my keys/id\n");
    assert_eq!(r.identity_files, ["/home/u/my keys/id"]);
}

#[cfg(unix)]
fn fake_ssh(dir: &Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-ssh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Runs a freshly written script. Another test forking at the moment the script was written can
/// briefly hold its fd open, making exec fail with ETXTBSY; retry that.
#[cfg(unix)]
async fn resolve_fresh(ssh: &Path, alias: &str, timeout: Duration) -> Result<Resolved, Error> {
    for _ in 0..100 {
        match resolve_with_timeout(ssh, alias, timeout).await {
            Err(Error::Io(m)) if m.contains("os error 26") => tokio::time::sleep(Duration::from_millis(20)).await,
            other => return other,
        }
    }
    panic!("{} stayed busy", ssh.display());
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_runs_ssh_dash_g() {
    let tmp = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(
        tmp.path(),
        r#"[ "$1" = "-G" ] || exit 9
printf 'hostname %s.example\nuser u\nport 2222\nproxyjump none\n' "$2""#,
    );
    let r = resolve_fresh(&ssh, "prod", RESOLVE_TIMEOUT).await.unwrap();
    assert_eq!(r.hostname.as_deref(), Some("prod.example"));
    assert_eq!(r.user.as_deref(), Some("u"));
    assert_eq!(r.port, Some(2222));
    assert_eq!(r.proxy_jump, None);
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_reports_ssh_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(tmp.path(), "echo 'hostname contains invalid characters' >&2; exit 255");
    let err = resolve_fresh(&ssh, "x", RESOLVE_TIMEOUT).await.unwrap_err();
    assert_eq!(err, Error::Other("ssh -G x failed: hostname contains invalid characters".into()));
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_times_out() {
    let tmp = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(tmp.path(), "exec sleep 30");
    let start = std::time::Instant::now();
    let err = resolve_fresh(&ssh, "x", Duration::from_millis(200)).await.unwrap_err();
    assert!(matches!(err, Error::Other(m) if m.contains("took too long")));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn resolve_missing_program_is_an_io_error() {
    let err = resolve(Path::new("/nonexistent/dromon-ssh"), "x").await.unwrap_err();
    assert!(matches!(err, Error::Io(_)));
}

#[tokio::test]
async fn resolve_rejects_option_injection() {
    let err = resolve(Path::new("/nonexistent/dromon-ssh"), "-oProxyCommand=evil").await.unwrap_err();
    assert!(matches!(err, Error::Connect(_)));
}
