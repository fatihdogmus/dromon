//! Properties of remote paths: parsing gives a normalized path and is idempotent on its own
//! output, joining a plain name and taking the parent or the name back undoes it, the
//! breadcrumbs run from the root down to the path, `starts_with` agrees with the components,
//! and the components rebuild the path.

use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::RemotePath;

/// Path pieces a server or a typed path may hold: empty parts, `.` and `..`, a Windows drive,
/// leading dashes and dots, spaces, unicode and control characters.
const PIECES: [&str; 16] =
    ["", ".", "..", "...", "a", "b", "ab", "C:", "-rf", ".hidden", " ", "é", "日本", "\n", "\u{0}", "\\"];

/// Any string as a path: built from pieces with slashes, or any text.
#[hegel::composite]
fn raw_paths(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        hegel::tuples!(gs::booleans(), gs::vecs(gs::sampled_from(PIECES.as_slice())).max_size(6))
            .map(|(absolute, parts)| format!("{}{}", if absolute { "/" } else { "" }, parts.join("/"))),
        gs::text().max_size(20),
    ))
}

#[hegel::composite]
fn paths(tc: &TestCase) -> RemotePath {
    RemotePath::new(&tc.draw(raw_paths()))
}

/// One path component, the names `join` takes as a child: not empty, no `/`, not `.` or `..`.
#[hegel::composite]
fn names(tc: &TestCase) -> String {
    tc.draw(
        hegel::one_of!(
            gs::sampled_from(["a", "...", ".hidden", "-rf", "C:", " ", "é", "日本", "\n", "\\"].as_slice())
                .map(str::to_owned),
            gs::text().min_size(1).max_size(12),
        )
        .filter(|n: &String| !n.contains('/') && n != "." && n != ".."),
    )
}

#[hegel::test]
fn parsing_gives_a_normalized_path(tc: TestCase) {
    let s = tc.draw(raw_paths());
    let p = RemotePath::new(&s);
    let text = p.as_str();
    assert!(text.starts_with('/'), "{p:?}");
    assert!(p.is_root() || !text.ends_with('/'), "{p:?}");
    assert!(text[1..].split('/').all(|c| !matches!(c, "." | "..")), "{p:?}");
    assert!(p.is_root() || text[1..].split('/').all(|c| !c.is_empty()), "{p:?}");
}

#[hegel::test]
fn parsing_is_idempotent_on_its_own_output(tc: TestCase) {
    let p = tc.draw(paths());
    assert_eq!(RemotePath::new(p.as_str()), p);
    assert_eq!(RemotePath::from(String::from(p.clone())), p);
}

#[hegel::test]
fn joining_a_name_then_taking_the_parent_gives_the_folder(tc: TestCase) {
    let p = tc.draw(paths());
    let n = tc.draw(names());
    assert_eq!(p.join(&n).parent(), Some(p));
}

#[hegel::test]
fn joining_a_name_then_taking_the_name_gives_it_back(tc: TestCase) {
    let p = tc.draw(paths());
    let n = tc.draw(names());
    assert_eq!(p.join(&n).file_name(), Some(n.as_str()));
}

#[hegel::test]
fn breadcrumbs_run_from_the_root_down_to_the_path(tc: TestCase) {
    let p = tc.draw(paths());
    let crumbs = p.ancestors_from_root();
    assert_eq!(crumbs.first(), Some(&RemotePath::root()));
    assert_eq!(crumbs.last(), Some(&p));
    assert_eq!(crumbs.len(), p.components().count() + 1);
    for w in crumbs.windows(2) {
        assert_eq!(w[1].parent().as_ref(), Some(&w[0]));
    }
}

#[hegel::test]
fn a_path_starts_with_itself_and_every_ancestor(tc: TestCase) {
    let p = tc.draw(paths());
    for a in p.ancestors_from_root() {
        assert!(p.starts_with(&a), "{p:?} in {a:?}");
    }
}

#[hegel::test]
fn starts_with_agrees_with_the_components(tc: TestCase) {
    // A small alphabet, so one path is often inside the other (or only shares a prefix: ab, a).
    let parts = || gs::vecs(gs::sampled_from(["a", "b", "ab"].as_slice())).max_size(4);
    let a = RemotePath::new(&tc.draw(parts()).join("/"));
    let b = RemotePath::new(&tc.draw(parts()).join("/"));
    let (ca, cb): (Vec<&str>, Vec<&str>) = (a.components().collect(), b.components().collect());
    assert_eq!(a.starts_with(&b), ca.starts_with(&cb), "{a:?} in {b:?}");
}

#[hegel::test]
fn the_components_rebuild_the_path(tc: TestCase) {
    let p = tc.draw(paths());
    let parts: Vec<&str> = p.components().collect();
    assert_eq!(format!("/{}", parts.join("/")), p.as_str());
    assert_eq!(RemotePath::new(&parts.join("/")), p);
}
