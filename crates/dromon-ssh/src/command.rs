//! The exact `ssh` command line for an SFTP session (plan §4.3 "Process launch").

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

use dromon_core::{HostSource, HostSpec};

use crate::askpass::{ENDPOINT_ENV, TOKEN_ENV};

/// Options every session gets, in order, before any per-host options.
pub const BASE_ARGS: &[&str] = &[
    "-T",
    "-x",
    "-e",
    "none",
    "-o",
    "BatchMode=no",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=3",
    "-o",
    "ConnectTimeout=15",
    "-o",
    "ControlMaster=no",
];

/// `DISPLAY` value set when none is present, so pre-8.4 clients' `prefer` logic still uses askpass.
pub const FALLBACK_DISPLAY: &str = "dromon:0";

/// A value we refuse to put on ssh's command line.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    #[error("{0} is empty")]
    Empty(&'static str),
    #[error("{0} can't start with \"-\"")]
    LeadingDash(&'static str),
    #[error("{0} can't contain line breaks or NUL characters")]
    ControlChar(&'static str),
    /// A `"`, a backslash sequence, `${`, or (in `User`) `%`, which ssh versions read differently.
    #[error("{0} contains characters ssh can't read back reliably")]
    Unquotable(&'static str),
}

/// How a `-o` value's `%` is treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Percent {
    /// The keyword has expanded `%` tokens since before 8.4: write `%%`.
    Escape,
    /// Expansion depends on the ssh version (`User`: 10.x expands, older don't): refuse `%`.
    Reject,
    /// Never expanded.
    Literal,
}

impl From<CommandError> for dromon_core::Error {
    fn from(e: CommandError) -> Self {
        dromon_core::Error::Connect(e.to_string())
    }
}

/// Rejects values that could be read as an ssh option or break a config line.
pub(crate) fn check_arg(what: &'static str, value: &str) -> Result<(), CommandError> {
    if value.is_empty() {
        return Err(CommandError::Empty(what));
    }
    if value.starts_with('-') {
        return Err(CommandError::LeadingDash(what));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err(CommandError::ControlChar(what));
    }
    Ok(())
}

/// The askpass wiring for one connection: our own executable in helper mode, the IPC endpoint,
/// and the per-connection token.
#[derive(Clone, PartialEq, Eq)]
pub struct AskpassEnv {
    pub exe: PathBuf,
    pub endpoint: String,
    pub token: String,
}

impl fmt::Debug for AskpassEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AskpassEnv")
            .field("exe", &self.exe)
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl AskpassEnv {
    /// The environment for the ssh child, given this process's current `DISPLAY`.
    pub fn vars(&self) -> Vec<(OsString, OsString)> {
        self.vars_with_display(std::env::var_os("DISPLAY").as_deref())
    }

    fn vars_with_display(&self, display: Option<&OsStr>) -> Vec<(OsString, OsString)> {
        let mut env = vec![
            (OsString::from("SSH_ASKPASS"), self.exe.clone().into_os_string()),
            (OsString::from("SSH_ASKPASS_REQUIRE"), OsString::from("force")),
        ];
        if display.is_none_or(OsStr::is_empty) {
            env.push((OsString::from("DISPLAY"), OsString::from(FALLBACK_DISPLAY)));
        }
        env.push((OsString::from(ENDPOINT_ENV), OsString::from(&self.endpoint)));
        env.push((OsString::from(TOKEN_ENV), OsString::from(&self.token)));
        env
    }
}

/// A program, its arguments and environment changes, ready for `process::spawn`.
#[derive(Clone, PartialEq, Eq)]
pub struct SshCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Variables set on the child, on top of what it inherits.
    pub env: Vec<(OsString, OsString)>,
    /// Inherited variables the child must not see. Empty by default: the app passes everything
    /// through, `SSH_AUTH_SOCK` included. A name also listed in `env` is set to that value.
    pub env_remove: Vec<OsString>,
}

impl fmt::Debug for SshCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env: Vec<_> = self
            .env
            .iter()
            .map(|(k, v)| {
                let v = if k == TOKEN_ENV { OsStr::new("<redacted>") } else { v.as_os_str() };
                (k, v)
            })
            .collect();
        f.debug_struct("SshCommand")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &env)
            .field("env_remove", &self.env_remove)
            .finish()
    }
}

impl SshCommand {
    /// `ssh <base options> [host options] [-v] -s <alias> sftp`.
    ///
    /// SSH-config hosts pass only the alias, so the user's config decides everything. Saved and
    /// recent hosts add `-o HostName=… -o User=… -o Port=… [-o IdentityFile=…] [-o ProxyJump=…]`
    /// for the fields that are set, so the user's global defaults still apply to the rest.
    pub fn sftp(host: &HostSpec, askpass: &AskpassEnv, verbose: bool) -> Result<Self, CommandError> {
        Self::sftp_with(ssh_program(), host, askpass.vars(), verbose)
    }

    fn sftp_with(
        program: PathBuf,
        host: &HostSpec,
        env: Vec<(OsString, OsString)>,
        verbose: bool,
    ) -> Result<Self, CommandError> {
        check_arg("The host alias", &host.alias)?;
        let mut args: Vec<OsString> = BASE_ARGS.iter().map(OsString::from).collect();
        if matches!(host.source, HostSource::Saved | HostSource::Recent) {
            for (keyword, value, percent) in host_options(host) {
                if let Some(value) = value {
                    args.push("-o".into());
                    args.push(config_option(keyword, &value, percent)?.into());
                }
            }
        }
        if verbose {
            args.push("-v".into());
        }
        args.extend(["-s".into(), OsString::from(&host.alias), "sftp".into()]);
        Ok(SshCommand { program, args, env, env_remove: Vec::new() })
    }

    /// Reads `path` as the only ssh config file: inserts `-F <path>` right after [`BASE_ARGS`].
    /// ssh then skips `~/.ssh/config` and the system-wide `ssh_config` entirely; the explicit
    /// `-o` options still win over the file. Call it once. The path must not be empty, start
    /// with `-`, or contain line breaks or NUL.
    pub fn with_config_file(mut self, path: &Path) -> Result<Self, CommandError> {
        check_arg("The ssh config file", &path.as_os_str().to_string_lossy())?;
        let at = BASE_ARGS.len().min(self.args.len());
        self.args.splice(at..at, [OsString::from("-F"), path.as_os_str().to_owned()]);
        Ok(self)
    }

    /// Keeps the inherited variable `name` (e.g. `SSH_AUTH_SOCK`) away from the child. Opt-in:
    /// nothing is removed unless asked.
    pub fn without_env(mut self, name: impl Into<OsString>) -> Self {
        self.env_remove.push(name.into());
        self
    }
}

/// The settings a saved or recent host passes explicitly, in order, each with how its `%` is
/// treated: the `-o` options of [`SshCommand::sftp`] and the lines of a `Host` block
/// (`hosts_conf::host_block`).
pub(crate) fn host_options(host: &HostSpec) -> [(&'static str, Option<String>, Percent); 5] {
    [
        ("HostName", host.hostname.clone(), Percent::Escape),
        ("User", host.user.clone(), Percent::Reject),
        ("Port", host.port.map(|p| p.to_string()), Percent::Literal),
        ("IdentityFile", host.identity_file.clone(), Percent::Escape),
        ("ProxyJump", host.proxy_jump.clone(), Percent::Literal),
    ]
}

/// `Keyword=value`, escaped so ssh's config-line parser reads back exactly `value`.
fn config_option(keyword: &'static str, value: &str, percent: Percent) -> Result<String, CommandError> {
    Ok(format!("{keyword}={}", config_value(keyword, value, percent)?))
}

/// `value` as it goes after its keyword on a config line (`-o Keyword=value`, or a line of a
/// config file), escaped so ssh's config-line parser reads back exactly `value`.
///
/// OpenSSH 8.7+ splits the value like a shell (`"`, `'`, `\` escapes, `#` comments); 8.4–8.6
/// (Windows 11's inbox client) only knows double quotes and splits on `=`. Values with spaces,
/// tabs, `#`, `'` or `=` are double-quoted, which both parsers read the same. Values containing
/// `"` or a backslash sequence the newer parser would unescape are rejected, since no form reads
/// back the same in both. `%` follows [`Percent`]; `${` (environment expansion in newer
/// versions, with no escape) is rejected.
pub(crate) fn config_value(keyword: &'static str, value: &str, percent: Percent) -> Result<String, CommandError> {
    check_arg(keyword, value)?;
    let unquotable = value.contains('"')
        || value.contains("${")
        || (percent == Percent::Reject && value.contains('%'))
        || value.as_bytes().windows(2).any(|w| w[0] == b'\\' && matches!(w[1], b'\\' | b'\'' | b'"'));
    if unquotable {
        return Err(CommandError::Unquotable(keyword));
    }
    let value = if percent == Percent::Escape { value.replace('%', "%%") } else { value.to_owned() };
    if value.contains([' ', '\t', '#', '\'', '=']) {
        if value.ends_with('\\') {
            return Err(CommandError::Unquotable(keyword));
        }
        Ok(format!("\"{value}\""))
    } else {
        Ok(value)
    }
}

/// The ssh to run: Windows' inbox OpenSSH if present, else `ssh.exe` from `PATH`; elsewhere
/// `ssh` from `PATH`.
pub fn ssh_program() -> PathBuf {
    #[cfg(windows)]
    {
        let inbox = PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh.exe");
        if inbox.is_file() {
            return inbox;
        }
        PathBuf::from("ssh.exe")
    }
    #[cfg(not(windows))]
    PathBuf::from("ssh")
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
