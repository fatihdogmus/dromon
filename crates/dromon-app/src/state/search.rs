//! Search in subfolders: the filter field's other scope. "This folder" narrows the rows on
//! screen (`conn.rs`'s [`super::Filter`]); "Subfolders" (⌘⇧F, the palette's "Search in
//! subfolders…") finds names anywhere below the folder on screen, and its results take the
//! list area's place until the search ends.
//!
//! The search is a small machine ([`Phase`], stepped by the pure [`step`]): Idle → Waiting
//! (typed; it starts once typing pauses for [`DEBOUNCE`] on the window's clock, which the
//! view's poll passes in) → Searching (results stream in) → Done or Failed. The walk is the
//! service's (`Command::Search`, `service/search.rs`): the Properties walk matching names
//! with [`Matcher`] instead of adding sizes, stopping at [`CAP`] results.
//!
//! Each search has an id of this window's ([`SearchId`], numbered from the window's own range
//! as folder size walks are), so an answer for one that isn't on screen (an older one, one
//! that ended) changes nothing. Escape, clearing the field, switching back to "This folder",
//! navigating, another host and a disconnect all end it: [`App::sync_search`] runs after every
//! action and event, cancels the walk still running and the folder view is back as it was
//! (the search never touched it). While results are on screen, the list's actions are theirs
//! ([`App::results_action`]): opening, the selection, Download, Copy path and Properties;
//! what would change the folder view out of sight does nothing.

use dromon_core::{Entry, Error, HostId, RemotePath};
use dromon_transfer::RemoteItem;

use super::clip::paths_text;
use super::files::FileAction;
use super::menu::{MenuKind, OpenMenu};
use super::properties::PropertiesView;
use super::selection::Selection;
use super::service::Command;
use super::{Action, App, Effect, Filter, Nav, hosts};

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;

/// How long typing pauses before a search starts, in seconds: a walk isn't restarted for every
/// key.
pub const DEBOUNCE: f64 = 0.25;

/// Results a search stops at: the walk ends there, and the line says so.
pub const CAP: usize = 5_000;

/// Identifies one search, chosen by the UI (unique per session: each window numbers its own, as
/// it does its [`SizeId`](super::SizeId)s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SearchId(pub u64);

/// One search result: the entry, and the folder it is in relative to where the search started
/// (`assets/img`; `.` for that folder itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub folder: String,
    pub entry: Entry,
}

impl Found {
    /// The folder the entry is in, under the search's `root`.
    pub fn dir(&self, root: &RemotePath) -> RemotePath {
        root.join(&self.folder)
    }

    /// The entry's full path, under the search's `root`.
    pub fn path(&self, root: &RemotePath) -> RemotePath {
        self.dir(root).join(&self.entry.name)
    }
}

/// What the UI asks of the service about searches in subfolders (`Command::Search`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchCommand {
    /// Walk `root` on the host's session, as a folder size walk does (links never followed),
    /// for entries whose name matches `query` ([`Matcher`]); with `show_hidden` off, hidden
    /// entries are neither matched nor walked into. Answered to the sending window only with
    /// [`SearchEvent::Found`] batches (at most about ten a second), then [`SearchEvent::Done`],
    /// or [`SearchEvent::Failed`] when the walk can't go on; a host that isn't connected answers
    /// `Failed` with [`Error::ConnectionLost`] at once. The walk stops at [`CAP`] results.
    Start { id: SearchId, host: HostId, root: RemotePath, query: String, show_hidden: bool },
    /// Stop the search `id`. Nothing answers; one that already ended, or an unknown id, is ignored.
    Cancel { id: SearchId, host: HostId },
}

/// What the service tells the UI about a search in subfolders (`ServiceEvent::Search`).
#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    /// What the search `id` found since its last batch, in the order found (maybe nothing), and
    /// how many folders it has searched so far.
    Found { id: SearchId, host: HostId, entries: Vec<Found>, folders: u64 },
    /// The search `id` ended: it searched `folders` folders, `unreadable` more couldn't be read,
    /// and `capped` says it stopped at [`CAP`] results.
    Done { id: SearchId, host: HostId, folders: u64, unreadable: u64, capped: bool },
    /// The search `id` stopped (the connection was lost); what it found stays found.
    Failed { id: SearchId, host: HostId, error: Error },
}

/// A `Search` event's trace line (`DROMON_TRACE=1`): its kind, host, id and counts, never what
/// it found.
pub fn trace_line(ev: &SearchEvent) -> String {
    match ev {
        SearchEvent::Found { id, host, entries, folders } => {
            format!("SearchFound host={host} id={} entries={} folders={folders}", id.0, entries.len())
        }
        SearchEvent::Done { id, host, folders, unreadable, capped } => {
            format!("SearchDone host={host} id={} folders={folders} unreadable={unreadable} capped={capped}", id.0)
        }
        SearchEvent::Failed { id, host, .. } => format!("SearchFailed host={host} id={}", id.0),
    }
}

/// How a query matches names, ignoring case: a glob of the whole name when it has `*` or `?`
/// (`*.log`, `backup-202?-*`), else a substring of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Matcher {
    Contains(String),
    Glob(Vec<char>),
}

impl Matcher {
    pub fn new(query: &str) -> Matcher {
        let q = query.to_lowercase();
        if q.contains(['*', '?']) { Matcher::Glob(q.chars().collect()) } else { Matcher::Contains(q) }
    }

    pub fn matches(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        match self {
            Matcher::Contains(q) => name.contains(q.as_str()),
            Matcher::Glob(p) => glob(p, &name.chars().collect::<Vec<_>>()),
        }
    }
}

/// Whether all of `name` matches `pattern`: `*` is any run of characters, `?` one. On a
/// mismatch it goes back to the last `*` and lets it take one more character, so it never
/// backtracks further than that.
fn glob(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some(&'*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((sp, sn)) => {
                    star = Some((sp, sn + 1));
                    p = sp + 1;
                    n = sn + 1;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// Which the filter field does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// Narrow the folder on screen.
    #[default]
    Folder,
    /// Search everything below it.
    Subfolders,
}

/// Where the search stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    /// No search: the folder view.
    #[default]
    Idle,
    /// Typed `query`: it starts at `due` (the window's clock) unless typing goes on.
    Waiting { query: String, due: f64 },
    /// The walk `id` is under way and has searched `folders` folders.
    Searching { id: SearchId, folders: u64 },
    /// It went through everything (or stopped at [`CAP`], `capped`).
    Done { id: SearchId, folders: u64, unreadable: u64, capped: bool },
    /// It stopped: why.
    Failed { id: SearchId, error: Error },
}

impl Phase {
    /// The walk running, if any.
    pub fn running(&self) -> Option<SearchId> {
        match self {
            Phase::Searching { id, .. } => Some(*id),
            _ => None,
        }
    }
}

/// What happens to a search.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The field's text is `query` at `now`: it starts after [`DEBOUNCE`]; empty ends it.
    Typed {
        query: String,
        now: f64,
    },
    /// Search for `query` at once as `id` (the scope switched with text in the field).
    Start {
        id: SearchId,
        query: String,
    },
    /// The window's clock is `now`: a waiting search whose time came starts as `next`.
    Tick {
        now: f64,
        next: SearchId,
    },
    Found {
        id: SearchId,
        entries: Vec<Found>,
        folders: u64,
    },
    Done {
        id: SearchId,
        folders: u64,
        unreadable: u64,
        capped: bool,
    },
    Failed {
        id: SearchId,
        error: Error,
    },
    /// Back to the folder view.
    End,
}

/// What a step asks of its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Ask the service for the search `id`.
    Send { id: SearchId, query: String },
    /// Stop the walk `id`.
    Cancel(SearchId),
}

/// This window's search: its scope, where it stands, and what it found.
#[derive(Clone, Debug, Default)]
pub struct Search {
    pub scope: Scope,
    pub phase: Phase,
    /// What the search on screen looks for.
    pub query: String,
    /// What it found, in the order found; kept on screen while the next one waits to start.
    pub found: Vec<Found>,
    /// The selection among the results, by [`hit_key`].
    pub selection: Selection,
    /// Bumped whenever `found` is replaced rather than added to.
    pub version: u64,
    /// The host and folder the search is about (the one on screen when it started).
    pub origin: Option<(HostId, RemotePath)>,
    /// This window's last search number.
    last: u64,
}

/// A result's key, as rows and the selection name it: `hit-<n>`, in the order found (a
/// folder's path has slashes, which element paths can't hold).
pub fn hit_key(i: usize) -> String {
    format!("hit-{i}")
}

fn hit_index(key: &str) -> Option<usize> {
    key.strip_prefix("hit-")?.parse().ok()
}

impl Search {
    /// Whether results are on screen in place of the folder.
    pub fn is_on(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// When a waiting search starts.
    pub fn due(&self) -> Option<f64> {
        match self.phase {
            Phase::Waiting { due, .. } => Some(due),
            _ => None,
        }
    }

    pub fn hit(&self, key: &str) -> Option<&Found> {
        self.found.get(hit_index(key)?)
    }

    fn keys(&self) -> Vec<String> {
        (0..self.found.len()).map(hit_key).collect()
    }

    /// The selected results in order, or the cursor's when none is selected.
    fn picked(&self) -> Vec<&Found> {
        let selected: Vec<&Found> = self
            .found
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selection.contains(&hit_key(*i)))
            .map(|(_, f)| f)
            .collect();
        if selected.is_empty() {
            self.selection.cursor().and_then(|k| self.hit(k)).into_iter().collect()
        } else {
            selected
        }
    }
}

/// Steps `s` by `ev`; what the owner should send. A new search clears what the last one
/// found; answers count only for the walk running (anything else is stale and ignored).
pub fn step(s: &mut Search, ev: Event) -> Vec<Out> {
    let running = s.phase.running();
    let cancel: Vec<Out> = running.map(Out::Cancel).into_iter().collect();
    match ev {
        Event::Typed { query, .. } | Event::Start { query, .. } if query.is_empty() => {
            end(s);
            cancel
        }
        Event::Typed { query, now } => {
            s.phase = Phase::Waiting { query, due: now + DEBOUNCE };
            cancel
        }
        Event::Start { id, query } => cancel.into_iter().chain(start(s, id, query)).collect(),
        Event::Tick { now, next } => match &s.phase {
            Phase::Waiting { query, due } if *due <= now => {
                let query = query.clone();
                start(s, next, query)
            }
            _ => Vec::new(),
        },
        Event::Found { id, entries, folders } if running == Some(id) => {
            s.found.extend(entries);
            s.phase = Phase::Searching { id, folders };
            Vec::new()
        }
        Event::Done { id, folders, unreadable, capped } if running == Some(id) => {
            s.phase = Phase::Done { id, folders, unreadable, capped };
            Vec::new()
        }
        Event::Failed { id, error } if running == Some(id) => {
            s.phase = Phase::Failed { id, error };
            Vec::new()
        }
        Event::End => {
            end(s);
            cancel
        }
        Event::Found { .. } | Event::Done { .. } | Event::Failed { .. } => Vec::new(),
    }
}

fn start(s: &mut Search, id: SearchId, query: String) -> Vec<Out> {
    s.phase = Phase::Searching { id, folders: 0 };
    s.query = query.clone();
    s.found.clear();
    s.selection.clear();
    s.version += 1;
    vec![Out::Send { id, query }]
}

fn end(s: &mut Search) {
    s.phase = Phase::Idle;
    s.query.clear();
    s.found.clear();
    s.selection.clear();
    s.version += 1;
    s.origin = None;
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", crate::fmt::thousands(n), if n == 1 { one } else { many })
}

/// The line above the results: "Searching… 1,204 folders", "37 results in 2,150 folders"…
pub fn status_text(s: &Search) -> String {
    let results = s.found.len() as u64;
    match &s.phase {
        Phase::Idle | Phase::Waiting { .. } => "Searching…".into(),
        Phase::Searching { folders, .. } => format!("Searching… {}", count(*folders, "folder", "folders")),
        Phase::Done { folders, capped: true, .. } => format!(
            "First {} in {}; the search stopped there",
            count(results, "result", "results"),
            count(*folders, "folder", "folders")
        ),
        Phase::Done { folders, .. } => {
            format!("{} in {}", count(results, "result", "results"), count(*folders, "folder", "folders"))
        }
        Phase::Failed { error, .. } => format!("Search stopped: {error}"),
    }
}

/// "3 folders couldn't be read", once the search is done and some couldn't.
pub fn unreadable_text(s: &Search) -> Option<String> {
    match s.phase {
        Phase::Done { unreadable: n @ 1.., .. } => Some(format!("{} couldn't be read", count(n, "folder", "folders"))),
        _ => None,
    }
}

/// The search that ended, and what a screen reader is told of it.
pub fn finished(s: &Search) -> Option<(SearchId, String)> {
    let id = match s.phase {
        Phase::Done { id, .. } | Phase::Failed { id, .. } => id,
        _ => return None,
    };
    let text = std::iter::once(status_text(s)).chain(unreadable_text(s)).collect::<Vec<_>>().join(". ");
    Some((id, text))
}

/// Search's own actions (`Action::Search`).
#[derive(Clone, Debug, PartialEq)]
pub enum SearchAction {
    /// ⌘⇧F, the palette: the filter opens in Subfolders.
    Open,
    /// The field's scope switch.
    Scope(Scope),
    /// The field's text changed while in Subfolders, at `now` on the window's clock.
    Typed { text: String, now: f64 },
    /// The window's clock reached `now` (the view's poll, once a waiting search is due).
    Tick(f64),
}

impl App {
    /// The search whose results are on screen in place of the folder, if one is on.
    pub fn results(&self) -> Option<&Search> {
        Some(&self.ui.search).filter(|s| s.is_on())
    }

    fn next_search(&mut self) -> SearchId {
        self.ui.search.last += 1;
        SearchId((u64::from(self.window.0) << 32) + self.ui.search.last)
    }

    /// Steps the search and sends what it asks: a new walk of the folder on screen (the
    /// search's origin from then on), or cancelling the one on its origin's host.
    fn search_step(&mut self, ev: Event) {
        let before = self.ui.search.origin.clone();
        let user = matches!(ev, Event::Typed { .. } | Event::Start { .. } | Event::Tick { .. });
        if user {
            self.ui.search.origin = self.conn().map(|c| (c.host.clone(), c.cwd.clone()));
        }
        let outs = step(&mut self.ui.search, ev);
        for out in outs {
            match out {
                Out::Cancel(id) => {
                    if let Some((host, _)) = before.clone() {
                        self.send(Command::Search(SearchCommand::Cancel { id, host }));
                    }
                }
                Out::Send { id, query } => {
                    let Some(c) = self.conn() else { continue };
                    let (host, root, show_hidden) = (c.host.clone(), c.cwd.clone(), c.show_hidden);
                    self.send(Command::Search(SearchCommand::Start { id, host, root, query, show_hidden }));
                }
            }
        }
    }

    /// A search event from the service; stale ones change nothing.
    pub(super) fn search_event(&mut self, ev: SearchEvent) {
        let ev = match ev {
            SearchEvent::Found { id, entries, folders, .. } => Event::Found { id, entries, folders },
            SearchEvent::Done { id, folders, unreadable, capped, .. } => {
                Event::Done { id, folders, unreadable, capped }
            }
            SearchEvent::Failed { id, error, .. } => Event::Failed { id, error },
        };
        self.search_step(ev);
    }

    /// Search's actions of [`App::act`].
    pub(super) fn search_action(&mut self, action: SearchAction, fx: &mut Vec<Effect>) {
        match action {
            SearchAction::Open => {
                let Some(c) = self.conn_mut() else { return };
                if !c.filter.is_on() {
                    c.set_filter(Filter::On(String::new()));
                }
                self.ui.filter_edit.focused = true;
                if self.ui.search.scope != Scope::Subfolders {
                    self.set_scope(Scope::Subfolders, fx);
                }
            }
            SearchAction::Scope(scope) => self.set_scope(scope, fx),
            SearchAction::Typed { text, now } if self.ui.search.scope == Scope::Subfolders => {
                self.search_step(Event::Typed { query: text, now });
            }
            SearchAction::Typed { .. } => {}
            SearchAction::Tick(now) => {
                if self.ui.search.due().is_some_and(|due| due <= now) {
                    let next = self.next_search();
                    self.search_step(Event::Tick { now, next });
                    fx.push(Effect::ScrollTop);
                }
            }
        }
    }

    /// Switches the open filter's scope. The folder view's rows are filtered by the field only
    /// in "This folder"; "Subfolders" searches for what's in it at once.
    pub(super) fn set_scope(&mut self, scope: Scope, fx: &mut Vec<Effect>) {
        if !self.conn().is_some_and(|c| c.filter.is_on()) {
            return;
        }
        self.ui.search.scope = scope;
        let text = self.ui.filter_edit.text.clone();
        match scope {
            Scope::Folder => {
                self.search_step(Event::End);
                if let Some(c) = self.conn_mut() {
                    c.set_filter(Filter::On(text));
                }
            }
            Scope::Subfolders => {
                if let Some(c) = self.conn_mut() {
                    c.set_filter(Filter::On(String::new()));
                }
                let id = self.next_search();
                self.search_step(Event::Start { id, query: text });
            }
        }
        fx.push(Effect::ScrollTop);
    }

    /// The query the folder view's rows are filtered by: none while the field searches
    /// subfolders.
    pub(super) fn folder_query(&self) -> String {
        match self.ui.search.scope {
            Scope::Folder => self.ui.filter_edit.text.clone(),
            Scope::Subfolders => String::new(),
        }
    }

    /// Hidden files were shown or hidden: a search on screen runs again with the new setting.
    pub(super) fn search_again(&mut self) {
        if self.ui.search.is_on() && self.ui.search.scope == Scope::Subfolders {
            let (id, query) = (self.next_search(), self.ui.filter_edit.text.clone());
            self.search_step(Event::Start { id, query });
        }
    }

    /// After every action and event: a closed filter is back in "This folder", and a search
    /// whose scope, host or folder is no longer on screen (or whose host disconnected) ends.
    pub(super) fn sync_search(&mut self) {
        if !self.conn().is_some_and(|c| c.filter.is_on()) {
            self.ui.search.scope = Scope::Folder;
        }
        if !self.ui.search.is_on() {
            return;
        }
        let here = self.conn().filter(|c| hosts::is_live(&c.status)).map(|c| (c.host.clone(), c.cwd.clone()));
        if self.ui.search.scope == Scope::Subfolders && here.is_some() && here == self.ui.search.origin {
            return;
        }
        self.search_step(Event::End);
        if self.files.menu.as_ref().is_some_and(|m| matches!(m.kind, MenuKind::Hit(_))) {
            self.files.menu = None;
        }
    }

    /// While results are on screen, the list's actions are theirs: this carries those out and
    /// returns the rest for [`App::act`] to. What would act on the folder view out of sight
    /// (new folder, rename, delete, the clip, type-to-select, sorting) does nothing.
    pub(super) fn results_action(&mut self, action: Action, fx: &mut Vec<Effect>) -> Option<Action> {
        if self.results().is_none() {
            return Some(action);
        }
        match action {
            Action::Open(key) => self.open_hit(&key, fx),
            Action::OpenSelection => {
                if let Some(key) = self.ui.search.selection.cursor().map(str::to_owned) {
                    self.open_hit(&key, fx);
                }
            }
            Action::Click { name, extend, toggle } => self.with_hits(fx, |sel, rows| {
                if extend {
                    sel.extend_to(rows, &name, toggle);
                } else if toggle {
                    sel.toggle(&name);
                } else {
                    sel.only(&name);
                }
                None
            }),
            Action::Move { delta, extend } => self.with_hits(fx, |sel, rows| sel.step(rows, delta, extend)),
            Action::End { last, extend } => self.with_hits(fx, |sel, rows| sel.jump_to_end(rows, last, extend)),
            Action::SelectAll => self.with_hits(fx, |sel, rows| {
                sel.select_all(rows);
                None
            }),
            Action::Download => self.download_hits(),
            Action::File(FileAction::CopyPath) => {
                let Some((_, root)) = &self.ui.search.origin else { return None };
                let paths: Vec<RemotePath> = self.ui.search.picked().iter().map(|f| f.path(root)).collect();
                fx.push(Effect::Copy(if paths.is_empty() { paths_text([root]) } else { paths_text(&paths) }));
            }
            Action::File(FileAction::Properties) => {
                if let Some(view) = self.hit_properties() {
                    self.ui.sheets.open_properties(view);
                }
            }
            Action::File(FileAction::OpenMenu { kind: MenuKind::Hit(key), at }) => {
                if self.ui.search.hit(&key).is_some() && !self.ui.search.selection.contains(&key) {
                    self.ui.search.selection.only(&key);
                }
                self.files.menu = at.map(|at| OpenMenu { kind: MenuKind::Hit(key), at });
            }
            Action::Type { .. }
            | Action::Sort(_)
            | Action::File(
                FileAction::NewFolder
                | FileAction::Rename
                | FileAction::Delete
                | FileAction::Copy
                | FileAction::Cut
                | FileAction::RequestPaste { .. }
                | FileAction::Paste { .. }
                | FileAction::DragStart(_),
            ) => {}
            other => return Some(other),
        }
        None
    }

    /// Runs a selection change over the results; a returned key is revealed.
    fn with_hits(&mut self, fx: &mut Vec<Effect>, f: impl FnOnce(&mut Selection, &[&str]) -> Option<String>) {
        let keys = self.ui.search.keys();
        let rows: Vec<&str> = keys.iter().map(String::as_str).collect();
        if let Some(key) = f(&mut self.ui.search.selection, &rows) {
            fx.push(Effect::Reveal(key));
        }
    }

    /// Enter or a double-click on a result: a folder opens; a file's folder opens with the
    /// file selected. Either way the search ends, as it does on any navigation.
    fn open_hit(&mut self, key: &str, fx: &mut Vec<Effect>) {
        let (Some(hit), Some((_, root))) = (self.ui.search.hit(key).cloned(), self.ui.search.origin.clone()) else {
            return;
        };
        if hit.entry.kind.is_dir_like() {
            return self.navigate_selecting(hit.path(&root), Nav::Visit, None, fx);
        }
        if hit.folder != "." {
            return self.navigate_selecting(hit.dir(&root), Nav::Visit, Some(&hit.entry.name), fx);
        }
        // In the folder on screen already: the filter closes and the file is selected there.
        self.browse_action(Action::CloseFilter, fx);
        if let Some(c) = self.conn_mut().filter(|c| c.find(&hit.entry.name).is_some()) {
            c.selection.only(&hit.entry.name);
            fx.push(Effect::Reveal(hit.entry.name));
        }
    }

    /// Download: the selected results (or the cursor's), wherever they are.
    fn download_hits(&mut self) {
        let Some(host) = self.conn().filter(|c| c.interactive()).map(|c| c.host.clone()) else { return };
        let Some((_, root)) = &self.ui.search.origin else { return };
        let items: Vec<RemoteItem> = self
            .ui
            .search
            .picked()
            .iter()
            .map(|f| RemoteItem { path: f.path(root), kind: f.entry.kind, size: f.entry.size })
            .collect();
        self.download_items(host, items);
    }

    /// The Properties sheet of the one selected result.
    pub fn hit_properties(&self) -> Option<PropertiesView> {
        let s = self.results()?;
        let (host, root) = s.origin.as_ref()?;
        let picked = s.picked();
        let [hit] = picked[..] else { return None };
        (s.selection.len() == 1).then(|| PropertiesView::new(host.clone(), hit.dir(root), hit.entry.clone()))
    }

    /// Shift+F10 or the Menu key while results are on screen: the cursor result's menu (none
    /// while a field has the keyboard, a sheet or another menu is open). `None`: no results.
    pub(super) fn hit_menu_key(&self) -> Option<Vec<Effect>> {
        let s = self.results()?;
        if self.text_focused() || self.ui.sheets.is_open() || self.files.menu.is_some() {
            return Some(Vec::new());
        }
        let key = s.selection.cursor().filter(|k| s.hit(k).is_some());
        Some(key.map(|k| Effect::ContextMenu(MenuKind::Hit(k.to_owned()))).into_iter().collect())
    }
}
