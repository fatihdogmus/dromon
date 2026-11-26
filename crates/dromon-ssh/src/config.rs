//! Host discovery: the literal `Host` aliases in the user's and the system's ssh config, following
//! `Include` the way ssh does. Effective settings come from `ssh -G` (see `resolve`), so this
//! parser only reads `Host` and `Include` lines.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// ssh's own limit (`READCONF_MAX_DEPTH`): the top-level file is depth 0, and files are read up
/// to and including this depth.
pub const MAX_INCLUDE_DEPTH: usize = 16;

/// The config files ssh reads, and the directory user-config `Include`s are relative to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigSources {
    /// `~/.ssh/config`, or `None` when the home folder is unknown.
    pub user: Option<PathBuf>,
    /// `/etc/ssh/ssh_config`. Its relative `Include`s resolve against its own folder.
    pub system: Option<PathBuf>,
    /// `~/.ssh`. Relative `Include`s in the user config resolve against it, and its parent is
    /// the home folder used for `~` expansion.
    pub user_ssh_dir: PathBuf,
}

impl ConfigSources {
    /// The files ssh reads on this platform.
    ///
    /// - macOS/Linux: `~/.ssh/config` and `/etc/ssh/ssh_config`
    /// - Windows: `%USERPROFILE%\.ssh\config` and `%ProgramData%\ssh\ssh_config`
    pub fn default_for_platform() -> Self {
        #[cfg(windows)]
        let (home, system) =
            (env_path("USERPROFILE"), env_path("ProgramData").map(|d| d.join("ssh").join("ssh_config")));
        #[cfg(not(windows))]
        let (home, system) = (env_path("HOME"), Some(PathBuf::from("/etc/ssh/ssh_config")));

        let user_ssh_dir = home.map(|h| h.join(".ssh"));
        ConfigSources {
            user: user_ssh_dir.as_ref().map(|d| d.join("config")),
            system,
            user_ssh_dir: user_ssh_dir.unwrap_or_default(),
        }
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// What discovery found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discovery {
    /// Literal aliases, de-duplicated, in first-seen order (user config before system config).
    pub aliases: Vec<String>,
    /// Every config file that was read successfully, in reading order, for the file watcher.
    /// Missing files are not listed, so a watcher should also watch their parent folders.
    pub files_read: Vec<PathBuf>,
}

/// File access for the parser, so it can be tested without touching the disk.
pub trait ConfigFs {
    /// The file's contents, or `None` if it can't be read (missing, a folder, not UTF-8, ...).
    fn read_to_string(&self, path: &Path) -> Option<String>;
    /// The paths matching a glob pattern, in any order (the parser sorts them).
    fn glob(&self, pattern: &str) -> Vec<PathBuf>;
}

/// The real file system.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealFs;

impl ConfigFs for RealFs {
    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn glob(&self, pattern: &str) -> Vec<PathBuf> {
        // Like glob(3): `*` doesn't cross `/` or match a leading dot.
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        match glob::glob_with(pattern, options) {
            Ok(paths) => paths.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// How `Include` paths in one config file are resolved.
#[derive(Clone, Copy)]
pub struct ParseContext<'a> {
    /// Relative `Include` paths resolve against this folder (`~/.ssh` for the user config,
    /// `/etc/ssh` for the system one), for nested includes too, as ssh does.
    pub include_dir: &'a Path,
    /// The home folder for `~` expansion. `None` for the system config, where ssh rejects `~`.
    pub home: Option<&'a Path>,
    pub fs: &'a dyn ConfigFs,
}

/// Discovers aliases from the real config files.
pub fn discover_aliases(src: &ConfigSources) -> Discovery {
    discover_aliases_with(src, &RealFs)
}

/// [`discover_aliases`] over any file access.
pub fn discover_aliases_with(src: &ConfigSources, fs: &dyn ConfigFs) -> Discovery {
    let mut walker = Walker::default();
    if let Some(user) = &src.user {
        let ctx = ParseContext {
            include_dir: &src.user_ssh_dir,
            home: src.user_ssh_dir.parent().filter(|p| !p.as_os_str().is_empty()),
            fs,
        };
        walker.file(&ctx, user, 0);
    }
    if let Some(system) = &src.system {
        let dir = system.parent().unwrap_or(Path::new("/etc/ssh"));
        let ctx = ParseContext { include_dir: dir, home: None, fs };
        walker.file(&ctx, system, 0);
    }
    walker.finish()
}

/// Parses one config file's text (at depth 0), following its `Include`s through `ctx.fs`.
/// `files_read` lists only the included files, since the caller already has `text`.
pub fn parse_host_aliases(text: &str, ctx: &ParseContext<'_>) -> Discovery {
    let mut walker = Walker::default();
    walker.text(ctx, text, 0);
    walker.finish()
}

#[derive(Default)]
struct Walker {
    aliases: Vec<String>,
    seen: HashSet<String>,
    visited: HashSet<PathBuf>,
    files_read: Vec<PathBuf>,
}

impl Walker {
    fn finish(self) -> Discovery {
        Discovery { aliases: self.aliases, files_read: self.files_read }
    }

    fn file(&mut self, ctx: &ParseContext<'_>, path: &Path, depth: usize) {
        // A file already read adds no new aliases, so reading each file once also makes include
        // cycles harmless.
        if depth > MAX_INCLUDE_DEPTH || !self.visited.insert(path.to_path_buf()) {
            return;
        }
        let Some(text) = ctx.fs.read_to_string(path) else { return };
        self.files_read.push(path.to_path_buf());
        self.text(ctx, &text, depth);
    }

    fn text(&mut self, ctx: &ParseContext<'_>, text: &str, depth: usize) {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for line in text.lines() {
            let Some((keyword, rest)) = split_keyword(line) else { continue };
            // `Match` lines need nothing: their criteria (`Match host foo`) are not aliases, and
            // a `Host` line always ends the block. Includes inside a `Match` or `Host` block are
            // followed, since the hosts they define exist under some condition.
            if keyword.eq_ignore_ascii_case("host") {
                for alias in split_args(rest) {
                    if is_literal_alias(&alias) && self.seen.insert(alias.clone()) {
                        self.aliases.push(alias);
                    }
                }
            } else if keyword.eq_ignore_ascii_case("include") {
                for arg in split_args(rest) {
                    let Some(pattern) = include_pattern(ctx, &arg) else { continue };
                    let mut matches = ctx.fs.glob(&pattern);
                    matches.sort();
                    for path in matches {
                        self.file(ctx, &path, depth + 1);
                    }
                }
            }
        }
    }
}

pub(crate) fn is_literal_alias(alias: &str) -> bool {
    !alias.is_empty() && !alias.contains(['*', '?', '!'])
}

/// Splits a line into its keyword and the rest, accepting `Keyword value`, `Keyword=value` and
/// `Keyword = value`. Returns `None` for blank lines and comments.
pub(crate) fn split_keyword(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start_matches([' ', '\t']);
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find([' ', '\t', '=']).unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    Some((keyword, rest.trim_matches([' ', '\t', '\x0c'])))
}

/// Splits arguments the way OpenSSH's `argv_split` does: whitespace-separated, `"` and `'`
/// quotes, `\` escaping a quote, a backslash or (outside quotes) a space, and a `#` at the start
/// of a token ending the line. A line with an unterminated quote yields nothing (ssh rejects it).
pub(crate) fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
        match chars.peek() {
            None | Some('#') => return args,
            Some(_) => {}
        }
        let mut arg = String::new();
        let mut quote: Option<char> = None;
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.peek().copied() {
                    Some(n @ ('\'' | '"' | '\\')) => {
                        chars.next();
                        arg.push(n);
                    }
                    Some(' ') if quote.is_none() => {
                        chars.next();
                        arg.push(' ');
                    }
                    _ => arg.push('\\'),
                },
                ' ' | '\t' if quote.is_none() => break,
                '"' | '\'' if quote.is_none() => quote = Some(c),
                c if quote == Some(c) => quote = None,
                c => arg.push(c),
            }
        }
        if quote.is_some() {
            return Vec::new();
        }
        args.push(arg);
    }
}

/// Turns one `Include` argument into an absolute glob pattern, or `None` if ssh would reject it.
fn include_pattern(ctx: &ParseContext<'_>, arg: &str) -> Option<String> {
    if arg.is_empty() {
        return None;
    }
    let escape = |p: &Path| p.to_str().map(glob::Pattern::escape);
    if let Some(rest) = arg.strip_prefix('~') {
        // Only `~` and `~/...`; `~user` is not supported. ssh rejects `~` in system configs.
        let home = escape(ctx.home?)?;
        return match rest {
            "" => Some(home),
            _ if rest.starts_with('/') || (cfg!(windows) && rest.starts_with('\\')) => {
                Some(format!("{}{rest}", home.trim_end_matches(['/', '\\'])))
            }
            _ => None,
        };
    }
    if is_absolute(arg) {
        return Some(arg.to_owned());
    }
    let dir = escape(ctx.include_dir)?;
    if dir.is_empty() {
        return None;
    }
    let sep = if cfg!(windows) { '\\' } else { '/' };
    Some(format!("{}{sep}{arg}", dir.trim_end_matches(['/', '\\'])))
}

fn is_absolute(p: &str) -> bool {
    if p.starts_with('/') {
        return true;
    }
    cfg!(windows) && Path::new(p).is_absolute()
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
