use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::natural::natural_cmp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Dir,
    /// A symlink, with the kind of what it points at when that could be resolved
    /// (`None` for a dangling link or one we couldn't stat).
    Symlink { target: Option<LinkTarget> },
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
    entries.sort_by(|a, b| {
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
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, kind: EntryKind, size: u64, mtime: i64) -> Entry {
        Entry { name: name.into(), kind, size, mtime: Some(mtime), mode: None }
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
        let mut v = vec![
            e("c", EntryKind::File, 5, 9),
            e("a", EntryKind::File, 5, 1),
            e("b", EntryKind::File, 1, 5),
        ];
        sort_entries(&mut v, SortKey::Size, SortDir::Ascending);
        assert_eq!(names(&v), ["b", "a", "c"]);
        sort_entries(&mut v, SortKey::Modified, SortDir::Descending);
        assert_eq!(names(&v), ["c", "b", "a"]);
    }

    #[test]
    fn size_sort_orders_folders_by_name() {
        let mut v = vec![
            e("b", EntryKind::Dir, 4096, 1),
            e("a", EntryKind::Dir, 30, 1),
            e("f", EntryKind::File, 7, 1),
        ];
        sort_entries(&mut v, SortKey::Size, SortDir::Ascending);
        assert_eq!(names(&v), ["a", "b", "f"]);
    }

    #[test]
    fn hidden_is_dotfiles() {
        assert!(e(".env", EntryKind::File, 0, 0).hidden());
        assert!(!e("env", EntryKind::File, 0, 0).hidden());
    }
}
