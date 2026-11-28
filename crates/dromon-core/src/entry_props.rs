//! Properties of sorting listings, for every sort key and direction and any names, sizes and
//! dates a server sends: the result is a permutation of the rows, folders come first, the rows
//! follow the key with names breaking ties, sorting again changes nothing, and merging pages
//! gives what sorting the whole would.

use std::cmp::Ordering;

use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::{Entry, EntryKind, LinkTarget, SortDir, SortKey, merge_sorted, sort_entries};
use crate::natural::natural_cmp;

const KINDS: [EntryKind; 7] = [
    EntryKind::File,
    EntryKind::Dir,
    EntryKind::Symlink { target: None },
    EntryKind::Symlink { target: Some(LinkTarget::File) },
    EntryKind::Symlink { target: Some(LinkTarget::Dir) },
    EntryKind::Symlink { target: Some(LinkTarget::Other) },
    EntryKind::Other,
];

/// Names that collide often (same name, case, digits), and any text.
#[hegel::composite]
fn names(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::sampled_from(["a", "A", "b", "a1", "a01", "a10", ".env", "", "日本", "\u{0}"].as_slice())
            .map(str::to_owned),
        gs::text().max_size(10),
    ))
}

/// A listing row with any of the values a server may send.
#[hegel::composite]
fn entries(tc: &TestCase) -> Entry {
    let small_or_any_size =
        hegel::one_of!(gs::integers::<u64>().max_value(3), gs::just(u64::MAX), gs::integers::<u64>());
    let small_or_any_date = hegel::one_of!(
        gs::integers::<i64>().min_value(-1).max_value(2),
        gs::sampled_from([i64::MIN, i64::MAX].as_slice()),
        gs::integers::<i64>()
    );
    Entry {
        name: tc.draw(names()),
        kind: tc.draw(gs::sampled_from(KINDS.as_slice())),
        size: tc.draw(small_or_any_size),
        mtime: tc.draw(gs::optional(small_or_any_date)),
        mode: tc.draw(gs::optional(gs::integers::<u32>())),
        owner: tc.draw(gs::optional(gs::text().max_size(4))),
        group: tc.draw(gs::optional(gs::text().max_size(4))),
    }
}

#[hegel::composite]
fn keys(tc: &TestCase) -> SortKey {
    tc.draw(gs::sampled_from([SortKey::Name, SortKey::Size, SortKey::Modified].as_slice()))
}

#[hegel::composite]
fn dirs(tc: &TestCase) -> SortDir {
    tc.draw(gs::sampled_from([SortDir::Ascending, SortDir::Descending].as_slice()))
}

/// The rows as a sorted list of their debug forms: equal for two permutations of one listing.
fn multiset(rows: &[Entry]) -> Vec<String> {
    let mut v: Vec<String> = rows.iter().map(|e| format!("{e:?}")).collect();
    v.sort();
    v
}

/// The documented order of two rows of the same group (both folders, or both not): the key,
/// then the natural name order, all of it reversed when descending. Folders have no size.
fn key_then_name(a: &Entry, b: &Entry, key: SortKey, dir: SortDir) -> Ordering {
    let by_key = match key {
        SortKey::Name => Ordering::Equal,
        SortKey::Size if a.kind.is_dir_like() => Ordering::Equal,
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.mtime.cmp(&b.mtime),
    };
    let ord = by_key.then_with(|| natural_cmp(&a.name, &b.name));
    if dir == SortDir::Descending { ord.reverse() } else { ord }
}

#[hegel::test]
fn sorting_is_a_permutation_of_the_rows(tc: TestCase) {
    let rows = tc.draw(gs::vecs(entries()).max_size(20));
    let (key, dir) = (tc.draw(keys()), tc.draw(dirs()));
    let mut sorted = rows.clone();
    sort_entries(&mut sorted, key, dir);
    assert_eq!(multiset(&sorted), multiset(&rows));
}

#[hegel::test]
fn folders_come_first_in_either_direction(tc: TestCase) {
    let mut rows = tc.draw(gs::vecs(entries()).max_size(20));
    let (key, dir) = (tc.draw(keys()), tc.draw(dirs()));
    sort_entries(&mut rows, key, dir);
    let first_not_folder = rows.iter().position(|e| !e.kind.is_dir_like()).unwrap_or(rows.len());
    assert!(rows[first_not_folder..].iter().all(|e| !e.kind.is_dir_like()), "{rows:?}");
}

#[hegel::test]
fn rows_follow_the_key_with_names_breaking_ties(tc: TestCase) {
    let mut rows = tc.draw(gs::vecs(entries()).max_size(20));
    let (key, dir) = (tc.draw(keys()), tc.draw(dirs()));
    sort_entries(&mut rows, key, dir);
    for w in rows.windows(2).filter(|w| w[0].kind.is_dir_like() == w[1].kind.is_dir_like()) {
        assert_ne!(key_then_name(&w[0], &w[1], key, dir), Ordering::Greater, "{:?} before {:?}", w[0], w[1]);
    }
}

#[hegel::test]
fn sorting_twice_is_sorting_once(tc: TestCase) {
    let mut rows = tc.draw(gs::vecs(entries()).max_size(20));
    let (key, dir) = (tc.draw(keys()), tc.draw(dirs()));
    sort_entries(&mut rows, key, dir);
    let once = rows.clone();
    sort_entries(&mut rows, key, dir);
    assert_eq!(rows, once);
}

#[hegel::test]
fn merging_pages_gives_what_sorting_the_whole_does(tc: TestCase) {
    let rows = tc.draw(gs::vecs(entries()).max_size(20));
    let page = tc.draw(gs::integers::<usize>().min_value(1).max_value(8));
    let (key, dir) = (tc.draw(keys()), tc.draw(dirs()));
    let mut want = rows.clone();
    sort_entries(&mut want, key, dir);
    let mut got = Vec::new();
    for chunk in rows.chunks(page) {
        merge_sorted(&mut got, chunk.to_vec(), key, dir);
    }
    assert_eq!(got, want);
}
