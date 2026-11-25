//! Conflict naming (Chrome style) and part-file naming. See `.plans/dromon-plan.md` §3.5, §4.6.

/// Suffix of every in-flight part file.
const PART_SUFFIX: &str = ".dromon-part";

/// Number of `name (N)` candidates tried before falling back.
const MAX_TRIES: u32 = 10_000;

/// Number of `name (copy-N)` fallback candidates checked against `exists`.
const MAX_FALLBACK_TRIES: u32 = 100;

/// Compound extensions that are kept whole (matched case-insensitively).
const COMPOUND_EXTENSIONS: [&str; 4] = [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"];

/// Splits `name` into `(stem, extension)`; the extension includes its leading dot, or is empty.
fn split_extension(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for ext in COMPOUND_EXTENSIONS {
        if lower.len() > ext.len() && lower.ends_with(ext) {
            let at = name.len() - ext.len();
            return (&name[..at], &name[at..]);
        }
    }
    match name.rfind('.') {
        // Leading-dot-only names (".env") and trailing dots ("file.") have no extension.
        Some(0) | None => (name, ""),
        Some(i) if i + 1 == name.len() => (name, ""),
        Some(i) => (&name[..i], &name[i..]),
    }
}

/// Returns `name` if `exists(name)` is false, otherwise the first free Chrome-style name:
/// `stem (1).ext`, `stem (2).ext`, …
///
/// - The extension is the part after the last dot, except for leading-dot-only names
///   (`.env` → `.env (1)`), names without a dot, and names with a trailing dot (`file.`), which
///   have no extension.
/// - `.tar.gz`, `.tar.bz2`, `.tar.xz` and `.tar.zst` are kept whole.
/// - An existing ` (N)` suffix is not re-parsed: `a (1).txt` → `a (1) (1).txt`.
/// - After 10 000 numbered candidates are all taken, falls back to `stem (copy-N).ext` for
///   `N` in 1..=100 (first free), and if even those are all taken, returns `stem (copy-100).ext`
///   unchecked. The function never loops unboundedly and never panics.
pub fn unique_name(name: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(name) {
        return name.to_owned();
    }
    let (stem, ext) = split_extension(name);
    for n in 1..=MAX_TRIES {
        let candidate = format!("{stem} ({n}){ext}");
        if !exists(&candidate) {
            return candidate;
        }
    }
    let mut candidate = String::new();
    for n in 1..=MAX_FALLBACK_TRIES {
        candidate = format!("{stem} (copy-{n}){ext}");
        if !exists(&candidate) {
            break;
        }
    }
    candidate
}

/// Download part file: `<final_name>.dromon-part`.
pub fn download_part_name(final_name: &str) -> String {
    format!("{final_name}{PART_SUFFIX}")
}

/// Upload part file: `.<name>.dromon-part` (hidden on the remote).
pub fn upload_part_name(name: &str) -> String {
    format!(".{name}{PART_SUFFIX}")
}

/// Whether `name` looks like one of our part files (ends with `.dromon-part` and has something
/// before the suffix).
pub fn is_part_file(name: &str) -> bool {
    name.len() > PART_SUFFIX.len() && name.ends_with(PART_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn run(name: &str, taken: &[&str]) -> String {
        let set: HashSet<String> = taken.iter().map(|s| (*s).to_owned()).collect();
        unique_name(name, |n| set.contains(n))
    }

    #[test]
    fn free_name_is_returned_unchanged() {
        assert_eq!(run("report.pdf", &[]), "report.pdf");
        assert_eq!(run("report.pdf", &["other.pdf"]), "report.pdf");
    }

    #[test]
    fn table_of_first_conflicts() {
        let cases = [
            ("report.pdf", "report (1).pdf"),
            (".env", ".env (1)"),
            (".bashrc", ".bashrc (1)"),
            ("Makefile", "Makefile (1)"),
            ("site.tar.gz", "site (1).tar.gz"),
            ("site.tar.bz2", "site (1).tar.bz2"),
            ("site.tar.xz", "site (1).tar.xz"),
            ("site.tar.zst", "site (1).tar.zst"),
            ("SITE.TAR.GZ", "SITE (1).TAR.GZ"),
            ("a.b.c.txt", "a.b.c (1).txt"),
            ("file.", "file. (1)"),
            ("a (1).txt", "a (1) (1).txt"),
            (".tar.gz", ".tar (1).gz"),
            ("x.tgz", "x (1).tgz"),
            ("archive.tar", "archive (1).tar"),
            ("..", ".. (1)"),
            (".hidden.txt", ".hidden (1).txt"),
            ("naïve é.txt", "naïve é (1).txt"),
        ];
        for (name, want) in cases {
            assert_eq!(run(name, &[name]), want, "name = {name:?}");
        }
    }

    #[test]
    fn skips_taken_numbers() {
        assert_eq!(
            run("a.txt", &["a.txt", "a (1).txt", "a (2).txt"]),
            "a (3).txt"
        );
        // A gap is filled: first free, not last + 1.
        assert_eq!(run("a.txt", &["a.txt", "a (2).txt"]), "a (1).txt");
        assert_eq!(
            run("s.tar.gz", &["s.tar.gz", "s (1).tar.gz"]),
            "s (2).tar.gz"
        );
    }

    #[test]
    fn numbered_name_is_not_reparsed() {
        assert_eq!(
            run("a (1).txt", &["a (1).txt", "a (1) (1).txt"]),
            "a (1) (2).txt"
        );
    }

    #[test]
    fn falls_back_after_ten_thousand_tries() {
        let got = unique_name("a.txt", |n| !n.contains("copy-"));
        assert_eq!(got, "a (copy-1).txt");
        let got = unique_name("a.txt", |n| !n.contains("copy-") || n.contains("(copy-1)"));
        assert_eq!(got, "a (copy-2).txt");
    }

    #[test]
    fn numbered_candidate_10000_is_still_tried() {
        let got = unique_name("a.txt", |n| n != "a (10000).txt");
        assert_eq!(got, "a (10000).txt");
    }

    #[test]
    fn fully_saturated_returns_last_fallback() {
        assert_eq!(unique_name("a.txt", |_| true), "a (copy-100).txt");
    }

    #[test]
    fn part_names() {
        assert_eq!(download_part_name("a.zip"), "a.zip.dromon-part");
        assert_eq!(upload_part_name("a.zip"), ".a.zip.dromon-part");
        assert_eq!(download_part_name(""), ".dromon-part");
    }

    #[test]
    fn is_part_file_table() {
        let cases = [
            ("a.zip.dromon-part", true),
            (".a.zip.dromon-part", true),
            ("a.dromon-part", true),
            (".dromon-part", false),
            ("dromon-part", false),
            ("a.zip", false),
            ("a.dromon-part.zip", false),
            ("", false),
        ];
        for (name, want) in cases {
            assert_eq!(is_part_file(name), want, "name = {name:?}");
        }
    }

    #[test]
    fn part_names_round_trip() {
        assert!(is_part_file(&download_part_name("x.bin")));
        assert!(is_part_file(&upload_part_name("x.bin")));
    }
}
