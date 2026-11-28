//! Properties of fuzzy matching, for any query and title: `score` never panics; it matches
//! exactly when the query's characters, spaces left out and case folded, come in order in the
//! title; its ranges lie in the title on character boundaries, in order, apart, and spell the
//! query; an empty query matches everything and a title matches itself; and `rank` gives each
//! match once, best first, ties in the order given.

use hegel::TestCase;
use hegel::generators as gs;

use super::*;

/// Titles and queries: palette words, paths, unicode that folds to more than one character,
/// spaces, control characters, and anything at all.
#[hegel::composite]
fn words(tc: &TestCase) -> String {
    let odd: Vec<String> = [
        "",
        " ",
        "New folder",
        "Clear finished transfers",
        "/var/www/site",
        "quickLook",
        "İstanbul",
        "straße",
        "ǅ",
        "Ωmega",
        "e\u{301}",
        "日本語",
        "a\tb",
        "\u{0}",
        "AbC aBc",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    tc.draw(hegel::one_of!(gs::sampled_from(odd), gs::from_regex("[ a-cA-C/._-]{0,10}"), gs::text().max_size(24)))
}

/// The query's characters as matching takes them: spaces left out, case folded.
fn wanted(query: &str) -> Vec<char> {
    query.chars().filter(|c| !c.is_whitespace()).map(fold).collect()
}

/// Whether `q` comes in order in `title`, each character folded.
fn in_order(q: &[char], title: &str) -> bool {
    let mut q = q.iter().peekable();
    for c in title.chars().map(fold) {
        if q.peek() == Some(&&c) {
            q.next();
        }
    }
    q.peek().is_none()
}

#[hegel::test]
fn a_title_matches_exactly_when_the_query_comes_in_order_in_it(tc: TestCase) {
    let query = tc.draw(words());
    let title = tc.draw(words());
    let q = wanted(&query);
    assert_eq!(score(&query, &title).is_some(), in_order(&q, &title), "{query:?} in {title:?}");
}

#[hegel::test]
fn a_matchs_ranges_are_in_order_apart_on_boundaries_and_spell_the_query(tc: TestCase) {
    let title = tc.draw(words());
    // A query drawn from the title's own characters, so most of them match.
    let picks = tc.draw(gs::vecs(gs::booleans()).min_size(title.chars().count()).max_size(title.chars().count()));
    let query: String = title.chars().zip(&picks).filter(|(_, p)| **p).map(|(c, _)| c).collect();
    let query = if tc.draw(gs::booleans()) { query.to_uppercase() } else { query };
    let Some(m) = score(&query, &title) else { return };
    let mut end = 0;
    let mut spelled = Vec::new();
    for (i, &(a, b)) in m.ranges.iter().enumerate() {
        assert!(a < b && b <= title.len(), "{:?} outside {title:?}", (a, b));
        assert!(title.is_char_boundary(a) && title.is_char_boundary(b), "{:?} in {title:?}", (a, b));
        assert!(i == 0 || a > end, "{:?} overlap or touch: neighbours merge", m.ranges);
        end = b;
        spelled.extend(title[a..b].chars().map(fold));
    }
    assert_eq!(spelled, wanted(&query), "{query:?} in {title:?}: {:?}", m.ranges);
}

#[hegel::test]
fn an_empty_query_matches_everything_and_a_title_matches_itself(tc: TestCase) {
    let title = tc.draw(words());
    let spaces = tc.draw(gs::from_regex("[ \t\n]{0,3}"));
    assert_eq!(score(&spaces, &title), Some(Match { score: 0, ranges: Vec::new() }));
    assert!(score(&title, &title).is_some(), "{title:?} doesn't match itself");
}

#[hegel::test]
fn rank_gives_each_match_once_best_first_ties_in_the_order_given(tc: TestCase) {
    let titles = tc.draw(gs::vecs(words()).max_size(12));
    let query = tc.draw(hegel::one_of!(words(), gs::from_regex("[a-cA-C/ ]{0,3}")));
    let ranked = rank(&query, titles.iter().map(String::as_str));
    let mut expected: Vec<(usize, Match)> =
        titles.iter().enumerate().filter_map(|(i, t)| score(&query, t).map(|m| (i, m))).collect();
    assert_eq!(ranked.len(), expected.len(), "every match once");
    for w in ranked.windows(2) {
        let ((i, a), (j, b)) = (&w[0], &w[1]);
        assert!(a.score > b.score || (a.score == b.score && i < j), "{i} ({}) before {j} ({})", a.score, b.score);
    }
    expected.sort_by_key(|(i, _)| *i);
    let mut got = ranked.clone();
    got.sort_by_key(|(i, _)| *i);
    assert_eq!(got, expected, "each title ranks with its own match");
}
