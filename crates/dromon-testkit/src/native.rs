//! The native backend (`DROMON_FIXTURE_BACKEND=native`): the machine's own OpenSSH `sshd`, run
//! privately as root for this test process. For CI runners without Docker (GitHub's macOS
//! runners can't nest virtualisation); macOS (Apple's `/usr/sbin/sshd`, `/usr/libexec/sftp-server`)
//! and Linux.
//!
//! It changes the machine, so it is gated: it only starts when `CI` is set (GitHub sets it) or
//! with `DROMON_FIXTURE_NATIVE_OK=1`, and it needs root, either directly or through `sudo -n`
//! (passwordless). Otherwise [`fixture`](crate::fixture) prints a skip line (or panics with
//! `DROMON_REQUIRE_DOCKER=1`).
//!
//! 1. Client keypairs as for Docker (`crate::client_keys`).
//! 2. As root: a fresh `/var/tmp/dromon-testkit-sshd.XXXXXXXX` (755, root's) holding the
//!    ed25519 host key, `sshd_config`, `sshd.pid`, `sshd.log` and `authorized_keys/<user>`;
//!    folders of earlier processes that are gone are deleted first.
//! 3. As root: the users `dromon_key`, `dromon_pw`, `dromon_kbd`, `dromon_keypw`, `dromon_enc`
//!    (prefixed so they can't collide with real accounts) with homes under `/Users` (macOS,
//!    `dscl`) or `/home` (Linux, `useradd`). An existing user is reused and reset (shell, home,
//!    password). `pw`, `kbd` and `keypw` get [`PASSWORD`](crate::PASSWORD); `key` and `enc` none
//!    (Linux `*`; macOS a random one, since Open Directory wants one).
//! 4. `tests/docker/seed.sh <home>` as `dromon_key`, so the seed data is the image's.
//! 5. `sshd -t`, then `sshd -D -e -f <dir>/sshd_config` on 127.0.0.1 at a free port, with the
//!    platform's PAM (`UsePAM yes`) and the Match blocks of `tests/docker/sshd_config`. The
//!    platform PAM has no TOTP module: [`Feature::Totp`](crate::Feature::Totp) is Docker only.
//!
//! Cleanup: statics are never dropped, so sshd is started by a small root watchdog (`sh`) that
//! polls this process's pid once a second; when it is gone (normal exit, panic, `kill -9`), the
//! watchdog stops sshd, ends the fixture users' leftover sessions and deletes the folder. The
//! users and their homes stay (reused by the next run). `DROMON_FIXTURE_KEEP=1`: no watchdog.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::sync::OnceCell;

use crate::{
    Account, ENCRYPTED_KEY, Fixture, HOST, KEEP_ENV, KEY, NATIVE_OK_ENV, NULL_DEVICE, PASSWORD, Role, Server, State,
    client_keys, env_flag, note, probe_login,
};

/// Prefix of the fixture's account names.
const USER_PREFIX: &str = "dromon_";
const SEED: &str = include_str!("../../../tests/docker/seed.sh");
const SSHD_CANDIDATES: [&str; 3] = ["/usr/sbin/sshd", "/usr/local/sbin/sshd", "/usr/bin/sshd"];
/// macOS; Debian/Ubuntu; Fedora/RHEL; Arch.
const SFTP_SERVER_CANDIDATES: [&str; 4] =
    ["/usr/libexec/sftp-server", "/usr/lib/openssh/sftp-server", "/usr/libexec/openssh/sftp-server", "/usr/lib/ssh/sftp-server"];
/// How long sshd may take to say `Server listening on`.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Os {
    MacOs,
    Linux,
}

impl Os {
    fn detect() -> Result<Os, String> {
        if cfg!(target_os = "macos") {
            Ok(Os::MacOs)
        } else if cfg!(target_os = "linux") {
            Ok(Os::Linux)
        } else {
            Err(format!("the native sshd fixture runs on macOS and Linux only, not {}", std::env::consts::OS))
        }
    }

    fn arg(self) -> &'static str {
        match self {
            Os::MacOs => "macos",
            Os::Linux => "linux",
        }
    }

    fn home_root(self) -> &'static str {
        match self {
            Os::MacOs => "/Users",
            Os::Linux => "/home",
        }
    }
}

/// The running private sshd.
#[derive(Debug)]
pub(crate) struct Sshd {
    /// Root's folder with the host key, config, pid file and log.
    dir: String,
    /// Run commands directly (already root) instead of through `sudo -n`.
    as_root: bool,
    os: Os,
}

impl Sshd {
    pub(crate) fn exec(&self, command: &[&str]) -> Result<String, String> {
        let out = root_command(self.as_root, command)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("spawn {command:?}: {e}"))?;
        if !out.status.success() {
            return Err(format!("{command:?} as root: {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub(crate) fn host_public_key(&self) -> String {
        format!("{}/ssh_host_ed25519_key.pub", self.dir)
    }

    pub(crate) fn bsd_tools(&self) -> bool {
        self.os == Os::MacOs
    }
}

pub(crate) async fn start() -> State {
    let os = match Os::detect() {
        Ok(os) => os,
        Err(why) => return State::Unavailable(why),
    };
    let ci = std::env::var_os("CI").is_some_and(|v| !v.is_empty());
    if !ci && !env_flag(NATIVE_OK_ENV) {
        return State::Unavailable(format!(
            "the native sshd fixture creates system users (dromon_*) and runs sshd as root, so it only \
             starts on CI (CI set) or with {NATIVE_OK_ENV}=1, in a throwaway VM or container"
        ));
    }
    let as_root = match root_access() {
        Ok(as_root) => as_root,
        Err(why) => return State::Unavailable(why),
    };
    let Some(sshd) = SSHD_CANDIDATES.into_iter().find(|p| Path::new(p).is_file()) else {
        return State::Unavailable(format!("no sshd at {SSHD_CANDIDATES:?}"));
    };
    let Some(sftp_server) = SFTP_SERVER_CANDIDATES.into_iter().find(|p| Path::new(p).is_file()) else {
        return State::Unavailable(format!("no sftp-server at {SFTP_SERVER_CANDIDATES:?}"));
    };
    match launch(os, as_root, sshd, sftp_server).await {
        Ok(fixture) => State::Ready(fixture),
        Err(why) => State::Failed(why),
    }
}

/// `Ok(true)` when this process is root, `Ok(false)` when `sudo -n true` works.
fn root_access() -> Result<bool, String> {
    let uid = Command::new("id").arg("-u").output().map_err(|e| format!("spawn id: {e}"))?;
    if String::from_utf8_lossy(&uid.stdout).trim() == "0" {
        return Ok(true);
    }
    match Command::new("sudo").args(["-n", "true"]).stdin(Stdio::null()).output() {
        Ok(out) if out.status.success() => Ok(false),
        Ok(out) => Err(format!(
            "the native sshd fixture needs root, and `sudo -n true` failed ({}: {})",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(format!("the native sshd fixture needs root, and sudo can't run: {e}")),
    }
}

/// `argv` as root: directly, or through `sudo -n`, in `/` (the fixture users can't read the
/// test's working folder).
fn root_command(as_root: bool, argv: &[&str]) -> Command {
    let mut cmd = if as_root {
        let mut cmd = Command::new(argv[0]);
        cmd.args(&argv[1..]);
        cmd
    } else {
        let mut cmd = Command::new("sudo");
        cmd.args(["-n", "--"]).args(argv);
        cmd
    };
    cmd.current_dir("/");
    cmd
}

/// Runs `script` with `sh -c` as root, with `args` as `$1…` and `stdin` fed in; its stdout.
fn root_sh(as_root: bool, what: &str, script: &str, args: &[&str], stdin: &[u8]) -> Result<String, String> {
    let mut argv = vec!["sh", "-c", script, "sh"];
    argv.extend_from_slice(args);
    let mut child = root_command(as_root, &argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{what}: spawn: {e}"))?;
    let mut input = child.stdin.take().expect("piped stdin");
    input.write_all(stdin).map_err(|e| format!("{what}: write stdin: {e}"))?;
    drop(input);
    let out = child.wait_with_output().map_err(|e| format!("{what}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what}: {}: {}{}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Every script starts with this: `sudo` may keep the caller's PATH, which needn't have sbin.
const SCRIPT_HEAD: &str = "set -eu\nPATH=/usr/sbin:/usr/bin:/sbin:/bin:$PATH\nexport PATH\n";

/// `$1` this process's pid, `$2` keep (`1`/`0`). Sweeps folders of processes that are gone,
/// makes a fresh one with the host key and prints its path.
const PREPARE: &str = r#"
pid=$1 keep=$2
for d in /var/tmp/dromon-testkit-sshd.*; do
    if [ ! -d "$d" ] || [ -e "$d/kept" ]; then continue; fi
    owner=$(cat "$d/owner" 2>/dev/null || true)
    if [ -n "$owner" ] && kill -0 "$owner" 2>/dev/null; then continue; fi
    rm -rf "$d"
done
dir=$(mktemp -d /var/tmp/dromon-testkit-sshd.XXXXXXXX)
chmod 755 "$dir"
echo "$pid" >"$dir/owner"
if [ "$keep" = 1 ]; then : >"$dir/kept"; fi
ssh-keygen -q -t ed25519 -N '' -C dromon-testkit-native -f "$dir/ssh_host_ed25519_key"
mkdir -m 755 "$dir/authorized_keys"
echo "$dir"
"#;

/// `$1` macos/linux, `$2` the homes' parent, then pairs of user and password (`-`: none).
/// Creates or resets each user, its home and an empty `.profile` (Debian's skeleton has one,
/// and tests look for it); prints `<user> <primary group>` per user.
const USERS: &str = r#"
os=$1 root=$2
shift 2
while [ $# -ge 2 ]; do
    u=$1 pw=$2
    shift 2
    home=$root/$u
    case $os in
    macos)
        if ! dscl . -read "/Users/$u" UniqueID >/dev/null 2>&1; then
            uid=$(dscl . -list /Users UniqueID | awk '$2 >= 600 && $2 < 1000 { print $2 }' | sort -n | tail -n 1)
            uid=$((${uid:-600} + 1))
            dscl . -create "/Users/$u"
            dscl . -create "/Users/$u" UniqueID "$uid"
            dscl . -create "/Users/$u" PrimaryGroupID 20
        fi
        dscl . -create "/Users/$u" UserShell /bin/sh
        dscl . -create "/Users/$u" RealName "Dromon test user"
        dscl . -create "/Users/$u" NFSHomeDirectory "$home"
        dscl . -create "/Users/$u" IsHidden 1
        if [ "$pw" = - ]; then pw=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n'); fi
        dscl . -passwd "/Users/$u" "$pw"
        # Remote Login limited to some users: sshd's PAM (pam_sacl) checks this group.
        if dscl . -read /Groups/com.apple.access_ssh >/dev/null 2>&1; then
            dseditgroup -o edit -a "$u" -t user com.apple.access_ssh
        fi
        dscacheutil -flushcache 2>/dev/null || true
        ;;
    linux)
        if id -u "$u" >/dev/null 2>&1; then
            usermod -d "$home" -s /bin/sh "$u"
        else
            useradd -m -d "$home" -s /bin/sh "$u"
        fi
        # useradd leaves "!" (locked), which sshd refuses even for public keys; "*" only
        # disables passwords.
        if [ "$pw" = - ]; then usermod -p '*' "$u"; else printf '%s:%s\n' "$u" "$pw" | chpasswd; fi
        ;;
    esac
    group=$(id -gn "$u")
    mkdir -p "$home"
    chown "$u:$group" "$home"
    if [ ! -e "$home/.profile" ]; then
        : >"$home/.profile"
        chown "$u:$group" "$home/.profile"
    fi
    echo "$u $group"
done
"#;

/// `$1` user, `$2` its home: runs the seed script on stdin as that user.
const SEED_AS: &str = r#"
u=$1 home=$2
if command -v sudo >/dev/null 2>&1; then
    exec sudo -n -u "$u" sh -s -- "$home"
fi
exec su "$u" -c "sh -s -- '$home'"
"#;

/// `$1` the folder, `$2` sshd, `$3` the config, then pairs of user and authorized key. Writes
/// them and checks the config with `sshd -t`.
const CONFIG: &str = r#"
dir=$1 sshd=$2 config=$3
shift 3
printf '%s' "$config" >"$dir/sshd_config"
chmod 644 "$dir/sshd_config"
while [ $# -ge 2 ]; do
    printf '%s\n' "$2" >"$dir/authorized_keys/$1"
    chmod 644 "$dir/authorized_keys/$1"
    shift 2
done
# Debian's sshd wants its privilege separation folder.
if [ "$(uname -s)" = Linux ] && [ ! -d /run/sshd ]; then mkdir -m 755 /run/sshd; fi
"$sshd" -t -f "$dir/sshd_config"
"#;

/// `$1` the test process's pid, `$2` the folder, `$3` sshd, `$4` keep, then the fixture users.
/// Starts sshd, then (unless kept) waits for the test process to go and cleans up after it.
/// INT and HUP are ignored so a Ctrl-C in the developer's terminal doesn't orphan sshd.
const WATCHDOG: &str = r#"
set +e
trap '' INT HUP
pid=$1 dir=$2 sshd=$3 keep=$4
shift 4
"$sshd" -D -e -f "$dir/sshd_config" </dev/null >>"$dir/sshd.log" 2>&1 &
sshd_pid=$!
if [ "$keep" = 1 ]; then exit 0; fi
while kill -0 "$pid" 2>/dev/null; do sleep 1; done
kill "$sshd_pid" 2>/dev/null
wait "$sshd_pid" 2>/dev/null
for u in "$@"; do pkill -u "$u" 2>/dev/null; done
rm -rf "$dir"
"#;

fn script(body: &str) -> String {
    format!("{SCRIPT_HEAD}{body}")
}

async fn launch(os: Os, as_root: bool, sshd_bin: &str, sftp_server: &str) -> Result<Fixture, String> {
    let started = Instant::now();
    let keep = env_flag(KEEP_ENV);
    let keys = client_keys(keep)?;
    let pid = std::process::id().to_string();
    let keep_arg = if keep { "1" } else { "0" };

    let dir = root_sh(as_root, "prepare the sshd folder", &script(PREPARE), &[&pid, keep_arg], b"")?.trim().to_owned();
    let sshd = Sshd { dir: dir.clone(), as_root, os };

    let names: Vec<String> = Role::ALL.iter().map(|r| format!("{USER_PREFIX}{}", r.base_name())).collect();
    let mut user_args = vec![os.arg(), os.home_root()];
    for (role, name) in Role::ALL.iter().zip(&names) {
        user_args.extend([name.as_str(), if role.has_password() { PASSWORD } else { "-" }]);
    }
    let made = root_sh(as_root, "create the fixture users", &script(USERS), &user_args, b"")?;
    let mut accounts = Vec::new();
    for name in &names {
        let group = made
            .lines()
            .find_map(|l| l.strip_prefix(name.as_str()).and_then(|rest| rest.strip_prefix(' ')))
            .ok_or_else(|| format!("create the fixture users: no group for {name} in {made:?}"))?;
        accounts.push(Account { name: name.clone(), home: format!("{}/{name}", os.home_root()), group: group.trim().to_owned() });
    }
    let key_account = &accounts[Role::Key.index()];
    root_sh(as_root, "seed.sh", &script(SEED_AS), &[&key_account.name, &key_account.home], SEED.as_bytes())?;

    let port = free_port()?;
    let pub_key = |file: &str| {
        let path = keys.join(format!("{file}.pub"));
        std::fs::read_to_string(&path).map(|s| s.trim().to_owned()).map_err(|e| format!("{}: {e}", path.display()))
    };
    let (plain, encrypted) = (pub_key(KEY)?, pub_key(ENCRYPTED_KEY)?);
    let name = |role: Role| names[role.index()].as_str();
    let config = sshd_config(port, &dir, sftp_server, &names);
    let config_args = [
        dir.as_str(),
        sshd_bin,
        config.as_str(),
        name(Role::Key),
        plain.as_str(),
        name(Role::KeyThenPassword),
        encrypted.as_str(),
        name(Role::EncryptedKey),
        encrypted.as_str(),
    ];
    root_sh(as_root, "write and check sshd_config", &script(CONFIG), &config_args, b"")?;

    let watchdog = script(WATCHDOG);
    let mut argv = vec!["sh", "-c", watchdog.as_str(), "sh", &pid, &dir, sshd_bin, keep_arg];
    argv.extend(names.iter().map(String::as_str));
    // Never waited for: it outlives this runtime, and the process too when kept.
    let child = root_command(as_root, &argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn the sshd watchdog: {e}"))?;
    drop(child);
    wait_listening(&sshd).await?;

    let fixture = Fixture { port, keys, accounts, server: Server::Native(sshd), big_file: OnceCell::new() };
    probe_login(&fixture).await?;
    note(format_args!(
        "dromon-testkit: native sshd fixture on {HOST}:{port}, {sshd_bin} with {sftp_server}, folder {dir}, users {}*, \
         ready after {:.1} s",
        USER_PREFIX,
        started.elapsed().as_secs_f64(),
    ));
    if keep {
        note(format_args!(
            "dromon-testkit: {KEEP_ENV}=1: sshd keeps running; log in with \
             `ssh -F {NULL_DEVICE} -i {} -p {port} {}@{HOST}`; clean up with \
             `sudo sh -c 'kill $(cat {dir}/sshd.pid); rm -rf {dir}'; rm -r {}`",
            fixture.key().display(),
            fixture.user(Role::Key),
            fixture.keys.display(),
        ));
    }
    Ok(fixture)
}

/// A port nothing listens on right now (the OS picks it; sshd binds it a moment later).
fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind((HOST, 0)).map_err(|e| format!("pick a free port: {e}"))?;
    listener.local_addr().map(|a| a.port()).map_err(|e| format!("pick a free port: {e}"))
}

/// Waits for sshd's `Server listening on` in its log (read as root: the log is root's).
async fn wait_listening(sshd: &Sshd) -> Result<(), String> {
    let log = format!("{}/sshd.log", sshd.dir);
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let text = sshd.exec(&["cat", &log]).unwrap_or_default();
        if text.contains("Server listening on") {
            return Ok(());
        }
        if text.contains("Cannot bind any address") || text.lines().any(|l| l.contains("fatal")) {
            return Err(format!("sshd didn't start:\n{}", text.trim()));
        }
        if Instant::now() > deadline {
            return Err(format!("no `Server listening on` from sshd within {READY_TIMEOUT:?}; its log:\n{}", text.trim()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `tests/docker/sshd_config` for this machine: the same methods per user (Match blocks), with
/// paths into `dir`, the platform's sftp-server and the prefixed users.
/// `names` by [`Role::index`].
fn sshd_config(port: u16, dir: &str, sftp_server: &str, names: &[String]) -> String {
    let name = |role: Role| names[role.index()].as_str();
    format!(
        "# Generated by dromon-testkit (native backend); mirrors tests/docker/sshd_config.
ListenAddress {HOST}
Port {port}
HostKey {dir}/ssh_host_ed25519_key
PidFile {dir}/sshd.pid
UsePAM yes
PermitRootLogin no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
# Written by the testkit, not the users' own ~/.ssh.
AuthorizedKeysFile {dir}/authorized_keys/%u
# Those files are root's, but under /var/tmp (world-writable), which StrictModes refuses.
StrictModes no
AllowUsers {users}
MaxStartups 100:30:200
MaxSessions 50
PrintMotd no
Subsystem sftp {sftp_server}
LogLevel INFO

Match User {pw}
    PasswordAuthentication yes
    PubkeyAuthentication no
    AuthenticationMethods password

Match User {kbd}
    KbdInteractiveAuthentication yes
    PubkeyAuthentication no
    AuthenticationMethods keyboard-interactive

Match User {keypw}
    PasswordAuthentication yes
    AuthenticationMethods publickey,password
",
        users = names.join(" "),
        pw = name(Role::Password),
        kbd = name(Role::Kbd),
        keypw = name(Role::KeyThenPassword),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_the_docker_match_blocks_for_the_prefixed_users() {
        let names: Vec<String> = Role::ALL.iter().map(|r| format!("dromon_{}", r.base_name())).collect();
        let config = sshd_config(2222, "/var/tmp/d", "/usr/libexec/sftp-server", &names);
        assert!(config.contains("AllowUsers dromon_key dromon_pw dromon_kbd dromon_keypw dromon_enc\n"));
        assert!(config.contains("Match User dromon_pw\n    PasswordAuthentication yes\n    PubkeyAuthentication no\n    AuthenticationMethods password\n"));
        assert!(config.contains("Match User dromon_keypw\n    PasswordAuthentication yes\n    AuthenticationMethods publickey,password\n"));
        assert!(config.contains("AuthorizedKeysFile /var/tmp/d/authorized_keys/%u\n"));
        assert!(config.contains("Subsystem sftp /usr/libexec/sftp-server\n"));
        assert!(config.contains("ListenAddress 127.0.0.1\nPort 2222\n"));
    }
}
