use std::cmp::Ordering;

/// Natural, case-insensitive ordering for file names: `file2` sorts before `file10`, and `a.txt`
/// before `B.txt`. Runs of ASCII digits compare by numeric value (leading zeros break ties, fewer
/// first). Ties on the folded form fall back to a plain byte comparison so the order is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let ord = cmp_folded(a, b);
    if ord != Ordering::Equal { ord } else { a.cmp(b) }
}

fn cmp_folded(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = take_digits(&mut a);
                let db = take_digits(&mut b);
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| da.len().cmp(&db.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = fold(x).cmp(&fold(y));
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        s.push(c);
        it.next();
    }
    s
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

    #[test]
    fn huge_numbers_do_not_overflow() {
        assert_eq!(natural_cmp("x99999999999999999999999", "x100000000000000000000000"), Ordering::Less);
    }
}
