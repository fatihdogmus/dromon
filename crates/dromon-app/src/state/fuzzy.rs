//! Fuzzy matching for the ⌘K palette (§6.8): the query's characters in order anywhere in a
//! title (a subsequence), ignoring case and the query's spaces. Of every way the characters can
//! line up, the best scoring one counts: each matched character scores, more at the start of a
//! word (the title's start, after a space or punctuation, a camelCase hump) and more again right
//! after the previous match; characters skipped between matches cost a little, as do those
//! before the first. Equal scores keep the order they came in.

#[cfg(test)]
#[path = "fuzzy_tests.rs"]
mod tests;

/// Every matched character.
const MATCH: i32 = 16;
/// A match on the title's first character.
const FIRST: i32 = 32;
/// A match at the start of a later word.
const WORD: i32 = 24;
/// A match right after the previous one.
const NEXT: i32 = 24;
/// Each character skipped between two matches, up to [`GAP_MAX`] for one gap.
const GAP: i32 = 1;
const GAP_MAX: i32 = 12;
/// Each character before the first match, up to [`LEAD_MAX`].
const LEAD: i32 = 1;
const LEAD_MAX: i32 = 8;

/// How well a query matched a title, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    /// The matched characters as byte ranges of the title, in order, neighbours merged: for
    /// highlighting.
    pub ranges: Vec<(usize, usize)>,
}

/// One character compared without case: its first lowercase character (`İ` → `i`).
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// What a title's character at `i` earns for starting a word.
fn start_bonus(chars: &[(usize, char)], i: usize) -> i32 {
    if i == 0 {
        return FIRST;
    }
    let (prev, cur) = (chars[i - 1].1, chars[i].1);
    if !prev.is_alphanumeric() || (prev.is_lowercase() && cur.is_uppercase()) { WORD } else { 0 }
}

/// How `query` matches `title`, or `None` when it doesn't. An empty query (or one of spaces
/// only) matches everything with score 0.
pub fn score(query: &str, title: &str) -> Option<Match> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(fold).collect();
    if q.is_empty() {
        return Some(Match { score: 0, ranges: Vec::new() });
    }
    let t: Vec<(usize, char)> = title.char_indices().collect();
    let folded: Vec<char> = t.iter().map(|&(_, c)| fold(c)).collect();
    let (m, n) = (q.len(), t.len());
    if m > n {
        return None;
    }
    // best[i][j]: the best score with q[..=i] matched and q[i] on t[j]; from[i][j]: where q[i-1] was.
    let mut best = vec![vec![None::<i32>; n]; m];
    let mut from = vec![vec![0usize; n]; m];
    for j in 0..n {
        if folded[j] == q[0] {
            best[0][j] = Some(MATCH + start_bonus(&t, j) - (j as i32 * LEAD).min(LEAD_MAX));
        }
    }
    for i in 1..m {
        for j in i..n {
            if folded[j] != q[i] {
                continue;
            }
            let mut top: Option<(i32, usize)> = None;
            for (k, prev) in best[i - 1].iter().enumerate().take(j).skip(i - 1) {
                let Some(prev) = *prev else { continue };
                let link = if k + 1 == j { NEXT.max(start_bonus(&t, j)) } else { start_bonus(&t, j) - ((j - k - 1) as i32 * GAP).min(GAP_MAX) };
                let s = prev + MATCH + link;
                if top.is_none_or(|(b, _)| s > b) {
                    top = Some((s, k));
                }
            }
            if let Some((s, k)) = top {
                best[i][j] = Some(s);
                from[i][j] = k;
            }
        }
    }
    let (mut j, score) = (0..n).filter_map(|j| best[m - 1][j].map(|s| (j, s))).fold(None, |acc: Option<(usize, i32)>, (j, s)| match acc {
        Some((_, b)) if b >= s => acc,
        _ => Some((j, s)),
    })?;
    let mut at = vec![0usize; m];
    for i in (0..m).rev() {
        at[i] = j;
        if i > 0 {
            j = from[i][j];
        }
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &j in &at {
        let (start, c) = t[j];
        let end = start + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.1 == start => last.1 = end,
            _ => ranges.push((start, end)),
        }
    }
    Some(Match { score, ranges })
}

/// The titles `query` matches, best first (equal scores in the order given), as their indices
/// with how they matched.
pub fn rank<'a>(query: &str, titles: impl IntoIterator<Item = &'a str>) -> Vec<(usize, Match)> {
    let mut hits: Vec<(usize, Match)> = titles.into_iter().enumerate().filter_map(|(i, t)| score(query, t).map(|m| (i, m))).collect();
    // Stable: ties stay in order.
    hits.sort_by_key(|(_, m)| std::cmp::Reverse(m.score));
    hits
}
