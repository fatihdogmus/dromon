//! Quick connect (plan §3.8): what the user types into Host, Username, Port and More's alias,
//! identity file and jump host, checked the way the command builder and a `Host` block will use
//! it, into a [`HostSpec`] for a saved or recent host. Nothing the user typed reaches ssh
//! without passing these checks (and the command builder's own).

use dromon_core::{HostId, HostSource, HostSpec};

use crate::command::{CommandError, Percent, check_arg, config_value};

#[cfg(test)]
#[path = "quick_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "quick_props.rs"]
mod props;

/// The quick-connect popover's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum QuickField {
    Host,
    User,
    Port,
    Alias,
    IdentityFile,
    ProxyJump,
}

impl QuickField {
    /// The field's label in the popover.
    pub fn label(self) -> &'static str {
        match self {
            QuickField::Host => "Host",
            QuickField::User => "Username",
            QuickField::Port => "Port",
            QuickField::Alias => "Name",
            QuickField::IdentityFile => "Identity file",
            QuickField::ProxyJump => "Jump host",
        }
    }
}

/// What the fields hold, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuickInput {
    pub host: String,
    pub user: String,
    pub port: String,
    pub alias: String,
    pub identity_file: String,
    pub proxy_jump: String,
}

impl QuickInput {
    /// The fields of `spec`, for editing it.
    pub fn from_spec(spec: &HostSpec) -> QuickInput {
        let host = spec.hostname.clone().unwrap_or_else(|| spec.alias.clone());
        // A name that's just the hostname (made up for a host without one) stays empty.
        let alias = if spec.alias == generated_alias(&host) { String::new() } else { spec.alias.clone() };
        QuickInput {
            host,
            user: spec.user.clone().unwrap_or_default(),
            port: spec.port.map(|p| p.to_string()).unwrap_or_default(),
            alias,
            identity_file: spec.identity_file.clone().unwrap_or_default(),
            proxy_jump: spec.proxy_jump.clone().unwrap_or_default(),
        }
    }
}

/// An address split into its parts: `user@host:port`, `user@host`, `host:port`, `[v6]:port`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub user: Option<String>,
    pub host: String,
    /// As written: [`parse_port`] checks it.
    pub port: Option<String>,
}

/// Splits `text` (trimmed; an `ssh://` prefix and a trailing `/` are dropped) into user, host and
/// port, or `None` when there's nothing to split off: a bare host or address (an IPv6 address
/// with no brackets keeps its colons), or no host at all (`user@`).
pub fn split_address(text: &str) -> Option<Address> {
    let mut t = text.trim();
    let mut uri = false;
    if t.get(..6).is_some_and(|scheme| scheme.eq_ignore_ascii_case("ssh://")) {
        t = t[6..].trim_end_matches('/');
        uri = true;
    }
    // ssh splits at the last `@`: a user name may hold one (`me@corp@host`).
    let (user, rest) = match t.rsplit_once('@') {
        Some((u, r)) => ((!u.is_empty()).then(|| u.to_owned()), r),
        None => (None, t),
    };
    let (host, port, bracketed) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?.to_owned()),
        };
        (host, port, true)
    } else {
        match rest.matches(':').count() {
            1 => {
                let (h, p) = rest.split_once(':')?;
                (h, Some(p.to_owned()), false)
            }
            _ => (rest, None, false),
        }
    };
    if host.is_empty() || !(user.is_some() || port.is_some() || bracketed || uri) {
        return None;
    }
    Some(Address { user, host: host.to_owned(), port })
}

/// A port as typed: empty is none (ssh's default, 22); otherwise 1–65535.
pub fn parse_port(text: &str) -> Result<Option<u16>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Use a number from 1 to 65535".into());
    }
    match t.parse::<u32>() {
        Ok(p @ 1..=65535) => Ok(Some(p as u16)),
        _ => Err("Use a number from 1 to 65535".into()),
    }
}

/// Whether `c` may appear in a host's name (its alias): what a `Host` line holds literally and
/// ssh takes as a host argument. No spaces, quotes, `#`, `=`, `,`, `@`, `/`, `\` or patterns.
fn alias_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '%' | '+')
}

/// A name ssh and a `Host` line both take as it is, or why not.
pub fn check_alias(alias: &str) -> Result<(), String> {
    if alias.is_empty() {
        return Err("Enter a name".into());
    }
    if alias.starts_with('-') {
        return Err("Can't start with “-”".into());
    }
    if !alias.chars().all(alias_char) {
        return Err("Use letters, digits, dots, dashes and underscores".into());
    }
    Ok(())
}

/// The name of a host that wasn't given one: its hostname, with anything a name can't hold as
/// `-`. Stable (the same host always gets the same name) and never starts with `-`.
pub fn generated_alias(hostname: &str) -> String {
    let mapped: String = hostname.chars().map(|c| if alias_char(c) { c } else { '-' }).collect();
    let trimmed = mapped.trim_start_matches('-');
    if trimmed.is_empty() { "host".to_owned() } else { trimmed.to_owned() }
}

/// A recent host's id: `[user@]host:port` (IPv6 in brackets, the port 22 when none was given),
/// so the same server is one entry however it was typed.
pub fn recent_id(hostname: &str, user: Option<&str>, port: Option<u16>) -> HostId {
    let host = if hostname.contains(':') { format!("[{hostname}]") } else { hostname.to_owned() };
    let port = port.unwrap_or(22);
    HostId(match user {
        Some(u) => format!("{u}@{host}:{port}"),
        None => format!("{host}:{port}"),
    })
}

/// Why `e` refused a value, as a field's error.
fn reason(e: &CommandError) -> String {
    match e {
        CommandError::Empty(_) => "Can't be empty".into(),
        CommandError::LeadingDash(_) => "Can't start with “-”".into(),
        CommandError::ControlChar(_) => "Can't contain line breaks or control characters".into(),
        CommandError::Unquotable(_) => "Contains characters ssh can't read back reliably (\", \\\", ${)".into(),
    }
}

/// A one-word value (host, user, jump host): no leading `-`, no whitespace or control characters,
/// and something ssh reads back exactly.
fn check_word(keyword: &'static str, value: &str, percent: Percent) -> Result<(), String> {
    if value.starts_with('-') {
        return Err("Can't start with “-”".into());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("Can't contain spaces".into());
    }
    if value.chars().any(char::is_control) {
        return Err("Can't contain control characters".into());
    }
    config_value(keyword, value, percent).map(drop).map_err(|e| reason(&e))
}

/// An identity file's path: spaces are fine (ssh reads it quoted), a leading `-` and control
/// characters aren't.
fn check_path(value: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err("Can't contain control characters".into());
    }
    check_arg("IdentityFile", value).map_err(|e| reason(&e))?;
    config_value("IdentityFile", value, Percent::Escape).map(drop).map_err(|e| reason(&e))
}

/// The host the fields describe, as a host of `source` (`Saved`: its id is its name; `Recent`:
/// its id is [`recent_id`]), or every field's problem. An address typed whole into Host
/// (`user@host:port`) fills Username and Port when they're empty.
pub fn quick_host(input: &QuickInput, source: HostSource) -> Result<HostSpec, Vec<(QuickField, String)>> {
    let mut errors: Vec<(QuickField, String)> = Vec::new();
    let mut host = input.host.trim().to_owned();
    let mut user = input.user.trim().to_owned();
    let mut port_text = input.port.trim().to_owned();
    if let Some(a) = split_address(&host) {
        match a.user {
            Some(u) if user.is_empty() => user = u,
            Some(u) if u != user => errors.push((QuickField::User, format!("The host says “{u}”"))),
            _ => {}
        }
        match a.port {
            Some(p) if port_text.is_empty() => port_text = p,
            Some(p) if p != port_text => errors.push((QuickField::Port, format!("The host says {p}"))),
            _ => {}
        }
        host = a.host;
    }
    let mut check = |field: QuickField, r: Result<(), String>| {
        if let Err(e) = r {
            errors.push((field, e));
        }
    };
    if host.is_empty() {
        check(QuickField::Host, Err("Enter a host name or address".into()));
    } else if host.contains('@') {
        check(QuickField::Host, Err("Put the user name under Username".into()));
    } else {
        check(QuickField::Host, check_word("HostName", &host, Percent::Escape));
    }
    if !user.is_empty() {
        check(QuickField::User, check_word("User", &user, Percent::Reject));
    }
    let port = parse_port(&port_text).unwrap_or_else(|e| {
        check(QuickField::Port, Err(e));
        None
    });
    let alias = input.alias.trim();
    if !alias.is_empty() {
        check(QuickField::Alias, check_alias(alias));
    }
    let identity = input.identity_file.trim();
    if !identity.is_empty() {
        check(QuickField::IdentityFile, check_path(identity));
    }
    let jump = input.proxy_jump.trim();
    if !jump.is_empty() {
        check(QuickField::ProxyJump, check_word("ProxyJump", jump, Percent::Literal));
    }
    if !errors.is_empty() {
        errors.sort_by_key(|(f, _)| *f);
        errors.dedup_by_key(|(f, _)| *f);
        return Err(errors);
    }
    let alias = if alias.is_empty() { generated_alias(&host) } else { alias.to_owned() };
    let some = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    let user = some(&user);
    let id = match source {
        HostSource::Recent => recent_id(&host, user.as_deref(), port),
        HostSource::Saved | HostSource::SshConfig => HostId(alias.clone()),
    };
    Ok(HostSpec {
        id,
        source,
        alias,
        hostname: Some(host),
        user,
        port,
        identity_file: some(identity),
        proxy_jump: some(jump),
    })
}

/// Whether `a` and `b` reach the same account the same way (a recent host typed again).
pub fn same_target(a: &HostSpec, b: &HostSpec) -> bool {
    a.hostname == b.hostname
        && a.user == b.user
        && a.port.unwrap_or(22) == b.port.unwrap_or(22)
        && a.identity_file == b.identity_file
        && a.proxy_jump == b.proxy_jump
}
