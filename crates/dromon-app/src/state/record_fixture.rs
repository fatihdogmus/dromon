//! A recording's fixture (`docs/fixtures.md`): the hosts its steps used, each server's home, its
//! sign-in questions by kind with placeholder answers, and the folders it listed, the first time
//! each was listed, with their errors. Every name goes through the report's [`Redactor`]
//! (all numbered first in their natural order, so sorting a folder by name keeps its order);
//! sizes, dates and permissions stay, owners and groups go, and ssh's own words for a failed
//! connection are left out.
//!
//! The fixture is written one entry a line, to read and to diff. One that would pass [`CAP`]
//! characters sums up its largest folders first: the rows no step touched become a generator
//! of as many files and one of as many folders ("more-1", "more-dir-1", …).

use std::collections::{BTreeMap, BTreeSet};

use dromon_core::{Entry, EntryKind, HostId, HostSource, LinkTarget, RemotePath, SortDir, SortKey, sort_entries};

use super::{Ask, Folder, Recording, Server, Step, Taken};
use crate::fixture::{default_mode, default_size};
use crate::state::redact::Redactor;
use crate::state::{App, Sort};

/// The most characters a fixture's folders take before the largest are summed up: with the
/// rest of a report it stays within GitHub's 65,536 characters for an issue.
pub const CAP: usize = 40_000;
/// What a fixture accepts for a password or a passphrase, and for a code.
pub const SECRET: &str = "pw";
pub const CODE: &str = "123456";
/// What the replay types for an answer the server refused.
pub const WRONG: &str = "wrong";
/// A failed connection's last words: ssh's own name hosts, users or files.
const FAIL: &str = "Couldn't connect (ssh's own words aren't in the recording: they can name hosts, users or files)";

/// What the replay's servers hold, in stand-ins, for the script to find rows by: each listed
/// folder's entries (summed-up ones by the names their generators give) by real host and path.
#[derive(Debug, Default)]
pub struct Model {
    folders: BTreeMap<(HostId, RemotePath), Vec<Entry>>,
    homes: BTreeMap<HostId, RemotePath>,
    /// How many folders were summed up.
    pub summarised: usize,
}

impl Model {
    /// Where `host`'s sessions start in the replay.
    pub fn home(&self, host: &HostId) -> RemotePath {
        self.homes.get(host).cloned().unwrap_or_else(RemotePath::root)
    }

    /// The names of the rows the replay shows in `folder`, sorted by `sort`, hidden ones only
    /// when `hidden`.
    pub fn rows(&self, host: &HostId, folder: &RemotePath, sort: Sort, hidden: bool) -> Option<Vec<String>> {
        let mut entries = self.folders.get(&(host.clone(), folder.clone()))?.clone();
        sort_entries(&mut entries, sort.key, sort.dir);
        Some(entries.into_iter().filter(|e| hidden || !e.hidden()).map(|e| e.name).collect())
    }

    fn entries(&mut self, host: &HostId, folder: &RemotePath) -> Option<&mut Vec<Entry>> {
        self.folders.get_mut(&(host.clone(), folder.clone()))
    }

    /// A row renamed in the replay, `from` to `to` (stand-ins).
    pub fn rename(&mut self, host: &HostId, folder: &RemotePath, from: &str, to: &str) {
        if let Some(e) = self.entries(host, folder).and_then(|es| es.iter_mut().find(|e| e.name == from)) {
            e.name = to.to_owned();
        }
    }

    /// A folder made in the replay.
    pub fn add_dir(&mut self, host: &HostId, folder: &RemotePath, name: &str) {
        if let Some(es) = self.entries(host, folder) {
            es.push(Entry { name: name.to_owned(), ..dir_entry() });
        }
    }

    /// Rows deleted in the replay.
    pub fn remove(&mut self, host: &HostId, folder: &RemotePath, names: &[String]) {
        if let Some(es) = self.entries(host, folder) {
            es.retain(|e| !names.contains(&e.name));
        }
    }
}

fn dir_entry() -> Entry {
    let kind = EntryKind::Dir;
    Entry {
        name: String::new(),
        kind,
        size: default_size(kind),
        mtime: None,
        mode: default_mode(kind),
        owner: None,
        group: None,
    }
}

/// `s` as a JSON string.
pub fn json(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

/// One listed folder on its way into the fixture.
struct Listed {
    host: HostId,
    path: RemotePath,
    /// Its path in stand-ins, once named.
    key: String,
    node: Node,
}

enum Node {
    /// Its entries, each with whether a step touched it.
    Entries(Vec<(Entry, bool)>),
    /// Summed up: the entries steps touched, and the files and the number of folders that go
    /// into generators.
    Summed {
        kept: Vec<Entry>,
        files: Vec<Entry>,
        dirs: usize,
    },
    Error(&'static str),
}

impl Node {
    fn lines(&self) -> Vec<String> {
        match self {
            Node::Entries(all) => all.iter().map(|(e, _)| entry_json(e)).collect(),
            Node::Summed { kept, files, dirs } => {
                let mut out: Vec<String> = kept.iter().map(entry_json).collect();
                out.extend(files_generator(files));
                if *dirs > 0 {
                    out.push(format!(
                        "{{\"generate\":{{\"pattern\":\"more-dir-{{}}\",\"count\":{dirs},\"kind\":\"dir\"}}}}"
                    ));
                }
                out
            }
            Node::Error(_) => Vec::new(),
        }
    }

    fn json(&self) -> String {
        match self {
            Node::Error(e) => format!("{{\"error\":\"{e}\"}}"),
            _ => format!("[\n{}]", self.lines().join(",\n")),
        }
    }

    fn len(&self) -> usize {
        self.lines().iter().map(|l| l.len() + 2).sum()
    }

    /// The names it writes, each with whether it's a folder.
    fn names(&self) -> Vec<(&str, bool)> {
        let entries: Vec<&Entry> = match self {
            Node::Entries(all) => all.iter().map(|(e, _)| e).collect(),
            Node::Summed { kept, .. } => kept.iter().collect(),
            Node::Error(_) => Vec::new(),
        };
        entries.into_iter().map(|e| (e.name.as_str(), e.kind.is_dir_like())).collect()
    }
}

/// One entry's fixture line, what isn't the kind's default left out.
fn entry_json(e: &Entry) -> String {
    let (kind, target) = match e.kind {
        EntryKind::File => ("file", None),
        EntryKind::Dir => ("dir", None),
        EntryKind::Other => ("other", None),
        EntryKind::Symlink { target } => ("symlink", Some(target)),
    };
    let mut s = format!("{{\"name\":{},\"kind\":\"{kind}\"", json(&e.name));
    match target {
        Some(Some(LinkTarget::File)) => s.push_str(",\"target\":\"file\""),
        Some(Some(LinkTarget::Dir)) => s.push_str(",\"target\":\"dir\""),
        Some(Some(LinkTarget::Other)) => s.push_str(",\"target\":\"other\""),
        Some(None) => s.push_str(",\"target\":null"),
        None => {}
    }
    if e.size != default_size(e.kind) {
        s.push_str(&format!(",\"size\":{}", e.size));
    }
    if let Some(m) = e.mtime {
        s.push_str(&format!(",\"mtime\":{m}"));
    }
    let perms = |m: Option<u32>| m.map(|m| m & 0o7777);
    if let Some(m) = perms(e.mode).filter(|m| Some(*m) != perms(default_mode(e.kind))) {
        s.push_str(&format!(",\"mode\":\"{m:04o}\""));
    }
    s.push('}');
    s
}

/// A generator of as many files as `files`, their sizes' range and their dates' span.
fn files_generator(files: &[Entry]) -> Option<String> {
    let count = files.len() as i64;
    let min = files.iter().map(|e| e.size).min()?;
    let max = files.iter().map(|e| e.size).max()?;
    let mut g = format!("{{\"generate\":{{\"pattern\":\"more-{{}}\",\"count\":{count},\"size\":[{min},{max}]");
    let dates: Vec<i64> = files.iter().filter_map(|e| e.mtime).collect();
    if let (Some(first), Some(last)) = (dates.iter().min(), dates.iter().max()) {
        let every = ((last - first) / count).max(1);
        g.push_str(&format!(",\"mtime\":{{\"end\":{last},\"every\":{every}}}"));
    }
    g.push_str("}}");
    Some(g)
}

/// The hosts the steps used, or the recording started on.
fn used(rec: &Recording, steps: &[Taken]) -> BTreeSet<HostId> {
    let mut out: BTreeSet<HostId> = rec.start.iter().map(|p| p.host.clone()).collect();
    for t in steps {
        out.extend(t.at.iter().chain(&t.after).map(|p| p.host.clone()));
        match &t.step {
            Step::Connect(h) | Step::Disconnect(h) | Step::Decline(h) | Step::Dismiss(Some(h)) => {
                out.insert(h.clone());
            }
            Step::Answer { host, .. } => {
                out.insert(host.clone());
            }
            _ => {}
        }
    }
    out
}

/// The rows the steps name, by host and folder: they stay whole when a folder is summed up.
fn touched(rec: &Recording, steps: &[Taken]) -> BTreeMap<(HostId, RemotePath), BTreeSet<String>> {
    let mut out: BTreeMap<(HostId, RemotePath), BTreeSet<String>> = BTreeMap::new();
    for t in steps {
        let Some(super::Place { host, folder: Some(folder) }) = &t.at else { continue };
        let row = match &t.step {
            Step::Open(r) | Step::Click { row: r, .. } | Step::Rename(r) | Step::Delete(r) | Step::Properties(r) => r,
            Step::TypeSelect(Some(r)) | Step::Preview(Some(r)) => r,
            _ => continue,
        };
        out.entry((host.clone(), folder.clone())).or_default().insert(row.name.clone());
    }
    // The way to every folder listed, and to each home.
    for (host, s) in &rec.servers {
        for path in s.folders.keys().chain(&s.home) {
            for p in path.ancestors_from_root().into_iter().skip(1) {
                if let (Some(parent), Some(name)) = (p.parent(), p.file_name()) {
                    out.entry((host.clone(), parent)).or_default().insert(name.to_owned());
                }
            }
        }
    }
    out
}

/// The folders of one server, entries in their natural order, owners left out; still by their
/// own names, which [`named`] replaces once [`fit`] has chosen what goes in.
fn listed(host: &HostId, s: &Server, touched: &BTreeMap<(HostId, RemotePath), BTreeSet<String>>) -> Vec<Listed> {
    let none = BTreeSet::new();
    let mut out = Vec::new();
    for (path, folder) in &s.folders {
        let node = match folder {
            Folder::Listed(entries) => {
                let marks = touched.get(&(host.clone(), path.clone())).unwrap_or(&none);
                let mut entries = entries.clone();
                sort_entries(&mut entries, SortKey::Name, SortDir::Ascending);
                let marked = entries.into_iter().map(|e| (marks.contains(&e.name), e));
                Node::Entries(marked.map(|(touched, e)| (Entry { owner: None, group: None, ..e }, touched)).collect())
            }
            Folder::Denied => Node::Error("permission-denied"),
            Folder::Missing => Node::Error("not-found"),
            Folder::Lost => Node::Error("connection-lost"),
        };
        out.push(Listed { host: host.clone(), path: path.clone(), key: String::new(), node });
    }
    out
}

/// `node` with stand-ins for the names it writes, in its order (a summed-up folder's files
/// write none).
fn named(node: Node, r: &mut Redactor) -> Node {
    let mut name = |e: Entry| Entry { name: r.name(&e.name, e.kind.is_dir_like()), ..e };
    match node {
        Node::Entries(all) => Node::Entries(all.into_iter().map(|(e, touched)| (name(e), touched)).collect()),
        Node::Summed { kept, files, dirs } => Node::Summed { kept: kept.into_iter().map(name).collect(), files, dirs },
        error @ Node::Error(_) => error,
    }
}

/// Sums up the largest folders until the fixture's folders fit [`CAP`]; how many it summed up.
fn fit(folders: &mut [Listed]) -> usize {
    let lens: Vec<usize> = folders.iter().map(|f| f.node.len()).collect();
    let mut total: usize = lens.iter().sum();
    let mut order: Vec<usize> = (0..folders.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lens[i]));
    let mut summed = 0;
    for i in order {
        if total <= CAP {
            break;
        }
        let Node::Entries(all) = &folders[i].node else { continue };
        let all = all.clone();
        let before = lens[i];
        let (kept, rest): (Vec<_>, Vec<_>) = all.into_iter().partition(|(_, touched)| *touched);
        let (dirs, files): (Vec<Entry>, Vec<Entry>) =
            rest.into_iter().map(|(e, _)| e).partition(|e| e.kind.is_dir_like());
        folders[i].node = Node::Summed { kept: kept.into_iter().map(|(e, _)| e).collect(), files, dirs: dirs.len() };
        total = total + folders[i].node.len() - before;
        summed += 1;
    }
    summed
}

/// What the replay's server lists for a folder, for the model.
fn model_entries(node: &Node) -> Option<Vec<Entry>> {
    match node {
        Node::Entries(all) => Some(all.iter().map(|(e, _)| e.clone()).collect()),
        Node::Summed { kept, files, dirs } => {
            let mut out = kept.clone();
            let file = |n: usize, e: &Entry| Entry { name: format!("more-{n}"), mtime: None, ..e.clone() };
            out.extend(files.iter().enumerate().map(|(i, e)| file(i + 1, e)));
            out.extend((1..=*dirs).map(|n| Entry { name: format!("more-dir-{n}"), ..dir_entry() }));
            Some(out)
        }
        Node::Error(_) => None,
    }
}

/// A sign-in question as a fixture step.
fn ask_json(ask: &Ask, alias: &str) -> String {
    match ask {
        Ask::HostKey { key_type } => {
            let key_type = key_type
                .as_deref()
                .filter(|k| k.len() <= 16 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
                .unwrap_or("ED25519");
            let mut print: String =
                format!("recorded{alias}hostkey").chars().filter(char::is_ascii_alphanumeric).collect();
            while print.len() < 43 {
                print.push('A');
            }
            print.truncate(43);
            format!("{{\"host-key\":{{\"key_type\":{},\"fingerprint\":\"SHA256:{print}\"}}}}", json(key_type))
        }
        Ask::Password => format!("{{\"password\":{{\"accept\":\"{SECRET}\"}}}}"),
        Ask::Passphrase => {
            format!("{{\"passphrase\":{{\"key_path\":\"~/.ssh/{alias}-key\",\"accept\":\"{SECRET}\"}}}}")
        }
        Ask::Code => format!("{{\"code\":{{\"accept\":\"{CODE}\"}}}}"),
        Ask::Confirm => "{\"confirm\":{\"text\":\"Allow use of the key?\"}}".to_owned(),
        Ask::Notice => "{\"notice\":{\"text\":\"Confirm user presence for the key\"}}".to_owned(),
    }
}

/// The recording's fixture in its fenced block, and the model of what it serves.
pub fn build(rec: &Recording, steps: &[Taken], app: &App, r: &mut Redactor, version: &str) -> (String, Model) {
    let used = used(rec, steps);
    let touched = touched(rec, steps);
    let mut model = Model::default();
    let mut hosts = Vec::new();
    let mut used_servers = Vec::new();
    let mut folders = Vec::new();
    for host in rec.order.iter().filter(|h| used.contains(*h)) {
        let Some(s) = rec.servers.get(host) else { continue };
        let alias = r.host(host);
        let source = match app.hosts.get(host).map(|h| h.source) {
            Some(HostSource::SshConfig) => "ssh-config",
            Some(HostSource::Saved) => "saved",
            _ => "recent",
        };
        hosts.push(format!("{{\"alias\":{},\"source\":\"{source}\"}}", json(&alias)));
        folders.extend(listed(host, s, &touched));
        used_servers.push((host, alias, s));
    }
    model.summarised = fit(&mut folders);
    // Every name numbered first, in the order sorting by name gives, so each folder's stand-ins
    // sort as its rows do and a name in two folders is one stand-in.
    let homes: Vec<RemotePath> = used_servers.iter().filter_map(|(_, _, s)| s.home.clone()).collect();
    let mut names: Vec<(&str, bool)> = folders.iter().flat_map(|f| f.node.names()).collect();
    names.extend(folders.iter().map(|f| &f.path).chain(&homes).flat_map(|p| p.components().map(|c| (c, true))));
    r.number_in_order(names);
    for f in &mut folders {
        f.node = named(std::mem::replace(&mut f.node, Node::Error("")), r);
        f.key = r.remote_path(&f.path);
        if let Some(entries) = model_entries(&f.node) {
            model.folders.insert((f.host.clone(), f.path.clone()), entries);
        }
    }
    let mut servers = Vec::new();
    for (host, alias, s) in used_servers {
        let home = s.home.clone().unwrap_or_else(RemotePath::root);
        let mut head = format!("{}: {{\"home\":{}", json(&alias), json(&r.remote_path(&home)));
        model.homes.insert(host.clone(), home);
        if !s.auth.is_empty() {
            let auth: Vec<String> = s.auth.iter().map(|a| ask_json(a, &alias)).collect();
            head.push_str(&format!(",\n\"auth\": [{}]", auth.join(", ")));
        }
        if s.failed && !s.opened {
            head.push_str(&format!(",\n\"fail\": {}", json(FAIL)));
        }
        let tree: Vec<String> = folders
            .iter()
            .filter(|f| f.host == *host)
            .map(|f| format!("{}: {}", json(&f.key), f.node.json()))
            .collect();
        servers.push(format!("{head},\n\"tree\": {{\n{}}}}}", tree.join(",\n")));
    }
    let description =
        format!("Recorded with Dromon {version}: what the servers answered while recording, names replaced");
    let text = format!(
        "{{\"version\": 1,\n\"description\": {},\n\"now\": {},\n\"hosts\": [\n{}],\n\"servers\": {{\n{}}}}}",
        json(&description),
        app.now,
        hosts.join(",\n"),
        servers.join(",\n")
    );
    (format!("```json fixture\n{text}\n```"), model)
}
