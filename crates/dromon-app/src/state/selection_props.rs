//! Properties of the selection: after any clicks, ranges, arrows, Home and End, Select All,
//! cursor moves, clears and rows leaving or coming, it holds what each operation's doc says, it
//! names only rows on screen (its cursor and anchor too), and in order means the rows' order.
//! Type-to-select finds the first row starting with what was typed, cycles a repeated letter, and
//! builds its prefix until a pause.

use std::collections::BTreeSet;

use hegel::TestCase;
use hegel::generators as gs;

use super::*;

/// Row names: empty, unicode, control characters, names that are prefixes of each other.
#[hegel::composite]
fn row(tc: &TestCase) -> String {
    let odd: Vec<String> =
        ["", "a", "A", "ab", "abc", ".env", "-", "İ", "ß", "日本", "\u{0}"].into_iter().map(String::from).collect();
    tc.draw(hegel::one_of!(gs::sampled_from(odd), gs::from_regex("[a-cA-C.-]{1,4}"), gs::text().max_size(8)))
}

/// The selection beside a model of it, over the rows on screen.
struct Machine {
    sel: Selection,
    rows: Vec<String>,
    items: BTreeSet<String>,
    anchor: Option<String>,
    cursor: Option<String>,
    arrived: usize,
}

impl Machine {
    fn names(&self) -> Vec<&str> {
        self.rows.iter().map(String::as_str).collect()
    }

    fn pick(&self, tc: &TestCase) -> usize {
        tc.draw(gs::integers::<usize>().max_value(self.rows.len() - 1))
    }

    fn model_only(&mut self, i: usize) {
        let name = self.rows[i].clone();
        self.items = BTreeSet::from([name.clone()]);
        self.anchor = Some(name.clone());
        self.cursor = Some(name);
    }

    /// The range from the anchor (if on screen; else from the row itself) to row `to`.
    fn model_extend(&mut self, to: usize, keep: bool) {
        let from = self.anchor.as_ref().and_then(|a| self.rows.iter().position(|r| r == a)).unwrap_or(to);
        if !keep {
            self.items.clear();
        }
        self.items.extend(self.rows[from.min(to)..=from.max(to)].iter().cloned());
        self.anchor = Some(self.rows[from].clone());
        self.cursor = Some(self.rows[to].clone());
    }

    fn model_go(&mut self, to: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = self.cursor.clone().or_else(|| Some(self.rows[to].clone()));
            }
            self.model_extend(to, false);
        } else {
            self.model_only(to);
        }
    }
}

#[hegel::state_machine]
impl Machine {
    #[rule]
    fn click(&mut self, tc: TestCase) {
        tc.assume(!self.rows.is_empty());
        let i = self.pick(&tc);
        self.sel.only(&self.rows[i]);
        self.model_only(i);
    }

    #[rule]
    fn cmd_click(&mut self, tc: TestCase) {
        tc.assume(!self.rows.is_empty());
        let i = self.pick(&tc);
        let name = self.rows[i].clone();
        self.sel.toggle(&name);
        if !self.items.remove(&name) {
            self.items.insert(name.clone());
        }
        self.anchor = Some(name.clone());
        self.cursor = Some(name);
    }

    #[rule]
    fn shift_click(&mut self, tc: TestCase) {
        tc.assume(!self.rows.is_empty());
        let i = self.pick(&tc);
        let keep = tc.draw(gs::booleans());
        let rows = self.rows.clone();
        let names: Vec<&str> = rows.iter().map(String::as_str).collect();
        self.sel.extend_to(&names, names[i], keep);
        self.model_extend(i, keep);
    }

    #[rule]
    fn arrows(&mut self, tc: TestCase) {
        let delta = tc
            .draw(hegel::one_of!(gs::integers::<isize>().min_value(-3).max_value(3), gs::sampled_from(vec![-16, 16])));
        let extend = tc.draw(gs::booleans());
        let rows = self.rows.clone();
        let got = self.sel.step(&rows.iter().map(String::as_str).collect::<Vec<_>>(), delta, extend);
        if rows.is_empty() {
            assert_eq!(got, None);
            return;
        }
        let last = rows.len() as isize - 1;
        let to = match self.cursor.as_ref().and_then(|c| rows.iter().position(|r| r == c)) {
            Some(at) => (at as isize + delta).clamp(0, last),
            None if delta >= 0 => 0,
            None => last,
        } as usize;
        self.model_go(to, extend);
        assert_eq!(got.as_deref(), Some(rows[to].as_str()), "the row now under the cursor");
    }

    #[rule]
    fn home_or_end(&mut self, tc: TestCase) {
        let last = tc.draw(gs::booleans());
        let extend = tc.draw(gs::booleans());
        let rows = self.rows.clone();
        let got = self.sel.jump_to_end(&rows.iter().map(String::as_str).collect::<Vec<_>>(), last, extend);
        if rows.is_empty() {
            assert_eq!(got, None);
            return;
        }
        let to = if last { rows.len() - 1 } else { 0 };
        self.model_go(to, extend);
        assert_eq!(got.as_deref(), Some(rows[to].as_str()));
    }

    #[rule]
    fn select_all(&mut self, _: TestCase) {
        let rows = self.rows.clone();
        self.sel.select_all(&rows.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(rows.iter().all(|r| self.sel.contains(r)) && self.sel.len() == rows.len(), "every row, only rows");
        self.items = rows.iter().cloned().collect();
        if self.cursor.is_none() {
            self.cursor = rows.first().cloned();
            self.anchor = self.cursor.clone();
        }
    }

    #[rule]
    fn set_cursor(&mut self, tc: TestCase) {
        tc.assume(!self.rows.is_empty());
        let i = self.pick(&tc);
        self.sel.set_cursor(&self.rows[i]);
        self.cursor = Some(self.rows[i].clone());
    }

    #[rule]
    fn rows_leave_and_come(&mut self, tc: TestCase) {
        let keep = tc.draw(gs::vecs(gs::booleans()).min_size(self.rows.len()).max_size(self.rows.len()));
        let come = tc.draw(gs::integers::<usize>().max_value(20));
        let mut rows: Vec<String> = self.rows.iter().zip(&keep).filter(|(_, k)| **k).map(|(r, _)| r.clone()).collect();
        for _ in 0..come {
            self.arrived += 1;
            rows.push(format!("\u{1}arrived {}", self.arrived));
        }
        if tc.draw(gs::booleans()) {
            rows.reverse();
        }
        self.sel.retain(&rows.iter().map(String::as_str).collect::<Vec<_>>());
        self.rows = rows;
        self.items.retain(|n| self.rows.contains(n));
        for held in [&mut self.anchor, &mut self.cursor] {
            if held.as_ref().is_some_and(|h| !self.rows.contains(h)) {
                *held = None;
            }
        }
    }

    #[rule]
    fn clear(&mut self, _: TestCase) {
        self.sel.clear();
        self.items.clear();
        self.anchor = None;
        self.cursor = None;
    }

    #[invariant(always_run)]
    fn it_holds_what_each_operation_said(&self, _: TestCase) {
        let items: BTreeSet<String> = self.sel.iter().map(str::to_owned).collect();
        assert_eq!(items, self.items);
        assert_eq!(self.sel.cursor(), self.cursor.as_deref(), "the cursor");
        assert_eq!(self.sel.anchor(), self.anchor.as_deref(), "the anchor");
    }

    #[invariant(always_run)]
    fn it_names_only_rows_on_screen(&self, _: TestCase) {
        assert!(self.sel.iter().all(|n| self.rows.iter().any(|r| r == n)), "a selected name isn't a row");
        for held in [self.sel.cursor(), self.sel.anchor()].into_iter().flatten() {
            assert!(self.rows.iter().any(|r| r == held), "{held:?} isn't a row");
        }
    }

    #[invariant(always_run)]
    fn in_order_is_the_rows_order(&self, _: TestCase) {
        let names = self.names();
        let want: Vec<&str> = names.iter().copied().filter(|n| self.items.contains(*n)).collect();
        assert_eq!(self.sel.in_order(&names), want);
    }
}

#[hegel::test]
fn the_selection_keeps_its_rules_through_any_steps(tc: TestCase) {
    let rows = tc.draw(gs::vecs(row()).unique(true).max_size(30));
    let machine =
        Machine { sel: Selection::default(), rows, items: BTreeSet::new(), anchor: None, cursor: None, arrived: 0 };
    hegel::stateful::machine(machine).run(tc);
}

/// Whether `prefix`, lower-cased, is one letter typed again and again ("ccc").
fn repeated(prefix: &str) -> bool {
    let p: Vec<char> = prefix.to_lowercase().chars().collect();
    p.len() > 1 && p.iter().all(|c| *c == p[0])
}

#[hegel::test]
fn type_to_select_finds_the_first_row_starting_with_what_was_typed(tc: TestCase) {
    let rows = tc.draw(gs::vecs(row()).max_size(12));
    let prefix = tc.draw(hegel::one_of!(row(), gs::from_regex("[a-cA-C]{1,3}")));
    let cursor = tc.draw(gs::optional(row()));
    tc.assume(!repeated(&prefix));
    let names: Vec<&str> = rows.iter().map(String::as_str).collect();
    let p = prefix.to_lowercase();
    let want = if p.is_empty() { None } else { rows.iter().position(|r| r.to_lowercase().starts_with(&p)) };
    assert_eq!(find_prefix(&names, &prefix, cursor.as_deref()), want);
}

#[hegel::test]
fn a_repeated_letter_cycles_through_the_rows_starting_with_it(tc: TestCase) {
    let rows = tc.draw(gs::vecs(row()).unique(true).max_size(12));
    let letter = tc.draw(gs::sampled_from(vec!['a', 'A', 'b', 'İ', 'ß', '.']));
    let times = tc.draw(gs::integers::<usize>().min_value(2).max_value(4));
    let cursor = tc.draw(gs::optional(gs::integers::<usize>().max_value(rows.len())));
    let prefix: String = std::iter::repeat_n(letter, times).collect();
    tc.assume(repeated(&prefix));
    let names: Vec<&str> = rows.iter().map(String::as_str).collect();
    let single: String = prefix.to_lowercase().chars().take(1).collect();
    let hits: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].to_lowercase().starts_with(&single)).collect();
    let on = cursor.and_then(|c| rows.get(c)).map(String::as_str);
    let want = match on.and_then(|c| hits.iter().position(|&h| rows[h] == c)) {
        _ if hits.is_empty() => None,
        Some(k) => Some(hits[(k + 1) % hits.len()]),
        None => Some(hits[0]),
    };
    assert_eq!(find_prefix(&names, &prefix, on), want);
}

#[hegel::test]
fn typed_letters_build_one_prefix_until_a_pause(tc: TestCase) {
    // Times in quarter seconds, exact in an f64: a gap over four quarters is a pause.
    let start = tc.draw(gs::integers::<u64>().max_value(4_000));
    let typed =
        tc.draw(gs::vecs(hegel::tuples!(gs::integers::<u64>().max_value(12), gs::text().max_size(3))).max_size(10));
    let later = tc.draw(gs::integers::<u64>().max_value(12));
    let quarter = |q: u64| q as f64 * 0.25;
    let mut t = TypeSelect::default();
    let (mut buffer, mut last, mut now) = (String::new(), 0, start);
    for (gap, text) in &typed {
        now += gap;
        if now - last > 4 {
            buffer.clear();
        }
        buffer.push_str(text);
        last = now;
        assert_eq!(t.push(text, quarter(now)), buffer, "at {now} quarters");
    }
    let at = now + later;
    assert_eq!(t.collecting(quarter(at)), !buffer.is_empty() && at - last <= 4);
    t.reset();
    assert!(!t.collecting(quarter(at)));
}
