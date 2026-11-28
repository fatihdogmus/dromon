//! The file list's selection: a set of names plus the anchor a Shift range grows from and the
//! cursor arrow keys move. Names, not indices, so sorting, filtering and a refresh keep it.
//! Every operation takes the rows in their on-screen order.

use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    items: HashSet<String>,
    /// Where a Shift range starts: the last row clicked without Shift.
    anchor: Option<String>,
    /// The row the keyboard is on: the last row clicked or moved to.
    cursor: Option<String>,
}

impl Selection {
    pub fn contains(&self, name: &str) -> bool {
        self.items.contains(name)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    #[cfg(test)]
    pub fn anchor(&self) -> Option<&str> {
        self.anchor.as_deref()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(String::as_str)
    }

    /// The selected names in on-screen order.
    #[cfg(test)]
    pub fn in_order<'a>(&self, rows: &[&'a str]) -> Vec<&'a str> {
        rows.iter().copied().filter(|n| self.contains(n)).collect()
    }

    pub fn clear(&mut self) {
        *self = Selection::default();
    }

    /// A plain click: just this row.
    pub fn only(&mut self, name: &str) {
        self.items.clear();
        self.items.insert(name.to_owned());
        self.anchor = Some(name.to_owned());
        self.cursor = Some(name.to_owned());
    }

    /// Cmd-click: add or remove this row, and range from it next.
    pub fn toggle(&mut self, name: &str) {
        if !self.items.remove(name) {
            self.items.insert(name.to_owned());
        }
        self.anchor = Some(name.to_owned());
        self.cursor = Some(name.to_owned());
    }

    /// Shift-click (or Shift-arrow): the rows from the anchor to `name`, replacing the rest, as
    /// Finder does; with `keep` (Cmd-Shift) the range is added to what's selected. Without an
    /// anchor (or one that's no longer on screen) it starts at `name`.
    pub fn extend_to(&mut self, rows: &[&str], name: &str, keep: bool) {
        let Some(to) = rows.iter().position(|n| *n == name) else { return };
        let from = self.anchor.as_deref().and_then(|a| rows.iter().position(|n| *n == a)).unwrap_or(to);
        let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
        if !keep {
            self.items.clear();
        }
        self.items.extend(rows[lo..=hi].iter().map(|n| (*n).to_owned()));
        self.anchor = Some(rows[from].to_owned());
        self.cursor = Some(name.to_owned());
    }

    /// Arrow keys: move the cursor `delta` rows (clamped). With `extend`, grow the range from
    /// the anchor; else select only the row moved to. With no cursor, Down starts at the first
    /// row and Up at the last. Returns the row now under the cursor.
    pub fn step(&mut self, rows: &[&str], delta: isize, extend: bool) -> Option<String> {
        if rows.is_empty() {
            return None;
        }
        let last = rows.len() as isize - 1;
        let at = self.cursor.as_deref().and_then(|c| rows.iter().position(|n| *n == c));
        let to = match at {
            Some(i) => (i as isize + delta).clamp(0, last),
            None if delta >= 0 => 0,
            None => last,
        } as usize;
        self.go(rows, to, extend)
    }

    /// Home and End: to the first or last row.
    pub fn jump_to_end(&mut self, rows: &[&str], last: bool, extend: bool) -> Option<String> {
        if rows.is_empty() {
            return None;
        }
        self.go(rows, if last { rows.len() - 1 } else { 0 }, extend)
    }

    fn go(&mut self, rows: &[&str], to: usize, extend: bool) -> Option<String> {
        let name = rows[to];
        if extend {
            if self.anchor.is_none() {
                self.anchor = self.cursor.clone().or_else(|| Some(name.to_owned()));
            }
            self.extend_to(rows, name, false);
        } else {
            self.only(name);
        }
        Some(name.to_owned())
    }

    pub fn select_all(&mut self, rows: &[&str]) {
        self.items = rows.iter().map(|n| (*n).to_owned()).collect();
        if self.cursor.is_none() {
            self.cursor = rows.first().map(|n| (*n).to_owned());
            self.anchor = self.cursor.clone();
        }
    }

    /// Puts the keyboard's cursor on `name` without selecting it (its row left; this is the
    /// neighbour the arrows go on from).
    pub fn set_cursor(&mut self, name: &str) {
        self.cursor = Some(name.to_owned());
    }

    /// Drops names that are no longer on screen (after a refresh, a filter or hiding
    /// dotfiles); the cursor and anchor go with them.
    pub fn retain(&mut self, rows: &[&str]) {
        self.retain_rows(rows.iter().copied());
    }

    /// [`Selection::retain`] with the rows given one by one, in one pass and without copying
    /// their names: only the names this holds are looked for among them (by comparing them,
    /// when there are a few; a 100,000-row folder refreshes its rows on every page).
    pub fn retain_rows<'a>(&mut self, rows: impl Iterator<Item = &'a str>) {
        if self.items.is_empty() && self.cursor.is_none() && self.anchor.is_none() {
            return;
        }
        let few = self.items.len() <= 16;
        let on: HashSet<&str> = {
            let held: Vec<&str> = if few {
                self.items
                    .iter()
                    .map(String::as_str)
                    .chain(self.cursor.as_deref())
                    .chain(self.anchor.as_deref())
                    .collect()
            } else {
                Vec::new()
            };
            let holds = |n: &str| {
                if few {
                    held.contains(&n)
                } else {
                    self.items.contains(n) || self.cursor.as_deref() == Some(n) || self.anchor.as_deref() == Some(n)
                }
            };
            rows.filter(|n| holds(n)).collect()
        };
        self.items.retain(|n| on.contains(n.as_str()));
        if self.cursor.as_deref().is_some_and(|c| !on.contains(c)) {
            self.cursor = None;
        }
        if self.anchor.as_deref().is_some_and(|a| !on.contains(a)) {
            self.anchor = None;
        }
    }
}

/// Type-to-select: letters typed within a second of each other build one prefix. Lazily
/// expired, so it needs no timer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypeSelect {
    buffer: String,
    last: f64,
}

/// How long a pause starts a new prefix, in seconds (Finder's is about this).
pub const TYPE_SELECT_PAUSE: f64 = 1.0;

impl TypeSelect {
    /// Adds `typed` at time `now` and returns the prefix to look for.
    pub fn push(&mut self, typed: &str, now: f64) -> &str {
        if now - self.last > TYPE_SELECT_PAUSE {
            self.buffer.clear();
        }
        self.buffer.push_str(typed);
        self.last = now;
        &self.buffer
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    /// Whether what's typed at `now` adds to a prefix being typed: then a Space types too,
    /// rather than opening the preview.
    pub fn collecting(&self, now: f64) -> bool {
        !self.buffer.is_empty() && now - self.last <= TYPE_SELECT_PAUSE
    }
}

/// The first row whose name starts with `prefix`, ignoring case. A repeated single letter
/// ("ccc") cycles through the rows starting with it, as Finder does.
pub fn find_prefix(rows: &[&str], prefix: &str, cursor: Option<&str>) -> Option<usize> {
    let p = prefix.to_lowercase();
    if p.is_empty() {
        return None;
    }
    let starts = |n: &str| n.to_lowercase().starts_with(&p);
    let mut chars = p.chars();
    let first = chars.next()?;
    if p.chars().count() > 1 && p.chars().all(|c| c == first) {
        let single = first.to_string();
        let hits: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].to_lowercase().starts_with(&single)).collect();
        if hits.is_empty() {
            return None;
        }
        let at = cursor.and_then(|c| rows.iter().position(|n| *n == c));
        let next = at.and_then(|a| hits.iter().position(|&h| h == a)).map_or(0, |k| (k + 1) % hits.len());
        return Some(hits[next]);
    }
    rows.iter().position(|n| starts(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: [&str; 6] = ["assets", "src", "a.txt", "b.txt", "c.txt", "d.txt"];

    fn names(s: &Selection) -> Vec<&'static str> {
        s.in_order(&ROWS)
    }

    #[test]
    fn click_selects_one() {
        let mut s = Selection::default();
        s.only("a.txt");
        s.only("b.txt");
        assert_eq!(names(&s), ["b.txt"]);
        assert_eq!(s.cursor(), Some("b.txt"));
    }

    #[test]
    fn cmd_click_toggles() {
        let mut s = Selection::default();
        s.only("a.txt");
        s.toggle("c.txt");
        assert_eq!(names(&s), ["a.txt", "c.txt"]);
        s.toggle("a.txt");
        assert_eq!(names(&s), ["c.txt"]);
    }

    #[test]
    fn shift_click_ranges_from_the_anchor_both_ways() {
        let mut s = Selection::default();
        s.only("a.txt");
        s.extend_to(&ROWS, "c.txt", false);
        assert_eq!(names(&s), ["a.txt", "b.txt", "c.txt"]);
        // Shift-clicking above the anchor replaces the range, keeping the anchor.
        s.extend_to(&ROWS, "src", false);
        assert_eq!(names(&s), ["src", "a.txt"]);
        assert_eq!(s.anchor(), Some("a.txt"));
    }

    #[test]
    fn cmd_shift_click_adds_a_range() {
        let mut s = Selection::default();
        s.only("assets");
        s.toggle("c.txt");
        s.extend_to(&ROWS, "d.txt", true);
        assert_eq!(names(&s), ["assets", "c.txt", "d.txt"]);
    }

    #[test]
    fn shift_click_without_anchor_selects_one() {
        let mut s = Selection::default();
        s.extend_to(&ROWS, "b.txt", false);
        assert_eq!(names(&s), ["b.txt"]);
    }

    #[test]
    fn arrows_move_and_clamp() {
        let mut s = Selection::default();
        assert_eq!(s.step(&ROWS, 1, false).as_deref(), Some("assets"));
        assert_eq!(s.step(&ROWS, 1, false).as_deref(), Some("src"));
        assert_eq!(s.step(&ROWS, -5, false).as_deref(), Some("assets"));
        let mut s = Selection::default();
        assert_eq!(s.step(&ROWS, -1, false).as_deref(), Some("d.txt"));
        assert_eq!(s.step(&ROWS, 9, false).as_deref(), Some("d.txt"));
        assert!(Selection::default().step(&[], 1, false).is_none());
    }

    #[test]
    fn shift_arrows_extend() {
        let mut s = Selection::default();
        s.only("a.txt");
        s.step(&ROWS, 1, true);
        s.step(&ROWS, 1, true);
        assert_eq!(names(&s), ["a.txt", "b.txt", "c.txt"]);
        s.step(&ROWS, -3, true);
        assert_eq!(names(&s), ["src", "a.txt"]);
        s.jump_to_end(&ROWS, true, true);
        assert_eq!(names(&s), ["a.txt", "b.txt", "c.txt", "d.txt"]);
    }

    #[test]
    fn select_all_and_retain() {
        let mut s = Selection::default();
        s.select_all(&ROWS);
        assert_eq!(s.len(), 6);
        s.retain(&["a.txt", "zzz"]);
        assert_eq!(names(&s), ["a.txt"]);
        assert_eq!(s.cursor(), None, "the cursor was on assets, which left");
    }

    #[test]
    fn type_select_builds_a_prefix_then_expires() {
        let mut t = TypeSelect::default();
        assert_eq!(t.push("s", 10.0), "s");
        assert_eq!(t.push("r", 10.4), "sr");
        assert_eq!(t.push("b", 12.0), "b");
    }

    #[test]
    fn type_select_is_collecting_until_a_pause() {
        let mut t = TypeSelect::default();
        assert!(!t.collecting(0.5), "nothing typed yet");
        t.push("m", 10.0);
        assert!(t.collecting(10.6));
        assert!(!t.collecting(11.2), "a pause starts a new prefix");
        t.push("y", 11.2);
        t.reset();
        assert!(!t.collecting(11.3));
    }

    #[test]
    fn prefix_lookup_ignores_case_and_cycles_repeats() {
        let rows = ["Assets", "bin", "b.txt", "c.txt"];
        assert_eq!(find_prefix(&rows, "a", None), Some(0));
        assert_eq!(find_prefix(&rows, "BI", None), Some(1));
        assert_eq!(find_prefix(&rows, "x", None), None);
        assert_eq!(find_prefix(&rows, "bb", Some("bin")), Some(2));
        assert_eq!(find_prefix(&rows, "bb", Some("b.txt")), Some(1));
    }
}

#[cfg(test)]
#[path = "selection_props.rs"]
mod props;
