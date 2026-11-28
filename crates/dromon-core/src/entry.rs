use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::natural::natural_cmp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Dir,
    /// A symlink, with the kind of what it points at when that could be resolved
    /// (`None` for a dangling link or one we couldn't stat).
    Symlink {
        target: Option<LinkTarget>,
    },
    /// Sockets, FIFOs, devices.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LinkTarget {
    File,
    Dir,
    Other,
}

impl EntryKind {
    /// True for folders and for symlinks that resolve to a folder: things you navigate into.
    pub fn is_dir_like(self) -> bool {
        matches!(self, EntryKind::Dir | EntryKind::Symlink { target: Some(LinkTarget::Dir) })
    }

    pub fn is_symlink(self) -> bool {
        matches!(self, EntryKind::Symlink { .. })
    }
}

/// One row of a remote listing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Modification time, seconds since the Unix epoch.
    pub mtime: Option<i64>,
    /// Permission bits as reported by the server (`st_mode`), if any.
    pub mode: Option<u32>,
    /// The owning user: its name from the server's `ls -l` line, else the numeric uid; `None`
    /// when the server sent neither. Not part of [`Entry::fingerprint`].
    #[serde(default)]
    pub owner: Option<String>,
    /// The owning group, as [`Entry::owner`] (name, else numeric gid).
    #[serde(default)]
    pub group: Option<String>,
}

impl Entry {
    /// Dotfiles are hidden (shown dimmed by default).
    pub fn hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    /// The fields a poll compares to decide whether a row changed.
    pub fn fingerprint(&self) -> (EntryKind, u64, Option<i64>) {
        (self.kind, self.size, self.mtime)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SortDir {
    #[default]
    Ascending,
    Descending,
}

/// Sorts folders first (always, in either direction), then by `key` in `dir`, with natural name
/// order breaking ties so the order is stable and total. Folders show no size, so a size sort
/// orders them by name.
pub fn sort_entries(entries: &mut [Entry], key: SortKey, dir: SortDir) {
    entries.sort_by(|a, b| entry_cmp(a, b, key, dir));
}

/// The order [`sort_entries`] sorts by.
fn entry_cmp(a: &Entry, b: &Entry, key: SortKey, dir: SortDir) -> Ordering {
    let folders = b.kind.is_dir_like().cmp(&a.kind.is_dir_like());
    if folders != Ordering::Equal {
        return folders;
    }
    let by_key = match key {
        SortKey::Name => Ordering::Equal,
        SortKey::Size if a.kind.is_dir_like() => Ordering::Equal,
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.mtime.cmp(&b.mtime),
    };
    let ord = by_key.then_with(|| natural_cmp(&a.name, &b.name));
    match dir {
        SortDir::Ascending => ord,
        SortDir::Descending => ord.reverse(),
    }
}

/// Adds `more` to `sorted`, which is in [`sort_entries`] order, keeping that order: as
/// `extend` then `sort_entries`, in time for sorting `more` and finding where each of its
/// entries goes (binary searches), plus one pass that moves the entries. A page of a streaming
/// listing joins the rows already on screen this way.
pub fn merge_sorted(sorted: &mut Vec<Entry>, mut more: Vec<Entry>, key: SortKey, dir: SortDir) {
    sort_entries(&mut more, key, dir);
    let cmp = |a: &Entry, b: &Entry| entry_cmp(a, b, key, dir);
    let Some(first) = more.first() else { return };
    if sorted.last().is_none_or(|last| cmp(last, first) != Ordering::Greater) {
        sorted.extend(more);
        return;
    }
    let old = std::mem::take(sorted);
    // Where each of `more` goes: after the old entries that don't sort after it (as a stable
    // sort of `old` then `more` puts them). Non-decreasing, since `more` is sorted.
    let at: Vec<usize> = more.iter().map(|m| old.partition_point(|e| cmp(e, m) != Ordering::Greater)).collect();
    sorted.reserve(old.len() + more.len());
    let mut old = old.into_iter();
    let mut taken = 0;
    for (m, at) in more.into_iter().zip(at) {
        sorted.extend(old.by_ref().take(at - taken));
        taken = at;
        sorted.push(m);
    }
    sorted.extend(old);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, kind: EntryKind, size: u64, mtime: i64) -> Entry {
        Entry { name: name.into(), kind, size, mtime: Some(mtime), mode: None, owner: None, group: None }
    }

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn folders_first_in_both_directions() {
        let link_dir = EntryKind::Symlink { target: Some(LinkTarget::Dir) };
        let mut v = vec![
            e("b.txt", EntryKind::File, 10, 3),
            e("src", EntryKind::Dir, 0, 1),
            e("a.txt", EntryKind::File, 20, 2),
            e("lib", link_dir, 0, 4),
        ];
        sort_entries(&mut v, SortKey::Name, SortDir::Ascending);
        assert_eq!(names(&v), ["lib", "src", "a.txt", "b.txt"]);
        sort_entries(&mut v, SortKey::Name, SortDir::Descending);
        assert_eq!(names(&v), ["src", "lib", "b.txt", "a.txt"]);
    }

    #[test]
    fn by_size_and_date_with_name_ties() {
        let mut v = vec![e("c", EntryKind::File, 5, 9), e("a", EntryKind::File, 5, 1), e("b", EntryKind::File, 1, 5)];
        sort_entries(&mut v, SortKey::Size, SortDir::Ascending);
        assert_eq!(names(&v), ["b", "a", "c"]);
        sort_entries(&mut v, SortKey::Modified, SortDir::Descending);
        assert_eq!(names(&v), ["c", "b", "a"]);
    }

    #[test]
    fn size_sort_orders_folders_by_name() {
        let mut v = vec![e("b", EntryKind::Dir, 4096, 1), e("a", EntryKind::Dir, 30, 1), e("f", EntryKind::File, 7, 1)];
        sort_entries(&mut v, SortKey::Size, SortDir::Ascending);
        assert_eq!(names(&v), ["a", "b", "f"]);
    }

    #[test]
    fn merging_pages_matches_sorting_the_whole() {
        let mut seed = 7u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            seed >> 33
        };
        // Equal sizes and dates, folders, mixed case and numbers: ties break on names.
        let all: Vec<Entry> = (0..600)
            .map(|i| {
                let r = next();
                let kind = if r % 7 == 0 { EntryKind::Dir } else { EntryKind::File };
                e(&format!("{}{}", ["a", "B", "file", "File", "x_"][(r % 5) as usize], i), kind, r % 4, (r % 3) as i64)
            })
            .collect();
        for key in [SortKey::Name, SortKey::Size, SortKey::Modified] {
            for dir in [SortDir::Ascending, SortDir::Descending] {
                let mut want = all.clone();
                sort_entries(&mut want, key, dir);
                for page in [1, 7, 64, 600] {
                    let mut got = Vec::new();
                    for chunk in all.chunks(page) {
                        merge_sorted(&mut got, chunk.to_vec(), key, dir);
                    }
                    assert_eq!(names(&got), names(&want), "{key:?} {dir:?} pages of {page}");
                }
                // Pages that all go after what's there take the shortcut.
                let mut got = want[..300].to_vec();
                merge_sorted(&mut got, want[300..].to_vec(), key, dir);
                assert_eq!(names(&got), names(&want));
            }
        }
    }

    #[test]
    fn owner_and_group_are_not_part_of_the_fingerprint() {
        let a = e("f", EntryKind::File, 1, 1);
        let b = Entry { owner: Some("alice".into()), group: Some("staff".into()), ..a.clone() };
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn hidden_is_dotfiles() {
        assert!(e(".env", EntryKind::File, 0, 0).hidden());
        assert!(!e("env", EntryKind::File, 0, 0).hidden());
    }
}

#[cfg(test)]
#[path = "entry_props.rs"]
mod props;
