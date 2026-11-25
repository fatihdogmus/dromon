use std::fmt;

use serde::{Deserialize, Serialize};

/// Identifies a host across the app: the alias for SSH-config and saved hosts, or
/// `user@host:port` for a quick connect. Stable across launches, so it keys persisted state
/// (last folder per host) and the in-memory password store.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HostId(pub String);

impl HostId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HostId({:?})", self.0)
    }
}

/// Where a host came from, which decides its sidebar section and what can be edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostSource {
    /// A literal `Host` alias in the user's ssh config (read-only for us).
    SshConfig,
    /// One of ours, from `hosts.conf`.
    Saved,
    /// A recent quick connect.
    Recent,
}

/// What we need to start ssh for a host. For `SshConfig` hosts only `alias` is passed to ssh and
/// the rest is display information from `ssh -G`; for saved and recent hosts the fields become
/// explicit `-o` options.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSpec {
    pub id: HostId,
    pub source: HostSource,
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

impl HostSpec {
    pub fn from_config_alias(alias: &str) -> Self {
        HostSpec {
            id: HostId(alias.to_owned()),
            source: HostSource::SshConfig,
            alias: alias.to_owned(),
            hostname: None,
            user: None,
            port: None,
            identity_file: None,
            proxy_jump: None,
        }
    }

    /// The name shown in the sidebar and in titles.
    pub fn display_name(&self) -> &str {
        &self.alias
    }
}
