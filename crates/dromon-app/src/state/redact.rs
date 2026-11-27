//! Stand-ins for the names a bug report must never carry (`report.rs`, and a recording of steps,
//! `record.rs`). Hosts become `host-1`, `host-2` …, in the order the report first names them.
//! In the report's sentences a folder's path becomes `<path>`, and a file's `<dir>/file-3.txt`,
//! keeping only a short, plain extension. A recording needs names that still work as names (a
//! fixture's folders, a script's element keys): a folder is `dir-4`, a file `file-3.txt`, a path
//! `/dir-1/dir-2`, and a leading dot stays so hidden files stay hidden ([`Redactor::name`],
//! [`Redactor::remote_path`]). Files are numbered by their name, folders likewise: the same name
//! is the same stand-in wherever it is, in one [`Redactor`], which a report and its recording
//! share.
//!
//! A host's id is replaced whole, so a quick connect's `user@203.0.113.7:2222` leaves no user,
//! address or port behind. Errors are rendered from their variant with the stand-in in the
//! path's place ([`Redactor::error`]), never by searching their text; a variant whose text is
//! free-form (ssh's reason, an I/O message) is left out with a sentence saying why.

use std::collections::HashMap;

use dromon_core::{Error, HostId, RemotePath};

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;

/// What an error with a free-form message says in its place.
pub const WITHHELD: &str = "(its message isn't included: it can name hosts, users or files)";

/// The longest extension a stand-in keeps; a longer one could be part of a name.
const MAX_EXT: usize = 8;

/// One report's stand-ins.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    /// Host ids, numbered in the order first named: `host-<n>`.
    hosts: HashMap<String, usize>,
    /// Files' names, likewise: `file-<n>`.
    files: HashMap<String, usize>,
    /// Folders' names, likewise: `dir-<n>`.
    dirs: HashMap<String, usize>,
}

/// The 1-based number of `name` among `seen`, numbering it next when it's new (a recording names
/// whole folders, so this is a lookup, not a search).
fn number(seen: &mut HashMap<String, usize>, name: &str) -> usize {
    if let Some(n) = seen.get(name) {
        return *n;
    }
    let n = seen.len() + 1;
    seen.insert(name.to_owned(), n);
    n
}

/// A file name's extension worth keeping: after the last dot of a name that isn't only a dot
/// file (`.env` has none), ASCII letters and digits, at most [`MAX_EXT`] of them.
fn extension(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    let plain = !ext.is_empty() && ext.len() <= MAX_EXT && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    (!stem.is_empty() && plain).then_some(ext)
}

/// The error's variant, for a report's kind line.
pub fn error_kind(error: &Error) -> &'static str {
    match error {
        Error::PermissionDenied(_) => "PermissionDenied",
        Error::NotFound(_) => "NotFound",
        Error::AlreadyExists(_) => "AlreadyExists",
        Error::ConnectionLost => "ConnectionLost",
        Error::Connect(_) => "Connect",
        Error::Cancelled => "Cancelled",
        Error::Unsupported(_) => "Unsupported",
        Error::Io(_) => "Io",
        Error::Other(_) => "Other",
    }
}

impl Redactor {
    /// `host-N` for the host `id`.
    pub fn host(&mut self, id: &HostId) -> String {
        self.host_str(id.as_str())
    }

    fn host_str(&mut self, id: &str) -> String {
        format!("host-{}", number(&mut self.hosts, id))
    }

    /// `<dir>/file-N.ext` for a file's path; `<path>` for anything whose name has no extension
    /// to keep (a folder, mostly).
    pub fn path(&mut self, path: &RemotePath) -> String {
        let Some(name) = path.file_name() else { return "<path>".to_owned() };
        match extension(name) {
            Some(ext) => format!("<dir>/file-{}.{ext}", number(&mut self.files, name)),
            None => "<path>".to_owned(),
        }
    }

    /// The stand-in for an entry's `name`: `dir-N` for a folder (`dir`: a folder, or a link to
    /// one), `file-N.ext` for anything else, keeping a short, plain extension; with the leading
    /// dot of a hidden one (`.file-4`).
    pub fn name(&mut self, name: &str, dir: bool) -> String {
        let dot = if name.starts_with('.') { "." } else { "" };
        if dir {
            return format!("{dot}dir-{}", number(&mut self.dirs, name));
        }
        let n = number(&mut self.files, name);
        match extension(name) {
            Some(ext) => format!("{dot}file-{n}.{ext}"),
            None => format!("{dot}file-{n}"),
        }
    }

    /// A folder's path with a stand-in for each part: `/dir-1/dir-2`; the root stays `/`.
    pub fn remote_path(&mut self, path: &RemotePath) -> String {
        if path.is_root() {
            return "/".to_owned();
        }
        path.components().map(|part| format!("/{}", self.name(part, true))).collect()
    }

    /// The error's own message with stand-ins for what it names, or [`WITHHELD`] for a
    /// variant whose message is free-form. A path's variant is rendered on a marker path, which
    /// the stand-in then takes the place of: the message keeps `dromon_core`'s wording, and the
    /// real path never enters it.
    pub fn error(&mut self, error: &Error) -> String {
        const MARK: &str = "/@";
        let with = |variant: fn(RemotePath) -> Error, stand_in: String| {
            variant(RemotePath::new(MARK)).to_string().replace(MARK, &stand_in)
        };
        match error {
            Error::PermissionDenied(p) => with(Error::PermissionDenied, self.path(p)),
            Error::NotFound(p) => with(Error::NotFound, self.path(p)),
            Error::AlreadyExists(p) => with(Error::AlreadyExists, self.path(p)),
            Error::ConnectionLost | Error::Cancelled | Error::Unsupported(_) => error.to_string(),
            Error::Connect(_) | Error::Io(_) | Error::Other(_) => WITHHELD.to_owned(),
        }
    }

    /// `line`, a service event's trace line (`state::service::trace_line`), with a stand-in for
    /// each host it names (`host=<id>`). `known` are the host ids the app has: one of them is
    /// taken whole even if it holds a space; anything else after `host=` up to the next space.
    pub fn trace(&mut self, line: &str, known: &[&str]) -> String {
        const KEY: &str = "host=";
        let mut out = String::with_capacity(line.len());
        let mut rest = line;
        while let Some(at) = rest.find(KEY) {
            let starts_word = at == 0 || rest[..at].ends_with(' ');
            out.push_str(&rest[..at + KEY.len()]);
            rest = &rest[at + KEY.len()..];
            if !starts_word {
                continue;
            }
            let ends = |len: usize| rest[len..].is_empty() || rest[len..].starts_with(' ');
            let whole =
                known.iter().filter(|k| !k.is_empty() && rest.starts_with(**k) && ends(k.len())).map(|k| k.len());
            let len = whole.max().unwrap_or_else(|| rest.find(' ').unwrap_or(rest.len()));
            out.push_str(&self.host_str(&rest[..len]));
            rest = &rest[len..];
        }
        out.push_str(rest);
        out
    }
}
