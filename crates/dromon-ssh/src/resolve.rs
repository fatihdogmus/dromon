//! Effective settings for an alias, from `ssh -G <alias>`, so we display whatever ssh would do.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use dromon_core::Error;

use crate::command::check_arg;

/// How long `ssh -G` may take. It doesn't connect, but `Match exec` can run commands.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

/// The settings we show for a host. `None` means ssh printed nothing for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub proxy_jump: Option<String>,
    /// Every `identityfile` line, in order, unexpanded (`~/.ssh/id_ed25519`).
    pub identity_files: Vec<String>,
}

/// Parses `ssh -G` output: one lowercase `key value` per line. Unknown keys are ignored;
/// `proxyjump none` (printed by some versions) means no jump host.
pub fn parse_ssh_g(output: &str) -> Resolved {
    let mut r = Resolved::default();
    for line in output.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once([' ', '\t']) else { continue };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key {
            "hostname" => r.hostname = Some(value.to_owned()),
            "user" => r.user = Some(value.to_owned()),
            "port" => r.port = value.parse().ok(),
            "proxyjump" => {
                r.proxy_jump = (!value.eq_ignore_ascii_case("none")).then(|| value.to_owned());
            }
            "identityfile" => r.identity_files.push(value.to_owned()),
            _ => {}
        }
    }
    r
}

/// Runs `ssh -G <alias>` and parses its output, giving up after [`RESOLVE_TIMEOUT`].
pub async fn resolve(ssh: &Path, alias: &str) -> Result<Resolved, Error> {
    resolve_with_timeout(ssh, alias, RESOLVE_TIMEOUT).await
}

/// [`resolve`] with an explicit timeout.
pub async fn resolve_with_timeout(ssh: &Path, alias: &str, timeout: Duration) -> Result<Resolved, Error> {
    check_arg("The host alias", alias)?;
    let mut cmd = tokio::process::Command::new(ssh);
    cmd.arg("-G")
        .arg(alias)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(crate::process::CREATE_NO_WINDOW);

    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => return Err(Error::Other(format!("ssh -G {alias} took too long"))),
        Ok(Err(e)) => return Err(Error::Io(format!("Couldn't run {}: {e}", ssh.display()))),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no details");
        return Err(Error::Other(format!("ssh -G {alias} failed: {reason}")));
    }
    Ok(parse_ssh_g(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
