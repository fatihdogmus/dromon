//! What one connection shows: where it is, what's there, what's selected, and how it's sorted
//! and filtered. All of it is plain data with pure updates; the view reads it and the service
//! feeds it through [`super::App::apply`].

use std::collections::HashSet;

use dromon_core::{ConnStatus, Entry, Error, HostId, RemotePath, SortDir, SortKey, sort_entries};

use super::history::NavHistory;
use super::log::LogBuffer;
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

    fn matches(&self, name: &str) -> bool {
        match self {
            Filter::Off => true,
            Filter::On(q) if q.is_empty() => true,
            Filter::On(q) => name.to_lowercase().contains(&q.to_lowercase()),
        }
    }
}

/// How the view moves to another folder, which decides what history does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Visit,
    Back,
    Forward,
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
        self.reveal = if from.starts_with(&self.cwd) && from != self.cwd {
            from.components().nth(self.cwd.components().count()).map(str::to_owned)
        } else {
            None
        };
        self.listing = Listing::Loading;
        self.paging = Paging::Idle;
        self.selection.clear();
        self.filter = Filter::Off;
        self.refresh_rows();
        true
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

    /// A listing for `path` arrived. Ignored unless it's the folder on screen (a late answer
    /// for a folder already left). The selection survives a refresh where its names remain.
    pub fn set_listing(&mut self, path: &RemotePath, entries: Vec<Entry>) -> bool {
        if *path != self.cwd {
            return false;
        }
        self.paging = Paging::Idle;
        self.show(entries, true);
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
            (_, true) => {
                self.paging = if done { Paging::Idle } else { Paging::Streaming };
                self.show(entries, done);
                true
            }
            (Paging::Streaming, false) => {
                let mut all = std::mem::replace(&mut self.listing, Listing::Loading);
                if let Listing::Ready(have) = &mut all {
                    have.extend(entries);
                }
                let Listing::Ready(all) = all else { return false };
                self.paging = if done { Paging::Idle } else { Paging::Streaming };
                self.show(all, done);
                true
            }
            (Paging::Refreshing(mut have), false) => {
                have.extend(entries);
                if done {
                    self.show(have, true);
                    true
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
        let rows: Vec<u32> = self
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| (self.show_hidden || !e.hidden()) && self.filter.matches(&e.name))
            .map(|(i, _)| i as u32)
            .collect();
        self.rows = rows;
        self.rows_version = self.rows_version.wrapping_add(1);
        let names: Vec<String> = self.row_names().into_iter().map(str::to_owned).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        self.selection.retain(&names);
    }

    /// Total bytes of the selected files (folders count nothing).
    pub fn selected_bytes(&self) -> u64 {
        let selected: HashSet<&str> = self.selection.iter().collect();
        self.entries()
            .iter()
            .filter(|e| selected.contains(e.name.as_str()) && !e.kind.is_dir_like())
            .map(|e| e.size)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dromon_core::{EntryKind, LinkTarget};

    fn e(name: &str, kind: EntryKind, size: u64) -> Entry {
        Entry { name: name.into(), kind, size, mtime: Some(size as i64), mode: None }
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
