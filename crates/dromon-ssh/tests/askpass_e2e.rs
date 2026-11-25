//! Spike S3 end to end: the real system `ssh` (`ssh_program()`: on Windows the inbox OpenSSH),
//! launched exactly as Dromon launches it (`SshCommand::sftp` + `spawn`), sends every prompt
//! through the askpass helper binary to an `AskpassServer` (a Unix socket, on Windows a named
//! pipe); the test answers them with `classify_prompt` + `AuthAttempt` + `PasswordStore` as the
//! service thread will, and ends in a working `Sftp` session.
//!
//! Uses the sshd fixture (`dromon-testkit`, one server for this test binary: the Docker image of
//! `tests/docker/`, or with `DROMON_FIXTURE_BACKEND=native` the machine's own sshd; users and
//! credentials: `tests/docker/Dockerfile`, names through `Fixture::user`). Runs whenever the
//! backend can; otherwise each test prints a skip line and passes (fails with
//! `DROMON_REQUIRE_DOCKER=1`). On Windows that is a Linux Docker named by `DOCKER_HOST` (CI:
//! Docker in WSL2, `tests/docker/README.md`). The TOTP tests need the Docker backend.
//!
//! Isolation from the developer's own ssh setup, test-only: a per-test ssh config passed with
//! `-F` (so `~/.ssh/config` and `/etc/ssh/ssh_config` are not read) pointing at a per-test
//! `known_hosts`, `IdentitiesOnly yes`, and `SSH_AUTH_SOCK` removed from ssh's environment.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::time::{Duration, Instant};

use dromon_core::{Error, HostId, HostSource, HostSpec, PromptAnswer, PromptKind, RemotePath};
use dromon_sftp::Sftp;
use dromon_ssh::{
    AskpassRequest, AskpassServer, AuthAttempt, AuthDecision, PasswordStore, SshChild, SshCommand, WRONG_PASSPHRASE,
    WRONG_PASSWORD, WRONG_SIGN_IN, classify_prompt, password_identity, spawn,
};
use dromon_testkit::{Backend, Feature, NULL_DEVICE, PASSPHRASE, PASSWORD, Role, totp_now, wrong_totp};
use secrecy::ExposeSecret;
use tokio::sync::mpsc;

const HELPER: &str = env!("CARGO_BIN_EXE_dromon-askpass-test-helper");
/// How long one connection may take, prompts included.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// ssh `-v` logs this right before it forks the askpass helper (`readpass.c`).
const ASKPASS_EXEC_LINE: &str = "read_passphrase: requested to askpass";

/// An absolute path without `..` or symlinks, as ssh will print it in prompts.
#[cfg(unix)]
fn key_file(path: &Path) -> String {
    let path = std::fs::canonicalize(path).expect("fixture key");
    path.to_str().unwrap().to_owned()
}

/// An absolute path, as ssh will print it in prompts. Not `canonicalize`: on Windows it returns
/// a `\\?\C:\…` verbatim path, which ssh can't read back from a config value.
#[cfg(windows)]
fn key_file(path: &Path) -> String {
    let path = std::path::absolute(path).expect("fixture key");
    path.to_str().unwrap().to_owned()
}

/// This test's askpass endpoint: a socket in its own folder, on Windows a fresh named pipe.
#[cfg(unix)]
fn bind_askpass(tmp: &Path) -> AskpassServer {
    AskpassServer::bind_in(&tmp.join("rt")).unwrap()
}

#[cfg(windows)]
fn bind_askpass(_tmp: &Path) -> AskpassServer {
    AskpassServer::bind().unwrap()
}

fn text(s: &str) -> PromptAnswer {
    PromptAnswer::Text(s.into())
}

/// What the answer loop saw for one prompt.
#[derive(Debug)]
struct Seen {
    kind: PromptKind,
    prompt: String,
    hint: Option<String>,
    /// `Some(error)` when the user was asked (with that inline error), `None` when answered
    /// from memory.
    asked: Option<Option<String>>,
}

/// The outcome of one connection attempt.
struct Outcome {
    sftp: Result<Sftp, Error>,
    seen: Vec<Seen>,
    stderr: Vec<String>,
    child: SshChild,
}

impl Outcome {
    fn kinds(&self) -> Vec<&PromptKind> {
        self.seen.iter().map(|s| &s.kind).collect()
    }

    fn asked(&self) -> usize {
        self.seen.iter().filter(|s| s.asked.is_some()).count()
    }

    /// The SFTP session, or a panic that shows ssh's stderr.
    fn ready(&self) -> &Sftp {
        match &self.sftp {
            Ok(sftp) => sftp,
            Err(e) => panic!("connection failed: {e}\nprompts: {:#?}\nssh stderr:\n{}", self.seen, self.stderr.join("\n")),
        }
    }
}

/// The "user" in front of the sheet: called for every `AskUser` decision.
type User<'a> = &'a mut dyn FnMut(&PromptKind, &str, Option<&str>) -> PromptAnswer;

/// One test's askpass server, password store and private ssh files, on the shared sshd.
struct Fixture {
    name: &'static str,
    sshd: &'static dromon_testkit::Fixture,
    tmp: tempfile::TempDir,
    server: AskpassServer,
    requests: mpsc::Receiver<AskpassRequest>,
    store: PasswordStore,
    connections: u32,
}

impl Fixture {
    async fn new(name: &'static str) -> Option<Fixture> {
        let sshd = dromon_testkit::fixture(name).await?;
        let tmp = tempfile::tempdir().unwrap();
        let mut server = bind_askpass(tmp.path());
        let requests = server.take_requests().unwrap();
        Some(Fixture { name, sshd, tmp, server, requests, store: PasswordStore::new(), connections: 0 })
    }

    fn port(&self) -> u16 {
        self.sshd.port()
    }

    /// `<user of role>@<host>`, as ssh names the identity in prompts.
    fn at(&self, role: Role, host: &str) -> String {
        format!("{}@{host}", self.sshd.user(role))
    }

    fn home(&self, role: Role) -> RemotePath {
        self.sshd.home(role)
    }

    /// The unencrypted fixture key, as ssh prints it.
    fn plain_key(&self) -> String {
        key_file(&self.sshd.key())
    }

    fn known_hosts(&self) -> PathBuf {
        self.tmp.path().join("known_hosts")
    }

    /// A recent quick-connect host on the fixture.
    fn host(&self, role: Role, identity_file: Option<String>) -> HostSpec {
        let user = self.sshd.user(role);
        let id = format!("{user}@127.0.0.1:{}", self.port());
        HostSpec {
            id: HostId(id),
            source: HostSource::Recent,
            alias: format!("dromon-fixture-{user}"),
            hostname: Some("127.0.0.1".into()),
            user: Some(user.into()),
            port: Some(self.port()),
            identity_file,
            proxy_jump: None,
        }
    }

    /// Writes the test-only ssh config: `extra` lines, then the isolation settings (ssh keeps
    /// the first value it reads, so `extra` wins).
    fn config_file(&mut self, extra: &str) -> PathBuf {
        self.connections += 1;
        let path = self.tmp.path().join(format!("ssh_config_{}", self.connections));
        let mut config = String::from("Host *\n");
        for line in extra.lines().map(str::trim).filter(|l| !l.is_empty()) {
            writeln!(config, "    {line}").unwrap();
        }
        let known_hosts = self.known_hosts();
        for line in [
            &format!("UserKnownHostsFile {}", known_hosts.display()) as &str,
            &format!("GlobalKnownHostsFile {NULL_DEVICE}"),
            "StrictHostKeyChecking ask",
            "UpdateHostKeys no",
            "HashKnownHosts no",
            "IdentitiesOnly yes",
            "AddKeysToAgent no",
            "NumberOfPasswordPrompts 3",
        ] {
            writeln!(config, "    {line}").unwrap();
        }
        std::fs::write(&path, config).unwrap();
        path
    }

    /// The encrypted fixture key, copied (with its `.pub`; still private to us) to a short path
    /// in this test's folder: ssh cuts the path in its passphrase prompt at 100 bytes
    /// (`%.100s`), and the temp folder's path may be longer.
    fn encrypted_key(&self) -> String {
        let src = PathBuf::from(key_file(&self.sshd.encrypted_key()));
        let dst = self.tmp.path().join("id_ed25519_enc");
        dromon_testkit::copy_private_key(&src, &dst).unwrap();
        std::fs::copy(src.with_extension("pub"), dst.with_extension("pub")).unwrap();
        let path = dst.to_str().unwrap().to_owned();
        assert!(path.len() <= 100, "{path}");
        path
    }

    /// One connection attempt: ssh as the app launches it, plus `-F <test config>`, without the
    /// user's agent (or with `agent` instead); every prompt answered through `AuthAttempt`.
    async fn connect(&mut self, host: &HostSpec, config: &str, agent: Option<&Path>, user: User<'_>) -> Outcome {
        let config = self.config_file(config);
        let env = self.server.register(host.id.clone(), Path::new(HELPER)).unwrap();
        let mut cmd = SshCommand::sftp(host, &env, true).unwrap().with_config_file(&config).unwrap().without_env("SSH_AUTH_SOCK");
        if let Some(agent) = agent {
            cmd.env.push(("SSH_AUTH_SOCK".into(), agent.into()));
        }
        let mut child = spawn(&cmd).await.expect("spawn ssh");
        let stream = child.take_stream().unwrap();
        let mut stderr_rx = Some(child.take_stderr().unwrap());

        let mut attempt = AuthAttempt::new(host.id.clone());
        let mut seen = Vec::new();
        let mut stderr = Vec::new();
        let mut helper_exec: Option<Instant> = None;
        let mut sftp = pin!(Sftp::new(stream));
        let deadline = tokio::time::sleep(CONNECT_TIMEOUT);
        let mut deadline = pin!(deadline);
        let sftp = loop {
            tokio::select! {
                biased;
                line = async { stderr_rx.as_mut().unwrap().recv().await }, if stderr_rx.is_some() => {
                    let Some(line) = line else { stderr_rx = None; continue };
                    if line.contains(ASKPASS_EXEC_LINE) {
                        helper_exec = Some(Instant::now());
                    }
                    on_stderr_line(&line, &mut attempt, &mut self.store);
                    stderr.push(line);
                }
                req = self.requests.recv() => {
                    let req = req.expect("askpass server alive");
                    let latency = helper_exec.take().map(|t| t.elapsed());
                    assert_eq!(req.host, host.id);
                    let kind = classify_prompt(&req.prompt, req.hint.as_deref());
                    let (answer, asked) = match attempt.on_prompt(&kind, &req.prompt, &mut self.store) {
                        AuthDecision::AnswerFromMemory(secret) => (text(secret.expose_secret()), None),
                        AuthDecision::AskUser { error } => {
                            let answer = user(&kind, &req.prompt, error.as_deref());
                            attempt.on_user_answer(&kind, &req.prompt, &answer, &mut self.store);
                            (answer, Some(error))
                        }
                    };
                    let how = match (&asked, &answer) {
                        (None, _) => "answered from memory".to_owned(),
                        (Some(error), PromptAnswer::Cancel) => format!("asked (error {error:?}), user cancelled"),
                        (Some(error), PromptAnswer::Text(_)) => format!("asked (error {error:?}), user answered"),
                    };
                    let latency = latency.map_or_else(|| "n/a".to_owned(), |l| format!("{:.2} ms", l.as_secs_f64() * 1000.0));
                    println!(
                        "[{}] prompt {kind:?}\n    hint: {:?}\n    text: {:?}\n    {how}; ssh's askpass exec → request arrival: {latency}",
                        self.name, req.hint, req.prompt
                    );
                    let _ = req.reply.send(answer);
                    seen.push(Seen { kind, prompt: req.prompt, hint: req.hint, asked });
                }
                result = &mut sftp => break result,
                () = &mut deadline => panic!("[{}] no SFTP session after {CONNECT_TIMEOUT:?}:\n{}", self.name, stderr.join("\n")),
            }
        };
        self.server.unregister(&env.token);
        // After a failure ssh is exiting: collect the rest of its stderr for the assertions.
        if sftp.is_err()
            && let Some(mut rx) = stderr_rx
        {
            while let Ok(Some(line)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                on_stderr_line(&line, &mut attempt, &mut self.store);
                stderr.push(line);
            }
        }
        Outcome { sftp, seen, stderr, child }
    }
}

/// `<user>@<host>: Permission denied (<methods>).` means ssh gave up on authentication.
fn on_stderr_line(line: &str, attempt: &mut AuthAttempt, store: &mut PasswordStore) {
    if line.contains("Permission denied (") {
        attempt.on_auth_failed(store);
    }
}

/// The fixture's host key fingerprint, read from the host key itself on the server
/// (`ssh-keygen -l` there). Not `ssh-keyscan` here: Windows' 9.5 offers a key exchange it
/// can't do (sntrup761x25519) and gives up ("choose_kex: unsupported KEX method").
fn fixture_fingerprint(sshd: &dromon_testkit::Fixture) -> String {
    let line = sshd.exec(&["ssh-keygen", "-l", "-E", "sha256", "-f", &sshd.host_public_key()]).unwrap();
    // `256 SHA256:… root@<container> (ED25519)`
    let fp = line.split_whitespace().nth(1).unwrap().to_owned();
    assert!(fp.starts_with("SHA256:"), "{line}");
    fp
}

fn host_key(kind: &PromptKind) -> bool {
    matches!(kind, PromptKind::HostKey { .. })
}

/// Accepts host keys; anything else is unexpected.
fn accept_host_key_only(kind: &PromptKind, prompt: &str, _error: Option<&str>) -> PromptAnswer {
    assert!(host_key(kind), "unexpected prompt {kind:?}: {prompt:?}");
    text("yes")
}

const PASSWORD_ONLY: &str = "PubkeyAuthentication no\nPreferredAuthentications password";
const PUBKEY_ONLY: &str = "PubkeyAuthentication yes\nPreferredAuthentications publickey";

#[tokio::test(flavor = "multi_thread")]
async fn unknown_host_key_is_asked_once_then_known() {
    let Some(mut f) = Fixture::new("unknown_host_key_is_asked_once_then_known").await else { return };
    let expected = fixture_fingerprint(f.sshd);
    let host = f.host(Role::Key, Some(f.plain_key()));

    let first = f.connect(&host, PUBKEY_ONLY, None, &mut accept_host_key_only).await;
    let sftp = first.ready();
    assert_eq!(sftp.home().await.unwrap(), f.home(Role::Key));
    assert!(!sftp.list(&f.home(Role::Key).join("fixture")).await.unwrap().is_empty());
    assert_eq!(first.seen.len(), 1, "{:#?}", first.seen);
    let s = &first.seen[0];
    assert_eq!(s.kind, PromptKind::HostKey { key_type: Some("ED25519".into()), fingerprint: Some(expected.clone()) });
    assert_eq!(s.hint, None);
    assert!(s.prompt.starts_with(&format!("The authenticity of host '[127.0.0.1]:{0} ([127.0.0.1]:{0})'", f.port())));
    assert!(s.prompt.contains(&expected));
    assert!(s.prompt.ends_with("(yes/no/[fingerprint])? "));
    assert_eq!(s.asked, Some(None));
    let known = std::fs::read_to_string(f.known_hosts()).unwrap();
    assert!(known.starts_with(&format!("[127.0.0.1]:{} ssh-ed25519 ", f.port())), "{known}");

    let second = f.connect(&host, PUBKEY_ONLY, None, &mut accept_host_key_only).await;
    assert_eq!(second.ready().home().await.unwrap(), f.home(Role::Key));
    assert!(second.seen.is_empty(), "{:#?}", second.seen);
    println!("host key {expected} matched ssh-keyscan; second connection had no prompts");
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_host_key_ends_the_connection() {
    let Some(mut f) = Fixture::new("rejected_host_key_ends_the_connection").await else { return };
    let host = f.host(Role::Key, Some(f.plain_key()));
    let mut user = |kind: &PromptKind, prompt: &str, _: Option<&str>| {
        assert!(host_key(kind), "{prompt:?}");
        PromptAnswer::Cancel
    };
    let mut out = f.connect(&host, PUBKEY_ONLY, None, &mut user).await;
    assert!(matches!(out.sftp, Err(Error::Connect(_))), "{:?}", out.sftp.as_ref().err());
    assert_eq!(out.seen.len(), 1);
    assert_eq!(out.child.wait().await.unwrap().code(), Some(255));
    assert!(out.stderr.iter().any(|l| l == "Host key verification failed."), "{:#?}", out.stderr);
    assert!(std::fs::read_to_string(f.known_hosts()).unwrap_or_default().is_empty());
    println!("rejected host key: {:?}", out.sftp.as_ref().err().unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn password_first_try_then_from_memory() {
    let Some(mut f) = Fixture::new("password_first_try_then_from_memory").await else { return };
    let host = f.host(Role::Password, None);
    let pw = f.at(Role::Password, "127.0.0.1");
    let mut user = |kind: &PromptKind, prompt: &str, error: Option<&str>| match kind {
        PromptKind::HostKey { .. } => text("yes"),
        PromptKind::Password => {
            assert_eq!(error, None);
            text(PASSWORD)
        }
        other => panic!("unexpected {other:?}: {prompt:?}"),
    };
    let first = f.connect(&host, PASSWORD_ONLY, None, &mut user).await;
    assert_eq!(first.ready().home().await.unwrap(), f.home(Role::Password));
    assert!(host_key(first.kinds()[0]));
    assert_eq!(first.kinds()[1..], [&PromptKind::Password]);
    assert_eq!(first.seen[1].prompt, format!("{pw}'s password: "));
    assert_eq!(first.seen[1].hint, None);
    assert_eq!(f.store.get(&host.id, &pw).unwrap().expose_secret(), PASSWORD);

    // Reconnect: answered from memory, the user is never asked.
    let mut never = |kind: &PromptKind, prompt: &str, _: Option<&str>| -> PromptAnswer {
        panic!("the user was asked {kind:?}: {prompt:?}")
    };
    let second = f.connect(&host, PASSWORD_ONLY, None, &mut never).await;
    assert_eq!(second.ready().home().await.unwrap(), f.home(Role::Password));
    assert_eq!(second.kinds(), [&PromptKind::Password]);
    assert_eq!(second.asked(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_password_then_right() {
    let Some(mut f) = Fixture::new("wrong_password_then_right").await else { return };
    let host = f.host(Role::Password, None);
    let pw = f.at(Role::Password, "127.0.0.1");
    let mut errors = Vec::new();
    let mut user = |kind: &PromptKind, prompt: &str, error: Option<&str>| match kind {
        PromptKind::HostKey { .. } => text("yes"),
        PromptKind::Password => {
            errors.push(error.map(str::to_owned));
            text(if errors.len() == 1 { "not-the-password" } else { PASSWORD })
        }
        other => panic!("unexpected {other:?}: {prompt:?}"),
    };
    let out = f.connect(&host, PASSWORD_ONLY, None, &mut user).await;
    assert_eq!(out.ready().home().await.unwrap(), f.home(Role::Password));
    assert_eq!(errors, [None, Some(WRONG_PASSWORD.to_owned())]);
    assert_eq!(out.seen[2].asked, Some(Some(WRONG_PASSWORD.to_owned())));
    assert_eq!(out.seen[1].prompt, out.seen[2].prompt);
    assert!(out.stderr.iter().any(|l| l == "Permission denied, please try again."), "{:#?}", out.stderr);
    assert_eq!(f.store.get(&host.id, &pw).unwrap().expose_secret(), PASSWORD);

    // A stale remembered password: tried silently, then asked with the error, then replaced.
    f.store.set(host.id.clone(), pw.clone(), "stale".into());
    let mut asked = Vec::new();
    let mut user = |kind: &PromptKind, _: &str, error: Option<&str>| {
        asked.push((kind.clone(), error.map(str::to_owned)));
        text(PASSWORD)
    };
    let out = f.connect(&host, PASSWORD_ONLY, None, &mut user).await;
    out.ready();
    assert_eq!(out.kinds(), [&PromptKind::Password, &PromptKind::Password]);
    assert_eq!(out.seen[0].asked, None);
    assert_eq!(asked, [(PromptKind::Password, Some(WRONG_PASSWORD.to_owned()))]);
    assert_eq!(f.store.get(&host.id, &pw).unwrap().expose_secret(), PASSWORD);
}

#[tokio::test(flavor = "multi_thread")]
async fn password_rejected_until_ssh_gives_up() {
    let Some(mut f) = Fixture::new("password_rejected_until_ssh_gives_up").await else { return };
    let host = f.host(Role::Password, None);
    let pw = f.at(Role::Password, "127.0.0.1");
    let mut user = |kind: &PromptKind, _: &str, _: Option<&str>| match kind {
        PromptKind::HostKey { .. } => text("yes"),
        _ => text("never-right"),
    };
    let out = f.connect(&host, PASSWORD_ONLY, None, &mut user).await;
    assert!(matches!(out.sftp, Err(Error::Connect(_))));
    // NumberOfPasswordPrompts 3.
    assert_eq!(out.kinds()[1..], [&PromptKind::Password; 3]);
    // With PAM, pam_unix answers the third failure with PAM_MAXTRIES and sshd then offers no
    // method at all ("Authentications that can continue: "), hence the empty list. Without PAM
    // the line reads "… Permission denied (password).". The native backend uses the platform's
    // PAM stack (macOS: pam_opendirectory), which may not answer PAM_MAXTRIES: either form.
    let denied = |methods: &str| format!("{pw}: Permission denied ({methods}).");
    let expected = match f.sshd.backend() {
        Backend::Docker => vec![denied("")],
        Backend::Native => vec![denied(""), denied("password")],
    };
    assert!(out.stderr.iter().any(|l| expected.contains(l)), "{expected:?} in {:#?}", out.stderr);
    assert!(f.store.get(&host.id, &pw).is_none(), "the rejected password is forgotten");
}

/// Gap 1: the jump host's ssh inherits the askpass token, so both passwords arrive for the same
/// host. Here the jump is the fixture itself (`pw@localhost:<port>`) and the target is the same
/// sshd as seen from the server (`127.0.0.1:<port on the server>`: 22 in the container), so the
/// identities differ.
#[tokio::test(flavor = "multi_thread")]
async fn proxy_jump_passwords_are_kept_apart() {
    let Some(mut f) = Fixture::new("proxy_jump_passwords_are_kept_apart").await else { return };
    let mut host = f.host(Role::Password, None);
    let (jump, target) = (f.at(Role::Password, "localhost"), f.at(Role::Password, "127.0.0.1"));
    host.port = Some(f.sshd.port_on_server());
    host.proxy_jump = Some(format!("{jump}:{}", f.port()));
    let mut user = |kind: &PromptKind, prompt: &str, error: Option<&str>| match kind {
        PromptKind::HostKey { .. } => text("yes"),
        PromptKind::Password => {
            assert_eq!(error, None, "{prompt:?} is not a retry");
            text(PASSWORD)
        }
        other => panic!("unexpected {other:?}: {prompt:?}"),
    };
    let first = f.connect(&host, PASSWORD_ONLY, None, &mut user).await;
    assert_eq!(first.ready().home().await.unwrap(), f.home(Role::Password));
    let passwords: Vec<_> = first.seen.iter().filter(|s| s.kind == PromptKind::Password).map(|s| s.prompt.as_str()).collect();
    assert_eq!(passwords, [format!("{jump}'s password: "), format!("{target}'s password: ")]);
    assert_eq!(first.seen.iter().filter(|s| host_key(&s.kind)).count(), 2);
    assert_eq!(f.store.get(&host.id, &jump).unwrap().expose_secret(), PASSWORD);
    assert_eq!(f.store.get(&host.id, &target).unwrap().expose_secret(), PASSWORD);

    let mut never = |kind: &PromptKind, prompt: &str, _: Option<&str>| -> PromptAnswer {
        panic!("the user was asked {kind:?}: {prompt:?}")
    };
    let second = f.connect(&host, PASSWORD_ONLY, None, &mut never).await;
    second.ready();
    assert_eq!(second.kinds(), [&PromptKind::Password, &PromptKind::Password]);
    assert_eq!(second.asked(), 0);
    f.store.clear(&host.id);
    assert!(f.store.is_empty());
}

/// Two methods: public key (encrypted, so a passphrase prompt) then password.
#[tokio::test(flavor = "multi_thread")]
async fn second_factor_after_public_key() {
    let Some(mut f) = Fixture::new("second_factor_after_public_key").await else { return };
    let key = f.encrypted_key();
    let host = f.host(Role::KeyThenPassword, Some(key.clone()));
    let keypw = f.at(Role::KeyThenPassword, "127.0.0.1");
    let config = "PubkeyAuthentication yes\nPreferredAuthentications publickey,password";
    let mut user = |kind: &PromptKind, prompt: &str, error: Option<&str>| {
        assert_eq!(error, None);
        match kind {
            PromptKind::HostKey { .. } => text("yes"),
            PromptKind::Passphrase { .. } => text(PASSPHRASE),
            PromptKind::Password => text(PASSWORD),
            other => panic!("unexpected {other:?}: {prompt:?}"),
        }
    };
    let out = f.connect(&host, config, None, &mut user).await;
    assert_eq!(out.ready().home().await.unwrap(), f.home(Role::KeyThenPassword));
    assert_eq!(out.kinds()[1..], [&PromptKind::Passphrase { key_path: Some(key.clone()) }, &PromptKind::Password]);
    assert_eq!(out.seen[1].prompt, format!("Enter passphrase for key '{key}': "));
    assert_eq!(out.seen[2].prompt, format!("{keypw}'s password: "));
    assert_eq!(password_identity(&out.seen[2].prompt), keypw);
    assert!(out.stderr.iter().any(|l| l.contains("Authenticated using \"publickey\" with partial success.")), "{:#?}", out.stderr);

    // The password alone is not enough: the server only offers publickey first.
    let mut user = |kind: &PromptKind, prompt: &str, _: Option<&str>| panic!("unexpected {kind:?}: {prompt:?}");
    let out = f.connect(&host, PASSWORD_ONLY, None, &mut user).await;
    assert!(matches!(out.sftp, Err(Error::Connect(_))));
    assert!(out.seen.is_empty());
    let denied = format!("{keypw}: Permission denied (publickey).");
    assert!(out.stderr.contains(&denied), "{:#?}", out.stderr);
}

const KBD_ONLY: &str = "PubkeyAuthentication no\nPreferredAuthentications keyboard-interactive";

/// What PAM asks user `kbd` (the password, then pam_google_authenticator), with ssh's prefix:
/// the fixture's password prompt (`(kbd@127.0.0.1) Password: `; Apple's OpenPAM says
/// `Password:`) and `(kbd@127.0.0.1) Verification code: `.
fn kbd_prompts(f: &Fixture) -> (String, String) {
    let kbd = f.at(Role::Kbd, "127.0.0.1");
    (f.sshd.password_prompt(Role::Kbd), format!("({kbd}) Verification code: "))
}

/// Keyboard-interactive through PAM with a real second factor: the account password, then a
/// TOTP code, each its own round trip. `(user@host) Password: ` is a password prompt
/// (`classify_prompt`), so `AuthAttempt` remembers it; the code is `Other` and never remembered.
#[tokio::test(flavor = "multi_thread")]
async fn keyboard_interactive_password_then_verification_code() {
    let Some(mut f) = Fixture::new("keyboard_interactive_password_then_verification_code").await else { return };
    if !f.sshd.supports(Feature::Totp, f.name) {
        return;
    }
    let host = f.host(Role::Kbd, None);
    let (password_prompt, code_prompt) = kbd_prompts(&f);
    let kbd = f.at(Role::Kbd, "127.0.0.1");
    let mut user = |kind: &PromptKind, prompt: &str, error: Option<&str>| {
        assert_eq!(error, None, "{prompt:?}");
        match kind {
            PromptKind::HostKey { .. } => text("yes"),
            PromptKind::Password => text(PASSWORD),
            PromptKind::Other => text(&totp_now()),
            other => panic!("unexpected {other:?}: {prompt:?}"),
        }
    };
    let first = f.connect(&host, KBD_ONLY, None, &mut user).await;
    assert_eq!(first.ready().home().await.unwrap(), f.home(Role::Kbd));
    assert!(host_key(first.kinds()[0]));
    assert_eq!(first.kinds()[1..], [&PromptKind::Password, &PromptKind::Other]);
    assert_eq!(first.seen[1].prompt, password_prompt);
    assert_eq!(first.seen[2].prompt, code_prompt);
    assert!(first.seen.iter().all(|s| s.hint.is_none()), "{:#?}", first.seen);
    assert_eq!(password_identity(&password_prompt), kbd);
    assert!(
        first.stderr.iter().any(|l| l.contains("using \"keyboard-interactive\"")),
        "{:#?}",
        first.stderr
    );
    assert_eq!(f.store.get(&host.id, &kbd).unwrap().expose_secret(), PASSWORD);

    // Reconnect: the password is answered from memory, the code is asked again.
    let mut asked = Vec::new();
    let mut user = |kind: &PromptKind, prompt: &str, error: Option<&str>| {
        assert_eq!((kind, error), (&PromptKind::Other, None), "{prompt:?}");
        asked.push(prompt.to_owned());
        text(&totp_now())
    };
    let second = f.connect(&host, KBD_ONLY, None, &mut user).await;
    assert_eq!(second.ready().home().await.unwrap(), f.home(Role::Kbd));
    assert_eq!(second.kinds(), [&PromptKind::Password, &PromptKind::Other]);
    assert_eq!(second.seen[0].asked, None, "password from memory");
    assert_eq!(asked, [code_prompt]);
    // The password is the only secret held: no code was stored.
    f.store.clear_password(&host.id, &kbd);
    assert!(f.store.is_empty());
}

/// A wrong code: PAM fails the whole round, so the server starts again with the password, and
/// ssh gives up after `NumberOfPasswordPrompts 3` rounds. `AuthAttempt` can't tell which answer
/// was wrong, so the retry says so ([`WRONG_SIGN_IN`]) rather than blaming the password.
#[tokio::test(flavor = "multi_thread")]
async fn keyboard_interactive_wrong_code_fails_cleanly() {
    let Some(mut f) = Fixture::new("keyboard_interactive_wrong_code_fails_cleanly").await else { return };
    if !f.sshd.supports(Feature::Totp, f.name) {
        return;
    }
    let host = f.host(Role::Kbd, None);
    let (password_prompt, code_prompt) = kbd_prompts(&f);
    let kbd = f.at(Role::Kbd, "127.0.0.1");
    let mut errors = Vec::new();
    let mut user = |kind: &PromptKind, prompt: &str, error: Option<&str>| match kind {
        PromptKind::HostKey { .. } => text("yes"),
        PromptKind::Password => {
            errors.push(error.map(str::to_owned));
            text(PASSWORD)
        }
        PromptKind::Other => {
            assert_eq!(error, None);
            text(&wrong_totp())
        }
        other => panic!("unexpected {other:?}: {prompt:?}"),
    };
    let mut out = f.connect(&host, KBD_ONLY, None, &mut user).await;
    assert!(matches!(out.sftp, Err(Error::Connect(_))), "{:?}", out.sftp.as_ref().err());
    assert_eq!(out.child.wait().await.unwrap().code(), Some(255));
    assert!(host_key(out.kinds()[0]));
    let round = [&PromptKind::Password, &PromptKind::Other];
    assert_eq!(out.kinds()[1..], [round, round, round].concat());
    let prompts: Vec<_> = out.seen[1..].iter().map(|s| s.prompt.as_str()).collect();
    assert_eq!(prompts, [password_prompt.as_str(), code_prompt.as_str()].repeat(3));
    assert_eq!(errors, [None, Some(WRONG_SIGN_IN.to_owned()), Some(WRONG_SIGN_IN.to_owned())]);
    let denied = format!("{kbd}: Permission denied (keyboard-interactive).");
    assert!(out.stderr.contains(&denied), "{:#?}", out.stderr);
    // The last password is forgotten when ssh gives up, and no code was ever stored.
    assert!(f.store.is_empty());
    println!("wrong code: {:?}", out.sftp.as_ref().err().unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_key_wrong_passphrase_then_right() {
    let Some(mut f) = Fixture::new("encrypted_key_wrong_passphrase_then_right").await else { return };
    let key = f.encrypted_key();
    let host = f.host(Role::EncryptedKey, Some(key.clone()));
    let mut errors = Vec::new();
    let mut user = |kind: &PromptKind, prompt: &str, error: Option<&str>| match kind {
        PromptKind::HostKey { .. } => text("yes"),
        PromptKind::Passphrase { .. } => {
            errors.push(error.map(str::to_owned));
            text(if errors.len() == 1 { "not-the-passphrase" } else { PASSPHRASE })
        }
        other => panic!("unexpected {other:?}: {prompt:?}"),
    };
    let out = f.connect(&host, PUBKEY_ONLY, None, &mut user).await;
    assert_eq!(out.ready().home().await.unwrap(), f.home(Role::EncryptedKey));
    let passphrase = PromptKind::Passphrase { key_path: Some(key.clone()) };
    assert_eq!(out.kinds()[1..], [&passphrase, &passphrase]);
    assert_eq!(errors, [None, Some(WRONG_PASSPHRASE.to_owned())]);
    assert_eq!(out.seen[1].prompt, format!("Enter passphrase for key '{key}': "));
    assert_eq!(out.seen[1].hint, None);
    assert_eq!(f.store.passphrase(&host.id, Some(&key)).unwrap().expose_secret(), PASSPHRASE);

    let mut never = |kind: &PromptKind, prompt: &str, _: Option<&str>| -> PromptAnswer {
        panic!("the user was asked {kind:?}: {prompt:?}")
    };
    let second = f.connect(&host, PUBKEY_ONLY, None, &mut never).await;
    second.ready();
    assert_eq!(second.kinds(), [&passphrase]);
    assert_eq!(second.asked(), 0);
}

/// A real `SSH_ASKPASS_PROMPT=confirm` question: `AddKeysToAgent ask` with a private agent.
///
/// Unix only: it needs a throwaway agent of its own (`ssh-agent -D -a <socket>`). Win32-OpenSSH's
/// ssh-agent is the per-machine `ssh-agent` service on the fixed pipe
/// `\\.\pipe\openssh-ssh-agent`, so on Windows the key would land in the user's real agent.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn add_key_to_agent_is_a_confirm_prompt() {
    use dromon_testkit::ENCRYPTED_KEY_COMMENT;

    let Some(mut f) = Fixture::new("add_key_to_agent_is_a_confirm_prompt").await else { return };
    let sock = f.tmp.path().join("agent.sock");
    let _agent = tokio::process::Command::new("ssh-agent")
        .arg("-D")
        .arg("-a")
        .arg(&sock)
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("ssh-agent");
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "ssh-agent didn't start");

    let key = f.encrypted_key();
    let host = f.host(Role::EncryptedKey, Some(key.clone()));
    let mut user = |kind: &PromptKind, prompt: &str, _: Option<&str>| match kind {
        PromptKind::HostKey { .. } | PromptKind::Confirm => text("yes"),
        PromptKind::Passphrase { .. } => text(PASSPHRASE),
        other => panic!("unexpected {other:?}: {prompt:?}"),
    };
    let config = format!("{PUBKEY_ONLY}\nAddKeysToAgent ask");
    let out = f.connect(&host, &config, Some(&sock), &mut user).await;
    out.ready();
    assert_eq!(out.kinds()[1..], [&PromptKind::Passphrase { key_path: Some(key.clone()) }, &PromptKind::Confirm]);
    let confirm = &out.seen[2];
    assert_eq!(confirm.hint.as_deref(), Some("confirm"));
    assert_eq!(confirm.prompt, format!("Add key {key} ({ENCRYPTED_KEY_COMMENT}) to agent?"));

    let listed = tokio::process::Command::new("ssh-add").arg("-l").env("SSH_AUTH_SOCK", &sock).output().await.unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.contains(ENCRYPTED_KEY_COMMENT), "{listed}");
}
