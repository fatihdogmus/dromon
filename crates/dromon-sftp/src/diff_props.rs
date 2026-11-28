//! Properties of listing diffs: applying the diff to the old listing gives the new one, by
//! name and by what a poll compares; a listing against itself (in any order) is no change;
//! nothing is both inserted and removed; each list is sorted without repeats; and a repeated
//! name counts as its last occurrence.

use std::collections::{BTreeMap, BTreeSet};

use dromon_core::{Entry, EntryKind, LinkTarget};
use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::{ListingDiff, diff};

/// A row from a small set of names and values, so that two listings share names and often
/// differ on them; sometimes any name.
#[hegel::composite]
fn entries(tc: &TestCase) -> Entry {
    let kinds = [EntryKind::File, EntryKind::Dir, EntryKind::Symlink { target: Some(LinkTarget::Dir) }];
    Entry {
        name: tc.draw(hegel::one_of!(
            gs::sampled_from(["a", "b", "c", "A", "é", "日本", ".x", ""].as_slice()).map(str::to_owned),
            gs::text().max_size(6),
        )),
        kind: tc.draw(gs::sampled_from(kinds.as_slice())),
        size: tc.draw(hegel::one_of!(gs::integers::<u64>().max_value(2), gs::just(u64::MAX))),
        mtime: tc.draw(gs::optional(gs::integers::<i64>().min_value(0).max_value(2))),
        mode: tc.draw(gs::optional(gs::sampled_from([0o644, 0o600].as_slice()))),
        owner: None,
        group: None,
    }
}

/// The listing as a server would send it once a repeated name is dropped (the first stays).
fn unique(rows: Vec<Entry>) -> Vec<Entry> {
    let mut seen = BTreeSet::new();
    rows.into_iter().filter(|e| seen.insert(e.name.clone())).collect()
}

/// A repeated name counts as its last occurrence.
fn last_wins(rows: &[Entry]) -> Vec<Entry> {
    let by_name: BTreeMap<&str, &Entry> = rows.iter().map(|e| (e.name.as_str(), e)).collect();
    by_name.into_values().cloned().collect()
}

/// Every name with what a poll compares of it.
fn by_name(rows: &[Entry]) -> BTreeMap<String, (EntryKind, u64, Option<i64>)> {
    rows.iter().map(|e| (e.name.clone(), e.fingerprint())).collect()
}

/// `old` with the diff applied: the removed rows gone, the inserted and changed ones taken from
/// `new`, every other row as it was.
fn apply(old: &[Entry], d: &ListingDiff, new: &[Entry]) -> Vec<Entry> {
    let take = |name: &String| new.iter().find(|e| &e.name == name).expect("an inserted or changed name is in new");
    let mut out: Vec<Entry> =
        old.iter().filter(|e| !d.removed.contains(&e.name) && !d.changed.contains(&e.name)).cloned().collect();
    out.extend(d.changed.iter().chain(&d.inserted).map(|n| take(n).clone()));
    out
}

#[hegel::test]
fn applying_the_diff_to_the_old_listing_gives_the_new_one(tc: TestCase) {
    let old = unique(tc.draw(gs::vecs(entries()).max_size(10)));
    let new = unique(tc.draw(gs::vecs(entries()).max_size(10)));
    let d = diff(&old, &new);
    assert_eq!(by_name(&apply(&old, &d, &new)), by_name(&new), "{d:?}");
}

#[hegel::test]
fn a_listing_against_itself_in_any_order_is_no_change(tc: TestCase) {
    let rows = tc.draw(gs::vecs(entries()).max_size(10));
    let listing = unique(rows.clone());
    let shuffled = tc.draw(gs::permutations(listing.clone()));
    assert!(diff(&rows, &rows).is_empty());
    assert!(diff(&listing, &shuffled).is_empty());
}

#[hegel::test]
fn nothing_is_both_inserted_and_removed(tc: TestCase) {
    let old = tc.draw(gs::vecs(entries()).max_size(10));
    let new = tc.draw(gs::vecs(entries()).max_size(10));
    let d = diff(&old, &new);
    let (old_names, new_names) = (by_name(&old), by_name(&new));
    for n in &d.inserted {
        assert!(!d.removed.contains(n) && !d.changed.contains(n), "{n:?} in {d:?}");
        assert!(new_names.contains_key(n) && !old_names.contains_key(n), "{n:?}");
    }
    for n in &d.removed {
        assert!(!d.changed.contains(n), "{n:?} in {d:?}");
        assert!(old_names.contains_key(n) && !new_names.contains_key(n), "{n:?}");
    }
    for n in &d.changed {
        assert!(old_names.contains_key(n) && new_names.contains_key(n), "{n:?}");
    }
}

#[hegel::test]
fn each_list_is_sorted_without_repeats(tc: TestCase) {
    let old = tc.draw(gs::vecs(entries()).max_size(10));
    let new = tc.draw(gs::vecs(entries()).max_size(10));
    let d = diff(&old, &new);
    for list in [&d.inserted, &d.removed, &d.changed] {
        assert!(list.windows(2).all(|w| w[0] < w[1]), "{list:?}");
    }
}

#[hegel::test]
fn a_repeated_name_counts_as_its_last_occurrence(tc: TestCase) {
    let old = tc.draw(gs::vecs(entries()).max_size(10));
    let new = tc.draw(gs::vecs(entries()).max_size(10));
    assert_eq!(diff(&old, &new), diff(&last_wins(&old), &last_wins(&new)));
}
