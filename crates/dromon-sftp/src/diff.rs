//! Listing diffs: what a poll changed, for the insert/remove/update animations (plan §4.4).

use std::collections::HashMap;

use dromon_core::Entry;

/// Names inserted, removed and changed between two listings of the same folder, each sorted
/// (byte order) for determinism.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListingDiff {
    pub inserted: Vec<String>,
    pub removed: Vec<String>,
    /// Present in both with a different [`Entry::fingerprint`] (kind, size or mtime).
    pub changed: Vec<String>,
}

impl ListingDiff {
    pub fn is_empty(&self) -> bool {
        self.inserted.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Compares by name, then by fingerprint. O(n) hashing plus sorting the (usually tiny) output.
/// If a listing repeats a name, its last occurrence wins.
///
/// One map (old names to their place) and one pass over `new`: what of `old` it didn't meet is
/// what left. A 100,000-entry refresh takes about a third of the time two maps took.
pub fn diff(old: &[Entry], new: &[Entry]) -> ListingDiff {
    // Collecting keeps the last occurrence of a repeated name.
    let before: HashMap<&str, usize> = old.iter().enumerate().map(|(i, e)| (e.name.as_str(), i)).collect();
    let mut met = vec![false; old.len()];

    let mut out = ListingDiff::default();
    // From the end, so the last occurrence of a name repeated in `new` is the one compared.
    for entry in new.iter().rev() {
        match before.get(entry.name.as_str()) {
            None => out.inserted.push(entry.name.clone()),
            Some(&i) if met[i] => {}
            Some(&i) => {
                met[i] = true;
                if old[i].fingerprint() != entry.fingerprint() {
                    out.changed.push(entry.name.clone());
                }
            }
        }
    }
    // Unmet, and the occurrence the map kept (a repeated old name is reported once).
    out.removed = (0..old.len())
        .filter(|&i| !met[i] && before.get(old[i].name.as_str()) == Some(&i))
        .map(|i| old[i].name.clone())
        .collect();
    out.inserted.sort_unstable();
    out.inserted.dedup();
    out.removed.sort_unstable();
    out.changed.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use dromon_core::{EntryKind, LinkTarget};

    use super::*;

    fn e(name: &str, size: u64, mtime: i64) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size,
            mtime: Some(mtime),
            mode: Some(0o644),
            owner: None,
            group: None,
        }
    }

    #[test]
    fn identical_listings_are_empty() {
        let a = vec![e("a", 1, 1), e("b", 2, 2)];
        let mut b = a.clone();
        b.reverse();
        assert!(diff(&a, &b).is_empty());
        assert!(diff(&[], &[]).is_empty());
    }

    #[test]
    fn inserted_removed_changed_sorted() {
        let old = vec![e("keep", 1, 1), e("gone", 1, 1), e("grew", 1, 1), e("touched", 1, 1)];
        let new = vec![e("zeta", 0, 0), e("touched", 1, 9), e("keep", 1, 1), e("grew", 5, 1), e("alpha", 0, 0)];
        let d = diff(&old, &new);
        assert_eq!(d.inserted, ["alpha", "zeta"]);
        assert_eq!(d.removed, ["gone"]);
        assert_eq!(d.changed, ["grew", "touched"]);
        assert!(!d.is_empty());
    }

    #[test]
    fn kind_change_counts_and_mode_does_not() {
        let mut dir = e("x", 1, 1);
        dir.kind = EntryKind::Dir;
        assert_eq!(diff(&[e("x", 1, 1)], &[dir]).changed, ["x"]);

        let mut link = e("l", 1, 1);
        link.kind = EntryKind::Symlink { target: None };
        let mut resolved = link.clone();
        resolved.kind = EntryKind::Symlink { target: Some(LinkTarget::Dir) };
        assert_eq!(diff(&[link], &[resolved]).changed, ["l"]);

        let mut chmod = e("m", 1, 1);
        chmod.mode = Some(0o600);
        assert!(diff(&[e("m", 1, 1)], &[chmod]).is_empty());
    }

    #[test]
    fn repeated_names_count_once_and_the_last_wins() {
        let old = vec![e("dup", 1, 1), e("dup", 2, 2), e("gone", 1, 1), e("gone", 1, 1)];
        let new = vec![e("dup", 9, 9), e("dup", 2, 2), e("new", 1, 1), e("new", 1, 1)];
        let d = diff(&old, &new);
        assert_eq!(d.inserted, ["new"]);
        assert_eq!(d.removed, ["gone"]);
        assert!(d.changed.is_empty(), "the last old dup (2, 2) against the last new one (2, 2)");
        assert_eq!(diff(&old, &[e("dup", 2, 2), e("dup", 9, 9)]).changed, ["dup"]);
    }

    #[test]
    fn missing_mtime_is_a_difference() {
        let mut none = e("t", 1, 1);
        none.mtime = None;
        assert_eq!(diff(&[e("t", 1, 1)], &[none]).changed, ["t"]);
    }

    #[test]
    fn hundred_thousand_entries() {
        let old: Vec<Entry> = (0..100_000).map(|i| e(&format!("file-{i:06}"), i, 1)).collect();
        let mut new = old.clone();
        new.retain(|x| x.size % 1000 != 0); // 100 removed
        for x in new.iter_mut().filter(|x| x.size % 1000 == 1) {
            x.mtime = Some(2); // 100 changed
        }
        new.extend((0..100).map(|i| e(&format!("new-{i:03}"), 0, 0))); // 100 inserted
        new.reverse();

        let start = Instant::now();
        let d = diff(&old, &new);
        let took = start.elapsed();
        assert_eq!((d.inserted.len(), d.removed.len(), d.changed.len()), (100, 100, 100));
        assert_eq!(d.removed[0], "file-000000");
        assert_eq!(d.changed[0], "file-000001");
        assert_eq!(d.inserted[0], "new-000");
        // Not a benchmark: a generous bound that only catches accidental O(n²).
        assert!(took.as_secs() < 5, "100k diff took {took:?}");
    }
}

#[cfg(test)]
#[path = "diff_props.rs"]
mod props;
