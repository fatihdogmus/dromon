use super::*;

fn order<'a>(query: &str, titles: &[&'a str]) -> Vec<&'a str> {
    rank(query, titles.iter().copied()).into_iter().map(|(i, _)| titles[i]).collect()
}

fn ranges(query: &str, title: &str) -> Vec<(usize, usize)> {
    score(query, title).expect("a match").ranges
}

#[test]
fn a_subsequence_matches_ignoring_case_and_spaces() {
    assert!(score("nf", "New folder").is_some());
    assert!(score("NEW F", "new folder").is_some());
    assert!(score("fn", "New folder").is_none(), "out of order");
    assert!(score("newx", "New folder").is_none());
    assert!(score("toolong", "short").is_none());
    assert_eq!(ranges("nf", "New folder"), [(0, 1), (4, 5)]);
    assert_eq!(ranges("new", "New folder"), [(0, 3)], "neighbours merge");
}

#[test]
fn word_starts_and_runs_rank_first() {
    // Word starts beat the same letters inside words.
    assert_eq!(order("sh", &["Refresh", "Show transfers"]), ["Show transfers", "Refresh"]);
    // A run beats scattered letters.
    assert_eq!(order("ab", &["Garb", "Grab"]), ["Grab", "Garb"]);
    // The best alignment counts, not the first: "copa" lines up on "Copy" and "path".
    assert_eq!(ranges("cp", "Clear Copy path"), [(0, 1), (11, 12)]);
    assert_eq!(ranges("copa", "Clear Copy path"), [(6, 8), (11, 13)]);
    // camelCase humps are word starts.
    assert_eq!(order("ql", &["quietly", "quickLook"]), ["quickLook", "quietly"]);
    // The palette's own: "nf" finds New folder before Clear finished transfers.
    assert_eq!(order("nf", &["Clear finished transfers", "New folder"]), ["New folder", "Clear finished transfers"]);
    assert_eq!(order("ref", &["Clear finished transfers", "Refresh"]), ["Refresh", "Clear finished transfers"]);
}

#[test]
fn ties_keep_the_order_given() {
    assert_eq!(score("re", "Refresh").unwrap().score, score("re", "Rename").unwrap().score);
    assert_eq!(order("re", &["Rename", "Refresh"]), ["Rename", "Refresh"]);
    assert_eq!(order("re", &["Refresh", "Rename"]), ["Refresh", "Rename"]);
}

#[test]
fn unicode_folds_case_and_reports_byte_ranges() {
    assert_eq!(ranges("ün", "Grün und blau"), [(2, 5)], "ü is two bytes");
    assert_eq!(ranges("ÜB", "über"), [(0, 3)]);
    assert!(score("straße", "Straße").is_some());
    assert_eq!(ranges("ö", "/srv/böxe"), [(6, 8)]);
    // Paths: the slash starts a word.
    assert_eq!(order("/vw", &["/srv/vw-cache", "/var/www"]), ["/var/www", "/srv/vw-cache"]);
    assert_eq!(order("rel", &["/var/www/site/releases", "/var/log"]), ["/var/www/site/releases"]);
}

#[test]
fn an_empty_query_matches_everything_in_order() {
    let m = score("", "Anything").unwrap();
    assert_eq!((m.score, m.ranges), (0, vec![]));
    assert_eq!(score("   ", "Anything").unwrap().score, 0);
    assert_eq!(order("", &["b", "a", "c"]), ["b", "a", "c"]);
    assert!(score("", "").is_some());
    assert!(score("a", "").is_none());
}
