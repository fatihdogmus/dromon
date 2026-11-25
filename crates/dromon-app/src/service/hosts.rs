//! Host discovery for the sidebar's SSH Config section (§4.3 "Host discovery"): the literal
//! aliases from the user's and the system's ssh config, then the settings `ssh -G` resolves
//! for each, a few at a time. Both steps run off the UI thread.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use dromon_core::HostSpec;
use dromon_ssh::{ConfigSources, RESOLVE_TIMEOUT, Resolved, discover_aliases, parse_ssh_g, resolve};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use super::Sink;
use crate::state::ServiceEvent;

/// How many `ssh -G` run at once.
const RESOLVE_AT_ONCE: usize = 4;

/// The aliases in the config files `src` names, as hosts with nothing resolved yet. Reads files:
/// call it off the UI thread (or in a headless run, where nothing waits for a frame).
pub fn aliases(src: &ConfigSources) -> Vec<HostSpec> {
    discover_aliases(src).aliases.iter().map(|a| HostSpec::from_config_alias(a)).collect()
}

/// `spec` with what `ssh -G` printed. Identity files stay unset: ssh lists its default key
/// paths whether or not they exist, and an ssh-config host passes only its alias anyway.
pub fn with_resolved(mut spec: HostSpec, r: Resolved) -> HostSpec {
    spec.hostname = r.hostname;
    spec.user = r.user;
    spec.port = r.port;
    spec.proxy_jump = r.proxy_jump;
    spec
}

/// Discovers the hosts, tells the UI and the manager (`found`), then resolves them and tells
/// both again if anything was learned. With `config_file` (tests), `ssh -G` reads that file
/// only, as the connections do.
pub async fn discover<S: Sink>(
    src: ConfigSources,
    ssh: PathBuf,
    config_file: Option<PathBuf>,
    sink: S,
    found: mpsc::UnboundedSender<Vec<HostSpec>>,
) {
    let specs = tokio::task::spawn_blocking(move || aliases(&src)).await.unwrap_or_default();
    let _ = found.send(specs.clone());
    sink.emit(ServiceEvent::Hosts(specs.clone()));
    if specs.is_empty() {
        return;
    }
    let resolved = resolve_all(&ssh, config_file.as_deref(), specs.clone()).await;
    if resolved != specs {
        let _ = found.send(resolved.clone());
        sink.emit(ServiceEvent::Hosts(resolved));
    }
}

/// `ssh [-F config_file] -G <alias>`. Without a config file this is `dromon_ssh::resolve`.
async fn resolve_with(ssh: &Path, config_file: Option<&Path>, alias: &str) -> Result<Resolved, String> {
    let Some(file) = config_file else {
        return resolve(ssh, alias).await.map_err(|e| e.to_string());
    };
    if alias.is_empty() || alias.starts_with('-') || alias.contains(['\n', '\r', '\0']) {
        return Err(format!("{alias:?} isn't a host alias ssh can take"));
    }
    let mut cmd = tokio::process::Command::new(ssh);
    cmd.arg("-F").arg(file).arg("-G").arg(alias).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(RESOLVE_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => Ok(parse_ssh_g(&String::from_utf8_lossy(&out.stdout))),
        Ok(Ok(out)) => Err(format!("ssh -G {alias} failed ({})", out.status)),
        Ok(Err(e)) => Err(format!("Couldn't run {}: {e}", ssh.display())),
        Err(_) => Err(format!("ssh -G {alias} took too long")),
    }
}

/// `ssh -G` for every host, [`RESOLVE_AT_ONCE`] at a time, keeping the order. A host ssh can't
/// resolve keeps what it had.
async fn resolve_all(ssh: &Path, config_file: Option<&Path>, specs: Vec<HostSpec>) -> Vec<HostSpec> {
    let gate = Arc::new(Semaphore::new(RESOLVE_AT_ONCE));
    let mut set = JoinSet::new();
    for (i, spec) in specs.iter().enumerate() {
        let (gate, ssh, alias) = (gate.clone(), ssh.to_path_buf(), spec.alias.clone());
        let config_file = config_file.map(Path::to_path_buf);
        set.spawn(async move {
            let _permit = gate.acquire_owned().await;
            (i, resolve_with(&ssh, config_file.as_deref(), &alias).await)
        });
    }
    let mut out = specs;
    while let Some(done) = set.join_next().await {
        if let Ok((i, Ok(r))) = done {
            out[i] = with_resolved(out[i].clone(), r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_settings_fill_the_display_fields() {
        let r = Resolved {
            hostname: Some("10.0.0.4".into()),
            user: Some("deploy".into()),
            port: Some(2222),
            proxy_jump: Some("bastion".into()),
            identity_files: vec!["~/.ssh/id_ed25519".into()],
        };
        let h = with_resolved(HostSpec::from_config_alias("prod"), r);
        assert_eq!(h.alias, "prod");
        assert_eq!((h.hostname.as_deref(), h.user.as_deref(), h.port), (Some("10.0.0.4"), Some("deploy"), Some(2222)));
        assert_eq!(h.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(h.identity_file, None);
    }

    #[test]
    fn aliases_come_from_the_config_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, "Host prod staging\n  User deploy\nHost *.internal !x\nHost pi\n").unwrap();
        let src = ConfigSources { user: Some(config), system: None, user_ssh_dir: dir.path().to_path_buf() };
        let names: Vec<String> = aliases(&src).into_iter().map(|h| h.alias).collect();
        assert_eq!(names, ["prod", "staging", "pi"]);
    }
}
