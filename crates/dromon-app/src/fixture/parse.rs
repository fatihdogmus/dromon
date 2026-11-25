//! A fixture's JSON (read with `serde_json`) into [`Fixture`]. Every error names where in the
//! document it is: object keys join with `.`, list items read `entry 3` (counting from 1), and
//! a field inside an item follows a `:` — `servers.prod.tree./var: entry 3: unknown kind
//! "folder"`. Unknown fields are errors, so a typo never goes unnoticed.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use dromon_core::{Entry, EntryKind, HostId, HostSource, HostSpec, LinkTarget, RemotePath};
use dromon_transfer::{ConflictView, JobId, JobKind, JobView, Progress, Reveal, Side, TaskState};
use serde_json::{Map, Value};

use super::generate::{Generator, MAX_COUNT, Mtime, Pattern, Size};
use super::{Change, FIXTURE_JOBS, Fixture, Item, Node, Server, Step, TreeError, VERSION, default_mode, default_size};

/// A fixture that can't be used, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Where in the document (`servers.prod.home`); empty for the document itself.
    pub path: String,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() { f.write_str(&self.message) } else { write!(f, "{}: {}", self.path, self.message) }
    }
}

impl std::error::Error for ParseError {}

type R<T> = Result<T, ParseError>;

/// Where the parser is.
#[derive(Clone, Default)]
struct At {
    text: String,
    /// Just after a list item (`entry 3`), so a field follows with `: `.
    in_item: bool,
}

impl At {
    fn key(&self, k: &str) -> At {
        let text = match (self.text.is_empty(), self.in_item) {
            (true, _) => k.to_owned(),
            (false, true) => format!("{}: {k}", self.text),
            (false, false) => format!("{}.{k}", self.text),
        };
        At { text, in_item: false }
    }

    fn item(&self, noun: &str, i: usize) -> At {
        let text = if self.text.is_empty() { format!("{noun} {}", i + 1) } else { format!("{}: {noun} {}", self.text, i + 1) };
        At { text, in_item: true }
    }

    fn err<T>(&self, message: impl Into<String>) -> R<T> {
        Err(ParseError { path: self.text.clone(), message: message.into() })
    }
}

fn what(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

fn object<'a>(v: &'a Value, at: &At) -> R<&'a Map<String, Value>> {
    v.as_object().map_or_else(|| at.err(format!("expected an object, found {}", what(v))), Ok)
}

fn list<'a>(v: &'a Value, at: &At) -> R<&'a Vec<Value>> {
    v.as_array().map_or_else(|| at.err(format!("expected a list, found {}", what(v))), Ok)
}

fn string(v: &Value, at: &At) -> R<String> {
    v.as_str().map(str::to_owned).map_or_else(|| at.err(format!("expected a string, found {}", what(v))), Ok)
}

fn text(v: &Value, at: &At) -> R<String> {
    let s = string(v, at)?;
    if s.is_empty() { at.err("must not be empty") } else { Ok(s) }
}

fn uint(v: &Value, at: &At) -> R<u64> {
    v.as_u64().map_or_else(|| at.err(format!("expected a whole number of at least 0, found {v}")), Ok)
}

/// Fails on any field not in `allowed`.
fn known(m: &Map<String, Value>, allowed: &[&str], at: &At) -> R<()> {
    match m.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => at.err(format!("unknown field {k:?} (expected {})", allowed.join(", "))),
        None => Ok(()),
    }
}

fn required<'a>(m: &'a Map<String, Value>, k: &str, at: &At) -> R<&'a Value> {
    match m.get(k) {
        Some(v) => Ok(v),
        None => at.key(k).err("missing"),
    }
}

/// A field that's absent or `null` is `None`.
fn optional<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    m.get(k).filter(|v| !v.is_null())
}

fn absolute(s: &str, at: &At) -> R<RemotePath> {
    if s.starts_with('/') { Ok(RemotePath::new(s)) } else { at.err(format!("{s:?} is not an absolute path")) }
}

/// Seconds since the epoch, from a whole number or an RFC 3339 date.
fn time(v: &Value, at: &At) -> R<i64> {
    let bad = || at.err(format!("{v} is not an RFC 3339 date (2026-11-25T14:00:00Z) or epoch seconds"));
    match v {
        Value::Number(n) => n.as_i64().map_or_else(bad, Ok),
        Value::String(s) => rfc3339(s).map_or_else(bad, Ok),
        _ => bad(),
    }
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` (a space or `t` for the `T` too).
pub fn rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = b.get(from..from + len)?;
        part.iter().all(u8::is_ascii_digit).then(|| part.iter().fold(0i64, |n, d| n * 10 + i64::from(d - b'0')))
    };
    let sep = |at: usize, set: &[u8]| b.get(at).is_some_and(|c| set.contains(c));
    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt ") && sep(13, b":") && sep(16, b":")) {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 2)?, num(8, 2)?, num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&mo) || d < 1 || d > days_in[mo as usize - 1] || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    if sep(i, b".") {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        c @ (b'+' | b'-') if i + 6 == b.len() && sep(i + 3, b":") => {
            let (oh, om) = (num(i + 1, 2)?, num(i + 4, 2)?);
            if oh > 23 || om > 59 {
                return None;
            }
            let o = oh * 3600 + om * 60;
            if *c == b'+' { o } else { -o }
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Reads a fixture document (see `docs/fixtures.md`).
pub fn parse(src: &str) -> Result<Fixture, ParseError> {
    let doc: Value = serde_json::from_str(src).map_err(|e| ParseError { path: String::new(), message: format!("not valid JSON: {e}") })?;
    let root = At::default();
    let Some(top) = doc.as_object() else {
        return root.err(format!("a fixture is a JSON object, found {}", what(&doc)));
    };
    known(top, &["version", "description", "now", "hosts", "servers", "transfers"], &root)?;
    let at = root.key("version");
    match top.get("version") {
        None => return at.err(format!("missing (this Dromon reads version {VERSION})")),
        Some(v) if v.as_u64() == Some(VERSION) => {}
        Some(v) => return at.err(format!("{v} is not supported (this Dromon reads version {VERSION})")),
    }
    if let Some(v) = top.get("description") {
        string(v, &root.key("description"))?;
    }
    let now = optional(top, "now").map(|v| time(v, &root.key("now"))).transpose()?;
    let hosts = match optional(top, "hosts") {
        Some(v) => hosts(v, &root.key("hosts"))?,
        None => Vec::new(),
    };
    let mut servers = BTreeMap::new();
    if let Some(v) = optional(top, "servers") {
        let at = root.key("servers");
        for (alias, v) in object(v, &at)? {
            let at = at.key(alias);
            if !hosts.iter().any(|h| &h.alias == alias) {
                return at.err("no host has this alias (add it to hosts)");
            }
            servers.insert(alias.clone(), server(v, &at)?);
        }
    }
    let transfers = match optional(top, "transfers") {
        Some(v) => transfers(v, &root.key("transfers"), &hosts)?,
        None => Vec::new(),
    };
    Ok(Fixture { now, hosts, servers, transfers })
}

/// The `transfers` list: job snapshots, newest first (see `docs/fixtures.md`, "Transfers").
fn transfers(v: &Value, at: &At, hosts: &[HostSpec]) -> R<Vec<JobView>> {
    let items = list(v, at)?;
    let mut out = Vec::with_capacity(items.len());
    for (i, v) in items.iter().enumerate() {
        let at = at.item("job", i);
        let m = object(v, &at)?;
        known(
            m,
            &[
                "kind", "host", "title", "state", "found", "error", "done", "total", "files", "speed", "eta", "waiting", "conflict",
                "reveal", "errors", "skipped",
            ],
            &at,
        )?;
        let kind = match string(required(m, "kind", &at)?, &at.key("kind"))?.as_str() {
            "download" => JobKind::Download,
            "upload" => JobKind::Upload,
            other => return at.key("kind").err(format!("unknown kind {other:?} (expected download or upload)")),
        };
        let host = text(required(m, "host", &at)?, &at.key("host"))?;
        if !hosts.iter().any(|h| h.alias == host) {
            return at.key("host").err(format!("{host:?} is not a host (add it to hosts)"));
        }
        let title = text(required(m, "title", &at)?, &at.key("title"))?;
        let only = |k: &str, state: &str| -> R<()> {
            match m.get(k) {
                Some(_) => at.key(k).err(format!("only a {state} job has {k}")),
                None => Ok(()),
            }
        };
        let state = match string(required(m, "state", &at)?, &at.key("state"))?.as_str() {
            "preparing" => {
                only("error", "failed")?;
                let found = optional(m, "found").map(|v| uint(v, &at.key("found"))).transpose()?.unwrap_or(0);
                TaskState::Preparing { files_found: found }
            }
            "failed" => {
                only("found", "preparing")?;
                TaskState::Failed { error: text(required(m, "error", &at)?, &at.key("error"))? }
            }
            other => {
                only("found", "preparing")?;
                only("error", "failed")?;
                match other {
                    "queued" => TaskState::Queued,
                    "running" => TaskState::Running,
                    "paused" => TaskState::Paused,
                    "done" => TaskState::Done,
                    "cancelled" => TaskState::Cancelled,
                    _ => {
                        return at.key("state").err(format!(
                            "unknown state {other:?} (expected queued, preparing, running, paused, done, failed or cancelled)"
                        ));
                    }
                }
            }
        };
        let bytes = |k: &str| optional(m, k).map(|v| uint(v, &at.key(k))).transpose().map(Option::unwrap_or_default);
        let (done, total) = (bytes("done")?, bytes("total")?);
        if total > 0 && done > total {
            return at.key("done").err(format!("{done} is more than total ({total})"));
        }
        let (files_done, files_total) = match optional(m, "files") {
            None => (0, 0),
            Some(v) => {
                let at = at.key("files");
                let r = list(v, &at)?;
                let (Some(d), Some(t), 2) = (r.first().and_then(Value::as_u64), r.get(1).and_then(Value::as_u64), r.len()) else {
                    return at.err("files is [done, total], whole numbers");
                };
                if d > t {
                    return at.err(format!("[{d}, {t}]: done is above total"));
                }
                (d, t)
            }
        };
        let seconds = |k: &str| -> R<Option<f64>> {
            match optional(m, k) {
                None => Ok(None),
                Some(v) => match v.as_f64().filter(|x| x.is_finite() && *x >= 0.0) {
                    Some(x) => Ok(Some(x)),
                    None => at.key(k).err(format!("expected a number of at least 0, found {v}")),
                },
            }
        };
        let speed = seconds("speed")?;
        let eta = seconds("eta")?.map(Duration::from_secs_f64);
        let waiting = match optional(m, "waiting") {
            None => false,
            Some(v) => v.as_bool().map_or_else(|| at.key("waiting").err(format!("expected true or false, found {}", what(v))), Ok)?,
        };
        let conflict = optional(m, "conflict").map(|v| conflict(v, &at.key("conflict"))).transpose()?;
        let host = HostId(host);
        let reveal = match optional(m, "reveal") {
            None => None,
            Some(v) => {
                let at = at.key("reveal");
                let path = text(v, &at)?;
                Some(match kind {
                    JobKind::Download => Reveal::Local(PathBuf::from(path)),
                    _ => Reveal::Remote(host.clone(), absolute(&path, &at)?),
                })
            }
        };
        let lines = |k: &str| -> R<Vec<String>> {
            match optional(m, k) {
                None => Ok(Vec::new()),
                Some(v) => {
                    let at = at.key(k);
                    list(v, &at)?.iter().enumerate().map(|(i, v)| text(v, &at.item("line", i))).collect()
                }
            }
        };
        out.push(JobView {
            // Newest first, so the first gets the highest id; above any id a session starts.
            id: JobId(FIXTURE_JOBS + (items.len() - i) as u64),
            kind,
            host,
            title,
            state,
            progress: Progress { done, total },
            files_done,
            files_total,
            speed,
            eta,
            waiting_for_connection: waiting,
            conflict,
            reveal,
            errors: lines("errors")?,
            skipped: lines("skipped")?,
            folders: Vec::new(),
        });
    }
    Ok(out)
}

fn conflict(v: &Value, at: &At) -> R<ConflictView> {
    let m = object(v, at)?;
    known(m, &["name", "folder", "dir", "more", "existing", "incoming"], at)?;
    let name = text(required(m, "name", at)?, &at.key("name"))?;
    let folder = absolute(&text(required(m, "folder", at)?, &at.key("folder"))?, &at.key("folder"))?;
    let existing_is_dir = match optional(m, "dir") {
        None => false,
        Some(v) => v.as_bool().map_or_else(|| at.key("dir").err(format!("expected true or false, found {}", what(v))), Ok)?,
    };
    let more = optional(m, "more").map(|v| uint(v, &at.key("more"))).transpose()?.unwrap_or(0) as usize;
    let side = |k: &str| -> R<Side> {
        let Some(v) = optional(m, k) else { return Ok(Side::default()) };
        let at = at.key(k);
        let s = object(v, &at)?;
        known(s, &["size", "mtime"], &at)?;
        Ok(Side {
            size: optional(s, "size").map(|v| uint(v, &at.key("size"))).transpose()?,
            mtime: optional(s, "mtime").map(|v| time(v, &at.key("mtime"))).transpose()?,
        })
    };
    Ok(ConflictView { name, folder, existing_is_dir, existing: side("existing")?, incoming: side("incoming")?, more })
}

fn hosts(v: &Value, at: &At) -> R<Vec<HostSpec>> {
    let mut out: Vec<HostSpec> = Vec::new();
    for (i, v) in list(v, at)?.iter().enumerate() {
        let at = at.item("host", i);
        let m = object(v, &at)?;
        known(m, &["alias", "source", "user", "hostname", "port"], &at)?;
        let alias = text(required(m, "alias", &at)?, &at.key("alias"))?;
        if alias.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return at.key("alias").err(format!("{alias:?} has whitespace"));
        }
        if out.iter().any(|h| h.alias == alias) {
            return at.key("alias").err(format!("{alias:?} is already a host"));
        }
        let source = match string(required(m, "source", &at)?, &at.key("source"))?.as_str() {
            "ssh-config" => HostSource::SshConfig,
            "saved" => HostSource::Saved,
            "recent" => HostSource::Recent,
            other => return at.key("source").err(format!("unknown source {other:?} (expected ssh-config, saved or recent)")),
        };
        let opt_text = |k: &str| optional(m, k).map(|v| text(v, &at.key(k))).transpose();
        let port = match optional(m, "port") {
            Some(v) => match v.as_u64().and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0) {
                Some(p) => Some(p),
                None => return at.key("port").err(format!("{v} is not a port (1–65535)")),
            },
            None => None,
        };
        out.push(HostSpec {
            id: HostId(alias.clone()),
            source,
            alias,
            hostname: opt_text("hostname")?,
            user: opt_text("user")?,
            port,
            identity_file: None,
            proxy_jump: None,
        });
    }
    Ok(out)
}

fn server(v: &Value, at: &At) -> R<Server> {
    let m = object(v, at)?;
    known(m, &["home", "auth", "tree", "fail", "changes"], at)?;
    let home = absolute(&text(required(m, "home", at)?, &at.key("home"))?, &at.key("home"))?;
    let mut auth = Vec::new();
    if let Some(v) = optional(m, "auth") {
        let at = at.key("auth");
        for (i, v) in list(v, &at)?.iter().enumerate() {
            auth.push(step(v, &at.item("step", i))?);
        }
    }
    let mut tree = BTreeMap::new();
    if let Some(v) = optional(m, "tree") {
        let at = at.key("tree");
        for (key, v) in object(v, &at)? {
            let at = at.key(key);
            let path = absolute(key, &at)?;
            if tree.contains_key(&path) {
                return at.err(format!("lists {path} again (paths are normalized)"));
            }
            tree.insert(path, node(v, &at)?);
        }
    }
    let fail = optional(m, "fail").map(|v| text(v, &at.key("fail"))).transpose()?;
    let mut changes = Vec::new();
    if let Some(v) = optional(m, "changes") {
        let at = at.key("changes");
        for (i, v) in list(v, &at)?.iter().enumerate() {
            let at = at.item("step", i);
            // A step is one change, or a list of changes made together.
            let step = match v.as_array() {
                Some(all) => all.iter().enumerate().map(|(j, v)| change(v, &at.item("change", j))).collect::<R<Vec<_>>>()?,
                None => vec![change(v, &at)?],
            };
            changes.push(step);
        }
    }
    Ok(Server { home, auth, tree, fail, changes })
}

/// `{"add": PATH, "kind": …, …entry fields}`, `{"remove": PATH}` or
/// `{"modify": PATH, "size"?: …, "mtime"?: …}`.
fn change(v: &Value, at: &At) -> R<Change> {
    let m = object(v, at)?;
    let path_at = |k: &str| -> R<RemotePath> {
        let at = at.key(k);
        let path = absolute(&text(&m[k], &at)?, &at)?;
        if path.is_root() { at.err("the root can't change") } else { Ok(path) }
    };
    if m.contains_key("add") {
        let path = path_at("add")?;
        let mut fields = m.clone();
        fields.remove("add");
        if fields.contains_key("name") || fields.contains_key("generate") {
            return at.err("an added entry takes its name from its path, and can't be a generator");
        }
        fields.insert("name".into(), Value::String(path.file_name().unwrap_or_default().to_owned()));
        let Item::Entry(entry) = item(&Value::Object(fields), at)? else { return at.err("expected an entry") };
        return Ok(Change::Add { path, entry });
    }
    if m.contains_key("remove") {
        known(m, &["remove"], at)?;
        return Ok(Change::Remove { path: path_at("remove")? });
    }
    if m.contains_key("modify") {
        known(m, &["modify", "size", "mtime"], at)?;
        let size = optional(m, "size").map(|v| uint(v, &at.key("size"))).transpose()?;
        let mtime = optional(m, "mtime").map(|v| time(v, &at.key("mtime"))).transpose()?;
        if size.is_none() && mtime.is_none() {
            return at.err("a modify changes a size, an mtime or both");
        }
        return Ok(Change::Modify { path: path_at("modify")?, size, mtime });
    }
    at.err("a change is {\"add\": path, …}, {\"remove\": path} or {\"modify\": path, …}")
}

fn step(v: &Value, at: &At) -> R<Step> {
    const KINDS: &str = "host-key, password, passphrase, code, confirm or notice";
    let m = object(v, at)?;
    let mut keys = m.keys();
    let (Some(kind), None) = (keys.next(), keys.next()) else {
        return at.err(format!("a step is an object with one field: {KINDS}"));
    };
    let at = at.key(kind);
    let fields = object(&m[kind], &at)?;
    let field = |k: &str| -> R<String> { text(required(fields, k, &at)?, &at.key(k)) };
    let opt = |k: &str| -> R<Option<String>> { optional(fields, k).map(|v| text(v, &at.key(k))).transpose() };
    // `accept` may be empty (an empty passphrase is an answer), but must be there.
    let accept = || -> R<String> { string(required(fields, "accept", &at)?, &at.key("accept")) };
    Ok(match kind.as_str() {
        "host-key" => {
            known(fields, &["key_type", "fingerprint"], &at)?;
            Step::HostKey { key_type: field("key_type")?, fingerprint: field("fingerprint")? }
        }
        "password" => {
            known(fields, &["prompt", "accept"], &at)?;
            Step::Password { prompt: opt("prompt")?, accept: accept()? }
        }
        "passphrase" => {
            known(fields, &["key_path", "accept"], &at)?;
            Step::Passphrase { key_path: field("key_path")?, accept: accept()? }
        }
        "code" => {
            known(fields, &["prompt", "accept"], &at)?;
            Step::Code { prompt: opt("prompt")?, accept: accept()? }
        }
        "confirm" => {
            known(fields, &["text"], &at)?;
            Step::Confirm { text: field("text")? }
        }
        "notice" => {
            known(fields, &["text"], &at)?;
            Step::Notice { text: field("text")? }
        }
        other => return at.err(format!("unknown step {other:?} (expected {KINDS})")),
    })
}

fn node(v: &Value, at: &At) -> R<Node> {
    if let Some(m) = v.as_object() {
        known(m, &["error"], at)?;
        let at = at.key("error");
        let e = match string(required(m, "error", &at)?, &at)?.as_str() {
            "permission-denied" => TreeError::PermissionDenied,
            "not-found" => TreeError::NotFound,
            "connection-lost" => TreeError::ConnectionLost,
            other => return at.err(format!("unknown error {other:?} (expected permission-denied, not-found or connection-lost)")),
        };
        return Ok(Node::Error(e));
    }
    let Some(items) = v.as_array() else {
        return at.err(format!("expected a list of entries or {{\"error\": …}}, found {}", what(v)));
    };
    let mut out = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for (i, v) in items.iter().enumerate() {
        let at = at.item("entry", i);
        let item = item(v, &at)?;
        if let Item::Entry(e) = &item {
            if names.contains(&e.name) {
                return at.err(format!("{:?} is listed twice", e.name));
            }
            names.push(e.name.clone());
        }
        out.push(item);
    }
    Ok(Node::Entries(out))
}

fn kind(v: &Value, at: &At) -> R<EntryKind> {
    Ok(match string(v, at)?.as_str() {
        "file" => EntryKind::File,
        "dir" => EntryKind::Dir,
        "symlink" => EntryKind::Symlink { target: None },
        "other" => EntryKind::Other,
        other => return at.err(format!("unknown kind {other:?} (expected file, dir, symlink or other)")),
    })
}

/// A mode: a number, or an octal string (`"0644"`, `"100644"`). Bare permissions get the
/// kind's type bits.
fn mode(v: &Value, kind: EntryKind, at: &At) -> R<u32> {
    let bits = match v {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => u32::from_str_radix(s.trim_start_matches("0o"), 8).ok(),
        _ => None,
    };
    let Some(bits) = bits else { return at.err(format!("{v} is not a mode (a number, or octal like \"0644\")")) };
    let types = default_mode(kind).unwrap_or(0) & !0o7777;
    Ok(if bits <= 0o7777 { types | bits } else { bits })
}

fn item(v: &Value, at: &At) -> R<Item> {
    let m = object(v, at)?;
    if let Some(g) = m.get("generate") {
        known(m, &["generate"], at)?;
        return Ok(Item::Generate(generator(g, &at.key("generate"))?));
    }
    known(m, &["name", "kind", "target", "size", "mtime", "mode"], at)?;
    let name = text(required(m, "name", at)?, &at.key("name"))?;
    if name.contains('/') || name == "." || name == ".." {
        return at.key("name").err(format!("{name:?} is not a file name"));
    }
    // A bad kind reads at the entry, where a reader looks for it.
    let mut kind = kind(required(m, "kind", at)?, at)?;
    if let Some(v) = optional(m, "target") {
        let at = at.key("target");
        if !kind.is_symlink() {
            return at.err("only a symlink has a target");
        }
        let target = match string(v, &at)?.as_str() {
            "file" => LinkTarget::File,
            "dir" => LinkTarget::Dir,
            "other" => LinkTarget::Other,
            other => return at.err(format!("unknown target {other:?} (expected file, dir, other, or null for a dangling link)")),
        };
        kind = EntryKind::Symlink { target: Some(target) };
    }
    let size = optional(m, "size").map(|v| uint(v, &at.key("size"))).transpose()?.unwrap_or_else(|| default_size(kind));
    let mtime = optional(m, "mtime").map(|v| time(v, &at.key("mtime"))).transpose()?;
    let mode = match optional(m, "mode") {
        Some(v) => Some(mode(v, kind, &at.key("mode"))?),
        None => default_mode(kind),
    };
    Ok(Item::Entry(Entry { name, kind, size, mtime, mode }))
}

fn generator(v: &Value, at: &At) -> R<Generator> {
    let m = object(v, at)?;
    known(m, &["pattern", "count", "start", "kind", "size", "mtime", "mode", "seed"], at)?;
    let pattern = Pattern::parse(&text(required(m, "pattern", at)?, &at.key("pattern"))?).or_else(|e| at.key("pattern").err(e))?;
    let count = uint(required(m, "count", at)?, &at.key("count"))?;
    if count > MAX_COUNT {
        return at.key("count").err(format!("{count} is more than {MAX_COUNT}"));
    }
    let start = optional(m, "start").map(|v| uint(v, &at.key("start"))).transpose()?.unwrap_or(1);
    let kind = match optional(m, "kind") {
        Some(v) => self::kind(v, &at.key("kind"))?,
        None => EntryKind::File,
    };
    let size = match optional(m, "size") {
        None => Size::Fixed(default_size(kind)),
        Some(Value::Array(r)) => {
            let at = at.key("size");
            let (Some(min), Some(max), 2) = (r.first().and_then(Value::as_u64), r.get(1).and_then(Value::as_u64), r.len()) else {
                return at.err("a range is [min, max], whole numbers");
            };
            if min > max {
                return at.err(format!("[{min}, {max}]: min is above max"));
            }
            Size::Range(min, max)
        }
        Some(v) => Size::Fixed(uint(v, &at.key("size"))?),
    };
    let mtime = match optional(m, "mtime") {
        None => Mtime::None,
        Some(Value::Object(s)) => {
            let at = at.key("mtime");
            known(s, &["end", "every", "jitter"], &at)?;
            let end = time(required(s, "end", &at)?, &at.key("end"))?;
            let every = uint(required(s, "every", &at)?, &at.key("every"))? as i64;
            let jitter = optional(s, "jitter").map(|v| uint(v, &at.key("jitter"))).transpose()?.unwrap_or(0) as i64;
            Mtime::Series { end, every, jitter }
        }
        Some(v) => Mtime::Fixed(time(v, &at.key("mtime"))?),
    };
    let mode = optional(m, "mode").map(|v| mode(v, kind, &at.key("mode"))).transpose()?;
    let seed = optional(m, "seed").map(|v| string(v, &at.key("seed"))).transpose()?;
    Ok(Generator { pattern, count, start, kind, size, mtime, mode, seed })
}
