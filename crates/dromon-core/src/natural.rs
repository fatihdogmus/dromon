use std::cmp::Ordering;

/// Natural, case-insensitive ordering for file names: `file2` sorts before `file10`, and `a.txt`
/// before `B.txt`. Runs of ASCII digits compare by numeric value (leading zeros break ties, fewer
/// first). Ties on the folded form fall back to a plain byte comparison so the order is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let ord = cmp_folded(a, b);
    if ord != Ordering::Equal { ord } else { a.cmp(b) }
}

/// Walks both names by byte offset (always on a char boundary), without allocating: sorting a
/// 100,000-entry folder compares names about 1.7 million times.
fn cmp_folded(a: &str, b: &str) -> Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    loop {
        let (x, y) = match (ab.get(i), bb.get(j)) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&x), Some(&y)) => (x, y),
        };
        let ord = if x.is_ascii_digit() && y.is_ascii_digit() {
            let (da, db) = (digits(a, i), digits(b, j));
            i += da.len();
            j += db.len();
            let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
            ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| da.len().cmp(&db.len()))
        } else if x.is_ascii() && y.is_ascii() {
            // ASCII folds to ASCII: the same order `fold` gives, without Unicode tables.
            i += 1;
            j += 1;
            x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase())
        } else {
            let (cx, cy) = (next_char(a, i), next_char(b, j));
            i += cx.len_utf8();
            j += cy.len_utf8();
            fold(cx).cmp(&fold(cy))
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
}

/// The run of ASCII digits starting at byte `at` of `s`.
fn digits(s: &str, at: usize) -> &str {
    let len = s.as_bytes()[at..].iter().take_while(|c| c.is_ascii_digit()).count();
    &s[at..at + len]
}

/// The char starting at byte `at` of `s` (a char boundary, before the end).
fn next_char(s: &str, at: usize) -> char {
    s[at..].chars().next().unwrap_or_default()
}

fn fold(c: char) -> char {
    // Single-char lowercase is enough for ordering; multi-char expansions (ß) keep their first.
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(sorted(vec!["file10", "file2", "file1"]), ["file1", "file2", "file10"]);
        assert_eq!(sorted(vec!["v1.10.0", "v1.9.2", "v1.9.10"]), ["v1.9.2", "v1.9.10", "v1.10.0"]);
    }

    #[test]
    fn case_insensitive_with_stable_ties() {
        assert_eq!(sorted(vec!["b.txt", "A.txt", "a.txt"]), ["A.txt", "a.txt", "b.txt"]);
    }

    #[test]
    fn leading_zeros_break_ties() {
        assert_eq!(sorted(vec!["img007", "img7", "img07"]), ["img7", "img07", "img007"]);
    }

    #[test]
    fn prefixes_first() {
        assert_eq!(sorted(vec!["abc", "ab"]), ["ab", "abc"]);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
    }

    /// The char-by-char version the byte walk replaced, as the reference for its order.
    fn reference(a: &str, b: &str) -> Ordering {
        fn take(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
            let mut s = String::new();
            while let Some(&c) = it.peek().filter(|c| c.is_ascii_digit()) {
                s.push(c);
                it.next();
            }
            s
        }
        let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
        let folded = loop {
            match (x.peek().copied(), y.peek().copied()) {
                (None, None) => break Ordering::Equal,
                (None, Some(_)) => break Ordering::Less,
                (Some(_), None) => break Ordering::Greater,
                (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                    let (da, db) = (take(&mut x), take(&mut y));
                    let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                    let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| da.len().cmp(&db.len()));
                    if ord != Ordering::Equal {
                        break ord;
                    }
                }
                (Some(p), Some(q)) => {
                    let ord = fold(p).cmp(&fold(q));
                    if ord != Ordering::Equal {
                        break ord;
                    }
                    x.next();
                    y.next();
                }
            }
        };
        if folded != Ordering::Equal { folded } else { a.cmp(b) }
    }

    #[test]
    fn same_order_as_the_char_walk() {
        let names = [
            "",
            "a",
            "A",
            "b",
            "B.txt",
            "a.txt",
            "file1",
            "file01",
            "file001",
            "file10",
            "file2",
            "File2",
            "x9",
            "x10y",
            "x10Y",
            "IMG_0001.jpg",
            "IMG_1.JPG",
            "Éclair",
            "éclair",
            "eclair",
            "Zebra",
            "ß",
            "ss",
            "straße",
            "STRASSE",
            "Ωmega",
            "ωmega",
            "日本",
            "日本2",
            "日本10",
            "a\u{301}",
            "á",
            "0",
            "00",
            "007x",
            "7x",
            "x 1",
            "x_1",
            "-",
            "~",
            "ÄÖÜ9",
            "äöü10",
            "İstanbul",
            "istanbul",
            "data_000123.gz",
            "data_123.gz",
        ];
        for a in names {
            for b in names {
                assert_eq!(natural_cmp(a, b), reference(a, b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn huge_numbers_do_not_overflow() {
        assert_eq!(natural_cmp("x99999999999999999999999", "x100000000000000000000000"), Ordering::Less);
    }
}

#[cfg(test)]
#[path = "natural_props.rs"]
mod props;
