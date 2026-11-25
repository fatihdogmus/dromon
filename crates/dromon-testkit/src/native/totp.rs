//! The native sshd's TOTP second factor for [`Role::Kbd`](crate::Role::Kbd), as in the image
//! (`tests/docker/pam-sshd`): the account password, then `Verification code: ` from
//! `pam_google_authenticator` with the image's `~/.google_authenticator`.
//!
//! The platform's `/etc/pam.d/sshd` is left alone. The kbd user gets a PAM service of its own:
//! `/etc/pam.d/<name of the sshd folder in lower case>` (Linux-PAM lowercases service names;
//! the folder's `pam_service` names it) is a copy of the platform's sshd policy (macOS:
//! `pam_krb5`, `pam_opendirectory`, `pam_sacl`, …; Debian: `common-auth`, …) with
//! `auth required <module>` appended, which is what Homebrew's caveat for the formula and
//! Debian's docs tell an admin to add to `/etc/pam.d/sshd`. The `Match User` block of the kbd user
//! selects it with `PAMServiceName` (OpenSSH 9.8+; macOS 15 ships 9.8p1 or 9.9p2, Debian trixie
//! 10.0p1), so the other users keep the platform's `sshd` service. The watchdog deletes the
//! policy with the folder, and the next start deletes those of folders that are gone.
//!
//! The module: Homebrew's `google-authenticator-libpam` on macOS (CI installs the bottle), or
//! the distribution's `libpam-google-authenticator` on Linux; without one, or with an sshd that
//! lacks `PAMServiceName`, [`Feature::Totp`](crate::Feature::Totp) is off and its tests skip.
//!
//! macOS: Apple's `sshd-session` is a platform binary, so dyld refuses the Homebrew module
//! (AMFI logs "Library Validation failed: … mapping process is a platform binary, but mapped
//! file is not"); Apple's OpenPAM then clears library validation for the process
//! (`csops(CS_OPS_CLEAR_LV)`, allowed by `sshd-session`'s
//! `com.apple.private.security.clear-library-validation` entitlement) and loads it again.
//!
//! Once sshd runs, [`probe`] signs in as the kbd user with the password and a code (through an
//! askpass script in the key folder) and prints the prompts it saw, or, when that fails, the
//! policy, sshd's log and, on macOS, the code signatures and the unified log's PAM and library
//! validation lines, so a failing TOTP test has its explanation above it.

use std::process::Stdio;
use std::time::Duration;

use super::{Os, Sshd, root_sh, script};
use crate::{Account, Fixture, HOST, NULL_DEVICE, PASSWORD, Role, note, openssh, totp_now};

/// The kbd user's `~/.google_authenticator`, the image's (`TOTP_AUTH`, `WINDOW_SIZE 3`).
const SECRET_FILE: &str = include_str!("../../../../tests/docker/google_authenticator");
/// Where PAM looks for service policies (Linux-PAM and Apple's OpenPAM both).
const PAM_DIR: &str = "/etc/pam.d";
/// The first that exists is used: Homebrew on Apple silicon and Intel, then Debian/Ubuntu
/// (amd64, arm64), Fedora/RHEL, Arch.
const MODULE_CANDIDATES: [&str; 6] = [
    "/opt/homebrew/opt/google-authenticator-libpam/lib/security/pam_google_authenticator.so",
    "/usr/local/opt/google-authenticator-libpam/lib/security/pam_google_authenticator.so",
    "/usr/lib/x86_64-linux-gnu/security/pam_google_authenticator.so",
    "/usr/lib/aarch64-linux-gnu/security/pam_google_authenticator.so",
    "/usr/lib64/security/pam_google_authenticator.so",
    "/usr/lib/security/pam_google_authenticator.so",
];
/// macOS's built-in `sshd` policy (Apple's OpenPAM, `openpam_apple_chains.c`), for when
/// `/etc/pam.d/sshd` is absent; the same as the file on macOS 15.
const MACOS_SSHD_POLICY: &str = "\
auth       optional       pam_krb5.so use_kcminit
auth       optional       pam_ntlm.so try_first_pass
auth       optional       pam_mount.so try_first_pass
auth       required       pam_opendirectory.so try_first_pass
account    required       pam_nologin.so
account    required       pam_sacl.so sacl_service=ssh
account    required       pam_opendirectory.so
password   required       pam_opendirectory.so
session    required       pam_launchd.so
session    optional       pam_mount.so
";
/// How long the probe login may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// The kbd user's PAM service, ready for its `Match` block.
#[derive(Debug)]
pub(crate) struct Totp {
    /// The `PAMServiceName`, also the policy's file name in [`PAM_DIR`].
    pub(super) service: String,
    /// The module's path, as the policy names it.
    pub(super) module: String,
}

impl Totp {
    pub(super) fn policy(&self) -> String {
        format!("{PAM_DIR}/{}", self.service)
    }
}

pub(super) enum Setup {
    Ready(Totp),
    /// Why there is no TOTP here (no module, no `PAMServiceName`, no policy to extend).
    Unavailable(String),
}

/// `$1` macos/linux, `$2` the sshd folder, `$3` sshd, `$4` the kbd user's `user:group`, `$5` its
/// home, `$6` the macOS fallback policy, then the module candidates; the secret file on stdin.
/// Prints `ready <service> <module>` or `unavailable: <why>`.
const SETUP: &str = r##"
os=$1 dir=$2 sshd=$3 owner=$4 home=$5 builtin=$6
shift 6
module=
for m in "$@"; do
    if [ -f "$m" ]; then module=$m; break; fi
done
if [ -z "$module" ]; then
    echo "unavailable: no pam_google_authenticator.so at $*"
    exit 0
fi
# Linux-PAM lowercases the service name (pam_start), so the folder's mktemp suffix won't do as is.
service=$(printf '%s' "${dir##*/}" | tr '[:upper:]' '[:lower:]')
if ! out=$("$sshd" -t -f /dev/null -h "$dir/ssh_host_ed25519_key" -o UsePAM=yes -o "PAMServiceName=$service" 2>&1); then
    echo "unavailable: $sshd has no PAMServiceName (OpenSSH 9.8+): $(printf '%s' "$out" | tr '\n' ' ')"
    exit 0
fi
if [ -f /etc/pam.d/sshd ]; then
    base=$(cat /etc/pam.d/sshd)
elif [ "$os" = macos ]; then
    base=$builtin
else
    echo "unavailable: no /etc/pam.d/sshd to add the TOTP module to"
    exit 0
fi
policy=/etc/pam.d/$service
# For the watchdog and the next start's sweep (native.rs).
printf '%s\n' "$service" >"$dir/pam_service"
{
    echo "# Written by dromon-testkit (native backend), deleted with $dir: the platform's sshd"
    echo "# policy plus a TOTP code, for ${owner%%:*} only (PAMServiceName in its Match block)."
    printf '%s\n' "$base"
    echo "auth       required       $module"
} >"$policy"
chmod 644 "$policy"
secret=$home/.google_authenticator
rm -f "$secret"
(umask 077 && cat >"$secret")
chown "$owner" "$secret"
chmod 400 "$secret"
echo "ready $service $module"
"##;

/// `$1` macos/linux, `$2` the sshd folder, `$3` the policy, `$4` the module, `$5` the secret
/// file (listed, never printed).
const DIAGNOSE: &str = r#"
set +e
os=$1 dir=$2 policy=$3 module=$4 secret=$5
echo "--- $policy"
cat "$policy"
echo "--- ls -l $secret"
ls -l "$secret"
echo "--- ssh -V"
ssh -V 2>&1
echo "--- $dir/sshd.log, last 80 lines"
tail -n 80 "$dir/sshd.log"
if [ "$os" = macos ]; then
    echo "--- sw_vers"
    sw_vers
    real=$(realpath "$module" 2>/dev/null || echo "$module")
    echo "--- codesign -dv $real"
    codesign -dv "$real" 2>&1
    echo "--- otool -L $real"
    otool -L "$real" 2>&1
    for b in /usr/sbin/sshd /usr/libexec/sshd-session /usr/libexec/sshd-auth; do
        if [ -e "$b" ]; then
            echo "--- codesign -dv --entitlements - $b"
            codesign -dv --entitlements - "$b" 2>&1
        fi
    done
    echo "--- log show --last 3m: library validation, PAM, sshd-session (last 80 lines)"
    log show --last 3m --style compact \
        --predicate 'eventMessage CONTAINS "Library Validation" OR eventMessage CONTAINS[c] "pam_" OR process == "sshd-session"' 2>&1 |
        tail -n 80
fi
exit 0
"#;

/// Answers the probe's keyboard-interactive prompts and records each one.
const ASKPASS: &str = r#"#!/bin/sh
# dromon-testkit: the TOTP probe's answers for the native sshd's kbd user.
printf '%s\n' "$1" >>"$DROMON_TESTKIT_PROMPTS"
case $1 in
*erification*) printf '%s\n' "$DROMON_TESTKIT_CODE" ;;
*assword*) printf '%s\n' "$DROMON_TESTKIT_PASSWORD" ;;
*) exit 1 ;;
esac
"#;

/// Gives `kbd` (its account, before sshd starts) a TOTP second factor if this machine can.
/// `Err` only when writing it failed.
pub(super) fn setup(os: Os, as_root: bool, dir: &str, sshd: &str, kbd: &Account) -> Result<Setup, String> {
    let owner = format!("{}:{}", kbd.name, kbd.group);
    let mut args = vec![os.arg(), dir, sshd, owner.as_str(), kbd.home.as_str(), MACOS_SSHD_POLICY];
    args.extend(MODULE_CANDIDATES);
    let out = root_sh(as_root, "set up TOTP for the kbd user", &script(SETUP), &args, SECRET_FILE.as_bytes())?;
    let line = out.trim();
    if let Some(why) = line.strip_prefix("unavailable: ") {
        return Ok(Setup::Unavailable(why.to_owned()));
    }
    let (service, module) = line
        .strip_prefix("ready ")
        .and_then(|rest| rest.split_once(' '))
        .ok_or_else(|| format!("set up TOTP for the kbd user: unexpected output {out:?}"))?;
    Ok(Setup::Ready(Totp { service: service.to_owned(), module: module.to_owned() }))
}

/// Signs in as the kbd user with the password and a code; prints the prompts, or why it failed
/// with what explains it. Never fails the fixture: the TOTP tests fail on their own.
pub(super) async fn probe(fixture: &Fixture, sshd: &Sshd, totp: &Totp) {
    let user = fixture.user(Role::Kbd);
    match sign_in(fixture).await {
        Ok(prompts) if prompts.iter().any(|p| p.contains("erification code")) => {
            let expected = fixture.password_prompt(Role::Kbd);
            let drift = if prompts.first() == Some(&expected) {
                String::new()
            } else {
                format!("; NOT Fixture::password_prompt's {expected:?}")
            };
            note(format_args!(
                "dromon-testkit: TOTP for {user} through PAM service {} ({}): signed in with the password and a code; \
                 prompts {prompts:?}{drift}",
                totp.service, totp.module,
            ));
        }
        result => {
            let why = match result {
                Ok(prompts) => format!("signed in without being asked for a code; prompts {prompts:?}"),
                Err(e) => e,
            };
            let kbd = &fixture.accounts[Role::Kbd.index()];
            let secret = format!("{}/.google_authenticator", kbd.home);
            let policy = totp.policy();
            let args = [sshd.os.arg(), sshd.dir.as_str(), policy.as_str(), totp.module.as_str(), secret.as_str()];
            let details = root_sh(sshd.as_root, "TOTP diagnostics", &script(DIAGNOSE), &args, b"").unwrap_or_else(|e| e);
            note(format_args!(
                "dromon-testkit: TOTP probe for {user} through PAM service {} ({}) failed: {why}\n{}",
                totp.service,
                totp.module,
                details.trim_end(),
            ));
        }
    }
}

/// The keyboard-interactive login with the askpass script: the prompts it answered, or why the
/// login failed.
async fn sign_in(fixture: &Fixture) -> Result<Vec<String>, String> {
    let askpass = fixture.keys.join("totp-askpass");
    let prompts = fixture.keys.join("totp-prompts");
    std::fs::write(&askpass, ASKPASS).map_err(|e| format!("{}: {e}", askpass.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", askpass.display()))?;
    }
    let _ = std::fs::remove_file(&prompts);
    let mut args: Vec<String> = ["-T", "-x", "-F", NULL_DEVICE].map(String::from).into();
    for option in [
        "BatchMode=no",
        "StrictHostKeyChecking=no",
        &format!("UserKnownHostsFile={NULL_DEVICE}"),
        &format!("GlobalKnownHostsFile={NULL_DEVICE}"),
        "LogLevel=ERROR",
        "PubkeyAuthentication=no",
        "PreferredAuthentications=keyboard-interactive",
        "NumberOfPasswordPrompts=1",
        "ConnectTimeout=10",
    ] {
        args.extend(["-o".into(), option.into()]);
    }
    args.extend(["-p".into(), fixture.port.to_string(), fixture.destination(Role::Kbd), "true".into()]);
    let child = tokio::process::Command::new(openssh("ssh"))
        .args(&args)
        .env("SSH_ASKPASS", &askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DROMON_TESTKIT_PROMPTS", &prompts)
        .env("DROMON_TESTKIT_PASSWORD", PASSWORD)
        .env("DROMON_TESTKIT_CODE", totp_now())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(PROBE_TIMEOUT, child)
        .await
        .map_err(|_| format!("ssh to {HOST}:{} didn't finish within {PROBE_TIMEOUT:?}", fixture.port))?
        .map_err(|e| format!("spawn ssh: {e}"))?;
    let seen: Vec<String> = std::fs::read_to_string(&prompts).unwrap_or_default().lines().map(str::to_owned).collect();
    if !out.status.success() {
        return Err(format!("ssh {}: {}; prompts {seen:?}", out.status, String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(seen)
}
