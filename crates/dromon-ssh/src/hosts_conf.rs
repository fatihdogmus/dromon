//! Saved hosts and "Add to SSH config" (plan §3.8, §4.3 "Saved and recent hosts").
//!
//! Saved hosts live in `hosts.conf` in the app's config folder, in ssh_config syntax: one `Host`
//! block per host with `HostName`, `User`, `Port`, `IdentityFile` and `ProxyJump`, so adding one
//! to the user's ssh config is a straight copy of its block ([`host_block`]). Values are written
//! the way the command builder writes `-o` values (quoted or refused alike), so the file reads
//! back exactly ([`parse_hosts_conf`]).
//!
//! Writes are atomic: a temporary file in the same folder, then a rename over the target. On
//! Unix `hosts.conf` is the user's alone (0600, its folder 0700 when we make it); appending to
//! the ssh config keeps that file's permissions and line endings, after a backup copy
//! ([`append_block`]). A failure leaves the target as it was.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use dromon_core::{HostId, HostSource, HostSpec};

use crate::command::{CommandError, check_arg, config_value, host_options};
use crate::config::{is_literal_alias, split_args, split_keyword};
use crate::quick::check_alias;

#[cfg(test)]
#[path = "hosts_conf_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "hosts_conf_props.rs"]
mod props;

/// The saved hosts' file name, in the app's config folder.
pub const HOSTS_CONF: &str = "hosts.conf";

/// The first lines of `hosts.conf`.
const HEADER: &str =
    "# Dromon's saved hosts, in ssh_config syntax. Dromon rewrites this file: edit hosts in the app.\n";

/// The `Host` block for `spec`, lines ending in `\n`:
///
/// ```text
/// Host box
///     HostName 10.0.0.4
///     User deploy
///     Port 2222
///     IdentityFile ~/.ssh/id_box
///     ProxyJump bastion
/// ```
///
/// Only the settings `spec` has. A name or value ssh couldn't read back exactly is refused.
pub fn host_block(spec: &HostSpec) -> Result<String, CommandError> {
    check_arg("The host's name", &spec.alias)?;
    if check_alias(&spec.alias).is_err() {
        return Err(CommandError::Unquotable("The host's name"));
    }
    let mut out = format!("Host {}\n", spec.alias);
    for (keyword, value, percent) in host_options(spec) {
        if let Some(value) = value {
            out.push_str(&format!("    {keyword} {}\n", config_value(keyword, &value, percent)?));
        }
    }
    Ok(out)
}

/// `hosts.conf` for `specs`: a comment, then their blocks, a blank line apart.
pub fn render_hosts_conf(specs: &[HostSpec]) -> Result<String, CommandError> {
    let mut out = HEADER.to_owned();
    for spec in specs {
        out.push('\n');
        out.push_str(&host_block(spec)?);
    }
    Ok(out)
}

/// The saved hosts in a `hosts.conf`, in order. Blocks whose `Host` line isn't exactly one literal
/// name are skipped, as are other keywords; a name seen twice keeps its first block.
pub fn parse_hosts_conf(text: &str) -> Vec<HostSpec> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out: Vec<HostSpec> = Vec::new();
    // The block being read, if its Host line named one host.
    let mut current: Option<HostSpec> = None;
    let finish = |spec: Option<HostSpec>, out: &mut Vec<HostSpec>| {
        if let Some(s) = spec
            && !out.iter().any(|h| h.alias == s.alias)
        {
            out.push(s);
        }
    };
    for line in text.lines() {
        let Some((keyword, rest)) = split_keyword(line) else { continue };
        let args = split_args(rest);
        if keyword.eq_ignore_ascii_case("host") || keyword.eq_ignore_ascii_case("match") {
            finish(current.take(), &mut out);
            if keyword.eq_ignore_ascii_case("host")
                && let [alias] = &args[..]
                && is_literal_alias(alias)
                && check_alias(alias).is_ok()
            {
                current = Some(HostSpec {
                    id: HostId(alias.clone()),
                    source: HostSource::Saved,
                    ..HostSpec::from_config_alias(alias)
                });
            }
            continue;
        }
        let (Some(spec), [value]) = (current.as_mut(), &args[..]) else { continue };
        // `%%` is how a literal `%` is written in the keywords that expand `%` tokens.
        let unescaped = || value.replace("%%", "%");
        match keyword.to_ascii_lowercase().as_str() {
            "hostname" => spec.hostname = Some(unescaped()),
            "user" => spec.user = Some(value.clone()),
            "port" => spec.port = value.parse().ok().filter(|p| *p > 0),
            "identityfile" => spec.identity_file = Some(unescaped()),
            "proxyjump" => spec.proxy_jump = Some(value.clone()),
            _ => {}
        }
    }
    finish(current, &mut out);
    out
}

/// The saved hosts in `path`; none when it doesn't exist.
pub fn read_hosts_conf(path: &Path) -> io::Result<Vec<HostSpec>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(parse_hosts_conf(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(with_path(e, "read", path)),
    }
}

/// Replaces `path` with `specs` atomically (user-only on Unix), making its folder if needed.
pub fn write_hosts_conf(path: &Path, specs: &[HostSpec]) -> io::Result<()> {
    let text = render_hosts_conf(specs).map_err(io::Error::other)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        make_private_dir(dir)?;
    }
    write_atomic(path, text.as_bytes(), None).map_err(|e| with_path(e, "write", path))
}

/// What [`append_block`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendOutcome {
    /// The copy of the file as it was; `None` when the file didn't exist (it was created).
    pub backup: Option<PathBuf>,
}

/// `<target>.dromon-backup-<stamp>`, and from the second try on `-2`, `-3`, …
pub fn backup_name(target: &Path, stamp: &str, n: u32) -> PathBuf {
    let name = target.file_name().map_or_else(|| "config".into(), |n| n.to_string_lossy().into_owned());
    let suffix = if n <= 1 { String::new() } else { format!("-{n}") };
    target.with_file_name(format!("{name}.dromon-backup-{stamp}{suffix}"))
}

/// The backup [`append_block`] would make now: the first free [`backup_name`]; `None` when
/// `target` doesn't exist (it would be created).
pub fn planned_backup(target: &Path, stamp: &str) -> Option<PathBuf> {
    fs::symlink_metadata(target).ok()?;
    (1..1000).map(|n| backup_name(target, stamp, n)).find(|p| fs::symlink_metadata(p).is_err())
}

/// `YYYYMMDD-HHMM`, the backup's time stamp.
pub fn backup_stamp(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> String {
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}")
}

/// Appends `block` to the ssh config `target`: copies the file to a fresh backup
/// ([`backup_name`], never over an existing file), then writes the file plus a blank line and the
/// block, in the file's own line endings (CRLF if it has any), atomically and with its
/// permissions. A missing file (and folder) is created, user-only. A link (dotfile managers) is
/// followed: the file it points to is the one changed. On any error the file is unchanged.
pub fn append_block(target: &Path, block: &str, stamp: &str) -> io::Result<AppendOutcome> {
    let real = match fs::symlink_metadata(target) {
        Ok(m) if m.file_type().is_symlink() => fs::canonicalize(target).map_err(|e| with_path(e, "follow", target))?,
        _ => target.to_path_buf(),
    };
    let original = match fs::read(&real) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(with_path(e, "read", target)),
    };
    let perms = original.as_ref().and_then(|_| fs::metadata(&real).ok()).map(|m| m.permissions());
    let backup = match &original {
        Some(bytes) => Some(write_backup(target, stamp, bytes, perms.clone())?),
        None => {
            if let Some(dir) = real.parent().filter(|d| !d.as_os_str().is_empty()) {
                make_private_dir(dir)?;
            }
            None
        }
    };
    let out = appended(original.as_deref().unwrap_or_default(), block);
    write_atomic(&real, &out, perms).map_err(|e| with_path(e, "write", target))?;
    Ok(AppendOutcome { backup })
}

/// `original` with `block` after it: a line break if its last line has none, a blank line, then
/// the block's lines, all in `original`'s line endings.
pub fn appended(original: &[u8], block: &str) -> Vec<u8> {
    let eol: &[u8] = if original.windows(2).any(|w| w == b"\r\n") { b"\r\n" } else { b"\n" };
    let mut out = original.to_vec();
    if !out.is_empty() {
        if !out.ends_with(b"\n") {
            out.extend_from_slice(eol);
        }
        out.extend_from_slice(eol);
    }
    for line in block.lines() {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(eol);
    }
    out
}

/// Copies `bytes` to the first free backup name, with `perms`.
fn write_backup(target: &Path, stamp: &str, bytes: &[u8], perms: Option<fs::Permissions>) -> io::Result<PathBuf> {
    for n in 1..1000 {
        let path = backup_name(target, stamp, n);
        let mut file = match create_new(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(with_path(e, "back up to", &path)),
        };
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        let done = written.and_then(|()| perms.clone().map_or(Ok(()), |p| fs::set_permissions(&path, p)));
        if let Err(e) = done {
            let _ = fs::remove_file(&path);
            return Err(with_path(e, "back up to", &path));
        }
        return Ok(path);
    }
    Err(io::Error::other(format!("Couldn't find a free backup name next to {}", target.display())))
}

/// A new file that must not exist yet; user-only on Unix.
fn create_new(path: &Path) -> io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)
}

/// Makes `dir` (and its parents) if missing; a folder we make is user-only on Unix.
fn make_private_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(|e| with_path(e, "create", dir))
}

/// Writes `bytes` to a temporary file next to `target`, then renames it over `target`. `perms`
/// are the new file's (else user-only on Unix). The temporary file goes away on any error.
fn write_atomic(target: &Path, bytes: &[u8], perms: Option<fs::Permissions>) -> io::Result<()> {
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    let mut opened = None;
    for n in 0..1000 {
        let tmp = dir.join(format!(".{name}.dromon-tmp-{}-{n}", std::process::id()));
        match create_new(&tmp) {
            Ok(f) => {
                opened = Some((tmp, f));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    let (tmp, mut file) = opened.ok_or_else(|| io::Error::other("no free temporary file name"))?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let done = written
        .and_then(|()| perms.map_or(Ok(()), |p| fs::set_permissions(&tmp, p)))
        .and_then(|()| fs::rename(&tmp, target));
    if done.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    done
}

fn with_path(e: io::Error, what: &str, path: &Path) -> io::Error {
    io::Error::new(e.kind(), format!("Couldn't {what} {}: {e}", path.display()))
}

/// `path` as the user knows it: `~/.ssh/config` (or `%USERPROFILE%\.ssh\config` on Windows)
/// when it's under `home`, else in full.
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    let home = home.map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
    display_path_str(&path.to_string_lossy(), &home, cfg!(windows))
}

/// [`display_path`] on strings, for either platform's paths.
pub fn display_path_str(path: &str, home: &str, windows: bool) -> String {
    let (sep, name) = if windows { ('\\', "%USERPROFILE%") } else { ('/', "~") };
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() || path.len() < home.len() || !path.is_char_boundary(home.len()) {
        return path.to_owned();
    }
    let (head, rest) = path.split_at(home.len());
    let same = if windows { head.eq_ignore_ascii_case(home) } else { head == home };
    let at_boundary = rest.is_empty() || rest.starts_with(sep) || (windows && rest.starts_with('/'));
    if same && at_boundary { format!("{name}{rest}") } else { path.to_owned() }
}
