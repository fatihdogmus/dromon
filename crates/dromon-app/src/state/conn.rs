//! What one connection shows: where it is, what's there, what's selected, and how it's sorted
//! and filtered. All of it is plain data with pure updates; the view reads it and the service
//! feeds it through [`super::App::apply`].

use std::collections::HashSet;

use dromon_core::{ConnStatus, Entry, Error, HostId, RemotePath, SortDir, SortKey, merge_sorted, sort_entries};

use super::history::NavHistory;
use super::log::LogBuffer;
use super::refresh::{self, Fresh};
use super::selection::Selection;

/// The listing of the current folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// Asked for, nothing yet: skeleton rows.
    Loading,
    /// Sorted by the view's [`Sort`].
    Ready(Vec<Entry>),
    Failed(ListError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListError {
    PermissionDenied,
    NotFound,
    Other(String),
}

impl From<Error> for ListError {
    fn from(e: Error) -> Self {
        match e {
            Error::PermissionDenied(_) => ListError::PermissionDenied,
            Error::NotFound(_) => ListError::NotFound,
            other => ListError::Other(other.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sort {
    pub key: SortKey,
    pub dir: SortDir,
}

impl Sort {
    /// A header click: the same column flips; another column starts in its natural direction
    /// (names A→Z, sizes and dates largest and newest first, as Finder does).
    pub fn toggled(self, key: SortKey) -> Sort {
        if key == self.key {
            let dir = match self.dir {
                SortDir::Ascending => SortDir::Descending,
                SortDir::Descending => SortDir::Ascending,
            };
            Sort { key, dir }
        } else {
            let dir = if key == SortKey::Name { SortDir::Ascending } else { SortDir::Descending };
            Sort { key, dir }
        }
    }
}

/// The ⌘F filter over the current folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    Off,
    /// Open, matching names that contain the query (ignoring case; empty matches all).
    On(String),
}

impl Filter {
    pub fn is_on(&self) -> bool {
        matches!(self, Filter::On(_))
    }

    pub fn query(&self) -> &str {
        match self {
            Filter::Off => "",
            Filter::On(q) => q,
        }
    }

    /// Whether a name is shown: contains the query, ignoring case. The query is folded once
    /// here, not once a name, and ASCII names are searched without folding them into a copy.
    fn matcher(&self) -> impl Fn(&str) -> bool + use<> {
        let query = match self {
            Filter::On(q) if !q.is_empty() => Some(q.to_lowercase()),
            _ => None,
        };
        move |name: &str| match &query {
            None => true,
            // An ASCII name folds to ASCII, which a query with other letters can't be in.
            Some(q) if name.is_ascii() => {
                q.is_ascii() && name.as_bytes().windows(q.len()).any(|w| w.eq_ignore_ascii_case(q.as_bytes()))
            }
            Some(q) => name.to_lowercase().contains(q.as_str()),
        }
    }
}

/// How the view moves to another folder, which decides what history does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Visit,
    Back,
    Forward,
    /// Move without touching history: the remembered folder failed and home replaces it.
    Replace,
}

/// Where a paged listing of the current folder stands (`ServiceEvent::ListingPage`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Paging {
    /// Nothing streaming: the listing on screen is whole.
    #[default]
    Idle,
    /// A new folder streaming in: pages are added to the rows on screen as they come.
    Streaming,
    /// A refresh of the folder on screen: pages collect here and replace the listing at once
    /// when the last arrives, so rows and the selection don't flicker through partial lists.
    Refreshing(Vec<Entry>),
}

/// One connection's view.
#[derive(Clone, Debug)]
pub struct ConnView {
    pub host: HostId,
    pub status: ConnStatus,
    /// The session has opened at least once (so a prompt now is a reconnect's).
    pub opened: bool,
    pub log: LogBuffer,
    pub paging: Paging,
    pub cwd: RemotePath,
    pub listing: Listing,
    pub selection: Selection,
    pub history: NavHistory,
    pub sort: Sort,
    pub show_hidden: bool,
    pub filter: Filter,
    /// Indices into the `Ready` entries of the rows on screen, in order (hidden files and the
    /// filter applied). Rebuilt by [`ConnView::refresh_rows`].
    rows: Vec<u32>,
    /// Bumped whenever `rows` changes, so virtual rows skip re-reading every key otherwise.
    pub rows_version: u64,
    /// A child to select once the listing arrives (the folder you came up out of).
    reveal: Option<String>,
    /// Where to go if the folder on screen fails to list: home, while the folder is the one
    /// remembered from last time and hasn't listed yet.
    pub fallback: Option<RemotePath>,
    /// The rows the last refresh brought, highlighted as they come in (`refresh.rs`).
    pub fresh: Fresh,
    /// The folder that was on screen when it turned out to be gone, while the view climbs to
    /// the nearest folder that exists (`refresh.rs`).
    pub vanished: Option<RemotePath>,
    /// The session's home folder (where it started), once it opened: `~` in a typed path.
    pub home: Option<RemotePath>,
}

impl ConnView {
    /// A connection that's being set up: no folder yet (the service decides where it starts).
    pub fn connecting(host: HostId, show_hidden: bool, sort: Sort) -> ConnView {
        ConnView {
            host,
            status: ConnStatus::Connecting,
            opened: false,
            log: LogBuffer::default(),
            paging: Paging::Idle,
            cwd: RemotePath::root(),
            listing: Listing::Loading,
            selection: Selection::default(),
            history: NavHistory::default(),
            sort,
            show_hidden,
            filter: Filter::Off,
            rows: Vec::new(),
            rows_version: 0,
            reveal: None,
            fallback: None,
            fresh: Fresh::default(),
            vanished: None,
            home: None,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        match &self.listing {
            Listing::Ready(e) => e,
            _ => &[],
        }
    }

    /// The rows on screen.
    pub fn rows(&self) -> impl ExactSizeIterator<Item = &Entry> + '_ {
        let entries = self.entries();
        self.rows.iter().map(move |&i| &entries[i as usize])
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Runs `f` on the selection with the names of the rows on screen, borrowed (a 100k-row
    /// folder's names aren't copied for every key press).
    pub fn with_row_names<R>(&mut self, f: impl FnOnce(&mut Selection, &[&str]) -> R) -> R {
        let entries: &[Entry] = match &self.listing {
            Listing::Ready(e) => e,
            _ => &[],
        };
        let names: Vec<&str> = self.rows.iter().map(|&i| entries[i as usize].name.as_str()).collect();
        f(&mut self.selection, &names)
    }

    /// Row `i` on screen.
    pub fn row(&self, i: usize) -> &Entry {
        &self.entries()[self.rows[i] as usize]
    }

    /// The names of the rows on screen, in order.
    pub fn row_names(&self) -> Vec<&str> {
        self.rows().map(|e| e.name.as_str()).collect()
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries().iter().find(|e| e.name == name)
    }

    /// The entries in the folder, hidden ones included when they're shown, before the filter.
    pub fn total(&self) -> usize {
        self.entries().iter().filter(|e| self.show_hidden || !e.hidden()).count()
    }

    /// Whether the listing is live and takes actions (not mid-reconnect).
    pub fn interactive(&self) -> bool {
        matches!(self.status, ConnStatus::Ready)
    }

    /// Moves to `path`, returning true when that needs a listing. History follows `how`.
    pub fn go(&mut self, path: RemotePath, how: Nav) -> bool {
        if path == self.cwd && !matches!(self.listing, Listing::Loading) {
            return false;
        }
        let from = std::mem::replace(&mut self.cwd, path);
        if how == Nav::Visit {
            self.history.visit(from.clone());
        }
        // Coming up out of a folder selects it, as Finder does.
        self.reveal = if how != Nav::Replace && from.starts_with(&self.cwd) && from != self.cwd {
            from.components().nth(self.cwd.components().count()).map(str::to_owned)
        } else {
            None
        };
        self.listing = Listing::Loading;
        self.paging = Paging::Idle;
        self.selection.clear();
        self.filter = Filter::Off;
        // A new folder never animates in, and a climb out of a vanished one ends here.
        self.fresh = Fresh { seq: self.fresh.seq, ..Fresh::default() };
        self.vanished = None;
        self.refresh_rows();
        true
    }

    /// Selects `name` once the listing in flight arrives (an uploaded file, shown from the
    /// transfers popover).
    pub fn reveal_after(&mut self, name: &str) {
        self.reveal = Some(name.to_owned());
    }

    /// Back: true when there was somewhere to go (and it needs a listing).
    pub fn back(&mut self) -> bool {
        match self.history.back(self.cwd.clone()) {
            Some(to) => self.go(to, Nav::Back),
            None => false,
        }
    }

    pub fn forward(&mut self) -> bool {
        match self.history.forward(self.cwd.clone()) {
            Some(to) => self.go(to, Nav::Forward),
            None => false,
        }
    }

    /// A whole listing for `path` arrived: a `Listing` event, or a poll's `Refreshed`. Ignored
    /// unless it's the folder on screen (a late answer for a folder already left). Over rows
    /// already listed it's a refresh ([`ConnView::refresh_with`]). Returns whether rows changed.
    pub fn set_listing(&mut self, path: &RemotePath, entries: Vec<Entry>) -> bool {
        if *path != self.cwd {
            return false;
        }
        self.paging = Paging::Idle;
        if matches!(self.listing, Listing::Ready(_)) {
            return self.refresh_with(entries);
        }
        self.show(entries, true);
        true
    }

    /// A new listing of the folder on screen replaces the one shown, changing only what
    /// changed (§4.5): an empty diff changes nothing at all; otherwise the inserted rows become
    /// [`Fresh`], selected rows that left leave the selection quietly, and a cursor whose row
    /// left moves to its neighbour (no scrolling: names, not positions, hold the place).
    /// Returns whether rows changed.
    fn refresh_with(&mut self, mut entries: Vec<Entry>) -> bool {
        let changes = dromon_sftp::diff(self.entries(), &entries);
        if changes.is_empty() {
            self.take_reveal(true);
            return false;
        }
        sort_entries(&mut entries, self.sort.key, self.sort.dir);
        let cursor = self.selection.cursor().map(str::to_owned);
        // The rows as they were, kept (not their names copied) for a cursor whose row leaves.
        let before_rows = std::mem::take(&mut self.rows);
        let before = std::mem::replace(&mut self.listing, Listing::Ready(entries));
        self.refresh_rows();
        if let Some(gone) = cursor.filter(|c| self.selection.cursor() != Some(c.as_str())) {
            let next = {
                let old = match &before {
                    Listing::Ready(e) => e.as_slice(),
                    _ => &[],
                };
                let before: Vec<&str> = before_rows.iter().map(|&i| old[i as usize].name.as_str()).collect();
                let now: HashSet<&str> = self.rows().map(|e| e.name.as_str()).collect();
                refresh::neighbour(&before, &gone, |n| now.contains(n)).map(str::to_owned)
            };
            if let Some(next) = next {
                self.selection.set_cursor(&next);
            }
        }
        self.fresh = self.fresh.next(changes.inserted);
        self.take_reveal(true);
        true
    }

    /// One page of a streamed listing of `path` arrived (see [`Paging`]). A first page of a
    /// new folder shows at once and later pages join it; a refresh of the folder on screen
    /// collects its pages and swaps them in with the last. Ignored unless `path` is on screen.
    /// Returns whether the rows changed.
    pub fn apply_page(&mut self, path: &RemotePath, entries: Vec<Entry>, first: bool, done: bool) -> bool {
        if *path != self.cwd {
            return false;
        }
        let refresh = matches!(self.listing, Listing::Ready(_));
        match (std::mem::take(&mut self.paging), first) {
            (_, true) if refresh && !done => {
                self.paging = Paging::Refreshing(entries);
                false
            }
            (_, true) if refresh => self.refresh_with(entries),
            (_, true) => {
                self.paging = if done { Paging::Idle } else { Paging::Streaming };
                self.show(entries, done);
                true
            }
            (Paging::Streaming, false) => {
                let Listing::Ready(have) = &mut self.listing else { return false };
                // The page joins the sorted rows in place (not a sort of the whole again: in a
                // 100,000-entry folder that was 70 ms a page); rows only come, none leave, so the
                // selection stays as it is.
                merge_sorted(have, entries, self.sort.key, self.sort.dir);
                self.paging = if done { Paging::Idle } else { Paging::Streaming };
                self.rebuild_rows();
                self.take_reveal(done);
                true
            }
            (Paging::Refreshing(mut have), false) => {
                have.extend(entries);
                if done {
                    self.refresh_with(have)
                } else {
                    self.paging = Paging::Refreshing(have);
                    false
                }
            }
            // A later page with no listing under way (its first was for a folder since left).
            (Paging::Idle, false) => false,
        }
    }

    /// Shows `entries` as the listing; `whole` when nothing more is coming, which is when a
    /// folder to reveal that isn't there gives up.
    fn show(&mut self, mut entries: Vec<Entry>, whole: bool) {
        sort_entries(&mut entries, self.sort.key, self.sort.dir);
        self.listing = Listing::Ready(entries);
        self.refresh_rows();
        self.take_reveal(whole);
    }

    /// Selects the row waiting to be revealed once it's on screen; `whole` when nothing more
    /// is coming, which is when one that isn't there gives up.
    fn take_reveal(&mut self, whole: bool) {
        if let Some(name) = self.reveal.take() {
            if self.rows().any(|e| e.name == name) {
                self.selection.only(&name);
            } else if !whole {
                self.reveal = Some(name);
            }
        }
    }

    /// Whether more of the folder on screen is still streaming in.
    pub fn loading_more(&self) -> bool {
        self.paging == Paging::Streaming
    }

    pub fn set_failed(&mut self, path: &RemotePath, error: ListError) -> bool {
        if *path != self.cwd {
            return false;
        }
        self.paging = Paging::Idle;
        self.listing = Listing::Failed(error);
        self.reveal = None;
        self.refresh_rows();
        true
    }

    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        if let Listing::Ready(entries) = &mut self.listing {
            sort_entries(entries, sort.key, sort.dir);
        }
        self.refresh_rows();
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refresh_rows();
    }

    pub fn set_filter(&mut self, filter: Filter) {
        if filter != self.filter {
            self.filter = filter;
            self.refresh_rows();
        }
    }

    /// Recomputes the rows on screen and drops selected names that left.
    pub fn refresh_rows(&mut self) {
        self.rebuild_rows();
        let entries = match &self.listing {
            Listing::Ready(e) => e.as_slice(),
            _ => &[],
        };
        self.selection.retain_rows(self.rows.iter().map(|&i| entries[i as usize].name.as_str()));
    }

    /// Recomputes the rows on screen only: for when no row can have left (a page added rows).
    fn rebuild_rows(&mut self) {
        let shown = self.filter.matcher();
        let show_hidden = self.show_hidden;
        let entries = match &self.listing {
            Listing::Ready(e) => e.as_slice(),
            _ => &[],
        };
        self.rows.clear();
        self.rows.extend(
            entries
                .iter()
                .enumerate()
                .filter(|(_, e)| (show_hidden || !e.hidden()) && shown(&e.name))
                .map(|(i, _)| i as u32),
        );
        self.rows_version = self.rows_version.wrapping_add(1);
    }

    /// Total bytes of the selected files (folders count nothing). The view asks on every
    /// rebuild while something is selected, so a few selected names are compared rather than
    /// every entry's name hashed (12 ms in a 100,000-entry folder).
    pub fn selected_bytes(&self) -> u64 {
        let files = self.entries().iter().filter(|e| !e.kind.is_dir_like());
        match self.selection.len() {
            0 => 0,
            n if n <= 16 => {
                let selected: Vec<&str> = self.selection.iter().collect();
                files.filter(|e| selected.contains(&e.name.as_str())).map(|e| e.size).sum()
            }
            _ => files.filter(|e| self.selection.contains(&e.name)).map(|e| e.size).sum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dromon_core::{EntryKind, LinkTarget};

    fn e(name: &str, kind: EntryKind, size: u64) -> Entry {
        Entry { name: name.into(), kind, size, mtime: Some(size as i64), mode: None, owner: None, group: None }
    }

    fn ready(path: &str) -> ConnView {
        let mut c = ConnView::connecting(HostId("h".into()), true, Sort::default());
        c.status = ConnStatus::Ready;
        c.go(RemotePath::new(path), Nav::Visit);
        let link = EntryKind::Symlink { target: Some(LinkTarget::Dir) };
        let entries = vec![
            e("b.txt", EntryKind::File, 300),
            e(".env", EntryKind::File, 10),
            e("src", EntryKind::Dir, 0),
            e("a.txt", EntryKind::File, 2000),
            e("current", link, 0),
        ];
        c.set_listing(&RemotePath::new(path), entries);
        c
    }

    #[test]
    fn listings_sort_folders_first() {
        let c = ready("/w");
        assert_eq!(c.row_names(), ["current", "src", ".env", "a.txt", "b.txt"]);
    }

    #[test]
    fn sort_toggles_like_finder() {
        let s = Sort::default();
        assert_eq!(s.toggled(SortKey::Name), Sort { key: SortKey::Name, dir: SortDir::Descending });
        let size = s.toggled(SortKey::Size);
        assert_eq!(size, Sort { key: SortKey::Size, dir: SortDir::Descending });
        assert_eq!(size.toggled(SortKey::Size).dir, SortDir::Ascending);
        let mut c = ready("/w");
        c.set_sort(c.sort.toggled(SortKey::Size));
        assert_eq!(c.row_names(), ["src", "current", "a.txt", "b.txt", ".env"]);
    }

    #[test]
    fn hidden_files_toggle_and_leave_the_selection() {
        let mut c = ready("/w");
        c.selection.only(".env");
        c.toggle_hidden();
        assert_eq!(c.row_names(), ["current", "src", "a.txt", "b.txt"]);
        assert!(c.selection.is_empty());
        assert_eq!(c.total(), 4);
        c.toggle_hidden();
        assert_eq!(c.row_count(), 5);
    }

    #[test]
    fn filter_matches_substrings_ignoring_case() {
        let mut c = ready("/w");
        c.set_filter(Filter::On("TXT".into()));
        assert_eq!(c.row_names(), ["a.txt", "b.txt"]);
        c.set_filter(Filter::On(String::new()));
        assert_eq!(c.row_count(), 5);
        c.set_filter(Filter::Off);
        assert_eq!(c.row_count(), 5);
    }

    #[test]
    fn filter_folds_non_ascii_names_and_queries() {
        let mut c = ready("/w");
        let w = RemotePath::new("/w");
        let names = ["Ärger.txt", "ärger-2.txt", "arger.txt", "STRASSE.md", "Straße.md", "x"];
        c.set_listing(&w, names.iter().map(|n| e(n, EntryKind::File, 1)).collect());
        let shown = |c: &mut ConnView, q: &str| {
            c.set_filter(Filter::On(q.into()));
            c.row_names().into_iter().map(str::to_owned).collect::<Vec<_>>()
        };
        assert_eq!(shown(&mut c, "ÄRG"), ["ärger-2.txt", "Ärger.txt"]);
        assert_eq!(shown(&mut c, "arg"), ["arger.txt"]);
        assert_eq!(shown(&mut c, "straße"), ["Straße.md"]);
        assert_eq!(shown(&mut c, "sse"), ["STRASSE.md"]);
        assert_eq!(shown(&mut c, "a-very-long-query-longer-than-names"), Vec::<String>::new());
    }

    #[test]
    fn selected_bytes_with_many_selected() {
        let mut c = ready("/w");
        let w = RemotePath::new("/w");
        c.set_listing(&w, (0..40).map(|i| e(&format!("f{i}"), EntryKind::File, i)).collect());
        let rows: Vec<String> = c.row_names().into_iter().map(str::to_owned).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        c.selection.select_all(&rows);
        assert_eq!(c.selected_bytes(), (0..40).sum::<u64>());
    }

    #[test]
    fn late_listings_for_other_folders_are_ignored() {
        let mut c = ready("/w");
        assert!(!c.set_listing(&RemotePath::new("/elsewhere"), vec![]));
        assert_eq!(c.row_count(), 5);
    }

    #[test]
    fn going_up_selects_the_folder_you_left() {
        let mut c = ready("/w/src/deep");
        assert!(c.go(RemotePath::new("/w"), Nav::Visit));
        assert_eq!(c.listing, Listing::Loading);
        let entries = vec![e("src", EntryKind::Dir, 0), e("x", EntryKind::File, 1)];
        c.set_listing(&RemotePath::new("/w"), entries);
        assert_eq!(c.selection.in_order(&c.row_names()), ["src"]);
    }

    #[test]
    fn navigation_clears_selection_and_filter_and_records_history() {
        let mut c = ready("/w");
        c.selection.only("a.txt");
        c.set_filter(Filter::On("a".into()));
        assert!(c.go(RemotePath::new("/w/src"), Nav::Visit));
        assert!(c.selection.is_empty());
        assert_eq!(c.filter, Filter::Off);
        assert!(c.history.can_back());
        assert!(c.back());
        assert_eq!(c.cwd, RemotePath::new("/w"));
        assert!(c.history.can_forward());
        assert!(c.forward());
        assert_eq!(c.cwd, RemotePath::new("/w/src"));
    }

    #[test]
    fn selected_bytes_counts_files_only() {
        let mut c = ready("/w");
        let rows = ["src", "a.txt", "b.txt"];
        c.selection.only("src");
        c.selection.extend_to(&rows, "b.txt", false);
        assert_eq!(c.selected_bytes(), 2300);
    }

    #[test]
    fn pages_of_a_new_folder_show_as_they_arrive() {
        let mut c = ready("/w");
        assert!(c.go(RemotePath::new("/w/src"), Nav::Visit));
        let src = RemotePath::new("/w/src");
        assert!(c.apply_page(&src, vec![e("m", EntryKind::File, 1), e("z", EntryKind::File, 2)], true, false));
        assert!(c.loading_more());
        c.selection.only("z");
        assert!(c.apply_page(&src, vec![e("a", EntryKind::File, 3), e("lib", EntryKind::Dir, 0)], false, false));
        assert_eq!(c.row_names(), ["lib", "a", "m", "z"]);
        assert!(c.selection.contains("z"), "the selection survives a page");
        assert!(c.apply_page(&src, vec![], false, true));
        assert!(!c.loading_more());
        assert_eq!(c.row_count(), 4);
        // Late pages, and pages for other folders, change nothing.
        assert!(!c.apply_page(&src, vec![e("late", EntryKind::File, 1)], false, true));
        assert!(!c.apply_page(&RemotePath::new("/w"), vec![], true, true));
        assert_eq!(c.row_count(), 4);
    }

    #[test]
    fn a_refresh_swaps_in_whole() {
        let mut c = ready("/w");
        c.selection.only("b.txt");
        let w = RemotePath::new("/w");
        assert!(!c.apply_page(&w, vec![e("a.txt", EntryKind::File, 1)], true, false));
        assert_eq!(c.row_count(), 5, "the old listing stays until the refresh is whole");
        assert!(c.selection.contains("b.txt"));
        assert!(c.apply_page(&w, vec![e("b.txt", EntryKind::File, 2), e("new", EntryKind::File, 3)], false, true));
        assert_eq!(c.row_names(), ["a.txt", "b.txt", "new"]);
        assert!(c.selection.contains("b.txt"));
    }

    #[test]
    fn a_folder_to_reveal_may_come_in_a_later_page() {
        let mut c = ready("/w/src/deep");
        let w = RemotePath::new("/w");
        assert!(c.go(w.clone(), Nav::Visit));
        c.apply_page(&w, vec![e("x", EntryKind::File, 1)], true, false);
        assert!(c.selection.is_empty());
        c.apply_page(&w, vec![e("src", EntryKind::Dir, 0)], false, true);
        assert_eq!(c.selection.in_order(&c.row_names()), ["src"]);
    }

    #[test]
    fn permission_errors_map_to_their_state() {
        assert_eq!(ListError::from(Error::PermissionDenied(RemotePath::root())), ListError::PermissionDenied);
        assert_eq!(ListError::from(Error::NotFound(RemotePath::root())), ListError::NotFound);
    }
}
