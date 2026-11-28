//! Properties of the natural name order: it is a total order on every pair and triple of
//! names, two names compare equal only when they are the same, digit runs compare by value
//! (more than u64 holds too), and sorting any list with it leaves the names in order.

use std::cmp::Ordering;

use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::natural_cmp;

/// Pieces that make names collide on the folded form: digit runs with leading zeros and longer
/// than u64, mixed case, letters whose lowercase is ASCII (the Kelvin sign) or several chars
/// (İ, ß), combining marks, dots, separators and control characters.
const PIECES: [&str; 34] = [
    "",
    "0",
    "00",
    "007",
    "1",
    "9",
    "10",
    "18446744073709551616",
    "99999999999999999999999",
    "a",
    "A",
    "b",
    "B",
    "k",
    "\u{212a}",
    "i",
    "\u{130}",
    "ß",
    "ss",
    "é",
    "É",
    "e\u{301}",
    "Ω",
    "ω",
    "日本",
    ".",
    "..",
    "-",
    "_",
    " ",
    "~",
    "/",
    "\u{0}",
    "\n",
];

/// A file name as a server may send it: mostly built from colliding pieces, sometimes any text.
#[hegel::composite]
fn names(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::vecs(gs::sampled_from(PIECES.as_slice())).max_size(6).map(|parts| parts.concat()),
        gs::from_regex(r"\.?[a-zA-Z_ ]{0,3}(0{0,3}[0-9]{1,25}[a-zA-Z.]{0,2}){0,3}"),
        gs::text().max_size(12),
    ))
}

#[hegel::test]
fn the_order_is_antisymmetric(tc: TestCase) {
    let a = tc.draw(names());
    let b = tc.draw(names());
    assert_eq!(natural_cmp(&a, &b), natural_cmp(&b, &a).reverse());
}

#[hegel::test]
fn the_order_is_transitive(tc: TestCase) {
    let a = tc.draw(names());
    let b = tc.draw(names());
    let c = tc.draw(names());
    for [x, y, z] in [[&a, &b, &c], [&a, &c, &b], [&b, &a, &c], [&b, &c, &a], [&c, &a, &b], [&c, &b, &a]] {
        if natural_cmp(x, y) != Ordering::Greater && natural_cmp(y, z) != Ordering::Greater {
            assert_ne!(natural_cmp(x, z), Ordering::Greater, "{x:?} <= {y:?} <= {z:?}");
        }
    }
}

#[hegel::test]
fn names_compare_equal_only_when_they_are_the_same(tc: TestCase) {
    let a = tc.draw(names());
    let b = tc.draw(names());
    assert_eq!(natural_cmp(&a, &a), Ordering::Equal);
    assert_eq!(natural_cmp(&a, &b) == Ordering::Equal, a == b);
}

#[hegel::test]
fn digit_runs_compare_by_their_value(tc: TestCase) {
    let prefix = tc.draw(gs::from_regex("[a-zA-Z_.]{0,3}"));
    let m = tc.draw(gs::integers::<u128>());
    let n = tc.draw(gs::integers::<u128>());
    let suffix = tc.draw(gs::from_regex("[a-z.]{0,2}"));
    let (a, b) = (format!("{prefix}{m}{suffix}"), format!("{prefix}{n}{suffix}"));
    assert_eq!(natural_cmp(&a, &b), m.cmp(&n), "{a:?} vs {b:?}");
}

#[hegel::test]
fn fewer_leading_zeros_sort_first_for_the_same_value(tc: TestCase) {
    let n = tc.draw(gs::integers::<u64>());
    let fewer = tc.draw(gs::integers::<usize>().max_value(5));
    let more = tc.draw(gs::integers::<usize>().min_value(fewer + 1).max_value(8));
    let (a, b) = (format!("img{}{n}", "0".repeat(fewer)), format!("img{}{n}", "0".repeat(more)));
    assert_eq!(natural_cmp(&a, &b), Ordering::Less, "{a:?} vs {b:?}");
}

#[hegel::test]
fn sorting_leaves_every_neighbour_in_order(tc: TestCase) {
    let mut v = tc.draw(gs::vecs(names()).max_size(30));
    v.sort_by(|a, b| natural_cmp(a, b));
    for w in v.windows(2) {
        assert_ne!(natural_cmp(&w[0], &w[1]), Ordering::Greater, "{:?} before {:?}", w[0], w[1]);
    }
}
