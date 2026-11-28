//! Properties of conflict and part-file naming, for any name: a free name comes back as it is,
//! a taken one never does and each next free name is new, the extension stays as documented,
//! part-file names are recognised as part files, and other names are not.

use std::collections::BTreeSet;

use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::{download_part_name, is_part_file, unique_name, upload_part_name};

const PART_SUFFIX: &str = ".dromon-part";

/// File names with every kind of dot and extension, a few names that look numbered or like
/// part files already, and any text.
#[hegel::composite]
fn names(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::sampled_from(
            [
                "report.pdf",
                ".env",
                "Makefile",
                "file.",
                "..",
                ".",
                "",
                "a (1).txt",
                "site.tar.gz",
                "SITE.TAR.ZST",
                ".tar.gz",
                "x.dromon-part",
                ".dromon-part",
                "naïve é.txt",
                "日本.テキスト",
                "-rf",
            ]
            .as_slice()
        )
        .map(str::to_owned),
        gs::from_regex(r"\.?[a-zA-Zé ]{0,4}(\.[a-zA-Z0-9]{0,3}){0,3}\.?"),
        gs::text().max_size(16),
    ))
}

/// `(stem, extension)` as `unique_name`'s doc defines them.
fn documented_split(name: &str) -> (&str, &str) {
    for ext in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"] {
        let at = name.len().saturating_sub(ext.len());
        if name.len() > ext.len() && name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(ext) {
            return name.split_at(at);
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name.split_at(i),
        _ => (name, ""),
    }
}

#[hegel::test]
fn a_free_name_comes_back_as_it_is(tc: TestCase) {
    let name = tc.draw(names());
    let taken = tc.draw(gs::btree_sets(names()).max_size(5));
    tc.assume(!taken.contains(&name));
    assert_eq!(unique_name(&name, |n| taken.contains(n)), name);
}

#[hegel::test]
fn a_taken_name_never_comes_back_and_each_next_one_is_new(tc: TestCase) {
    let name = tc.draw(names());
    let others = tc.draw(gs::btree_sets(names()).max_size(5));
    let rounds = tc.draw(gs::integers::<usize>().max_value(25));
    let mut taken: BTreeSet<String> = others;
    taken.insert(name.clone());
    for _ in 0..rounds {
        let got = unique_name(&name, |n| taken.contains(n));
        assert!(!taken.contains(&got), "{got:?} is taken");
        taken.insert(got);
    }
}

#[hegel::test]
fn the_first_free_name_keeps_the_extension(tc: TestCase) {
    let name = tc.draw(names());
    let (stem, ext) = documented_split(&name);
    assert_eq!(unique_name(&name, |n| n == name), format!("{stem} (1){ext}"));
}

#[hegel::test]
fn part_file_names_are_part_files(tc: TestCase) {
    let name = tc.draw(names());
    assert!(is_part_file(&upload_part_name(&name)), "{name:?}");
    assert_eq!(is_part_file(&download_part_name(&name)), !name.is_empty(), "{name:?}");
}

#[hegel::test]
fn other_names_are_not_part_files(tc: TestCase) {
    let name = tc.draw(names());
    let looks_like_one = name.ends_with(PART_SUFFIX) && name.len() > PART_SUFFIX.len();
    assert_eq!(is_part_file(&name), looks_like_one, "{name:?}");
}
