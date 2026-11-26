//! Generator entries: many similar entries from one line of a fixture, e.g. 10,000 photos:
//!
//! ```json
//! {"generate": {"pattern": "IMG_{05}.jpg", "count": 10000, "size": [1500000, 6299999],
//!               "mtime": {"end": "2026-11-25T14:00:00Z", "every": 3120, "jitter": 2340}}}
//! ```
//!
//! Random parts (a size range, a date's jitter) come from a small xorshift stream seeded with
//! the FNV-1a hash of `seed` (default: the folder's path; `<path>#<n>` for its n-th generator
//! after the first), so a listing is the same on every run.

use dromon_core::{Entry, EntryKind, RemotePath};

use super::default_mode;

/// How the names go: `prefix`, the number (zero-padded to `width`), `suffix`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub prefix: String,
    pub width: usize,
    pub suffix: String,
}

impl Pattern {
    /// Parses `IMG_{05}.jpg` (zero-padded to 5 digits) or `file-{}.txt` (no padding): exactly
    /// one `{}` or `{0N}`.
    pub fn parse(s: &str) -> Result<Pattern, String> {
        let bad = || format!("{s:?} needs exactly one {{}} or {{0N}} (N digits, zero-padded) for the number");
        let open = s.find('{').ok_or_else(bad)?;
        let close = open + s[open..].find('}').ok_or_else(bad)?;
        let (prefix, spec, suffix) = (&s[..open], &s[open + 1..close], &s[close + 1..]);
        if prefix.contains(['{', '}']) || suffix.contains(['{', '}']) {
            return Err(bad());
        }
        let width = match spec {
            "" => 0,
            _ if spec.len() >= 2 && spec.starts_with('0') && spec.bytes().all(|b| b.is_ascii_digit()) => spec[1..].parse().map_err(|_| bad())?,
            _ => return Err(bad()),
        };
        if width > 20 {
            return Err(format!("{s:?}: pad the number to at most 20 digits"));
        }
        Ok(Pattern { prefix: prefix.to_owned(), width, suffix: suffix.to_owned() })
    }

    pub fn name(&self, n: u64) -> String {
        format!("{}{:0width$}{}", self.prefix, n, self.suffix, width = self.width)
    }
}

/// A size: fixed, or drawn from `min..=max`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Size {
    Fixed(u64),
    Range(u64, u64),
}

/// A modification time: none, fixed, or one every `every` seconds ending `every` before `end`
/// (entry `i` of `count` is `every × (count − i)` before `end`), each up to `jitter` seconds
/// earlier in whole minutes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mtime {
    None,
    Fixed(i64),
    Series { end: i64, every: i64, jitter: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generator {
    pub pattern: Pattern,
    pub count: u64,
    /// The first number (default 1).
    pub start: u64,
    pub kind: EntryKind,
    pub size: Size,
    pub mtime: Mtime,
    pub mode: Option<u32>,
    pub seed: Option<String>,
}

/// The most entries one generator makes.
pub const MAX_COUNT: u64 = 1_000_000;

impl Generator {
    /// The entries, for the `index`-th generator of `folder`.
    pub fn entries(&self, folder: &RemotePath, index: usize) -> Vec<Entry> {
        let seed = match (&self.seed, index) {
            (Some(s), _) => s.clone(),
            (None, 0) => folder.as_str().to_owned(),
            (None, n) => format!("{folder}#{n}"),
        };
        let mut rng = Rng(hash(&seed));
        let mode = self.mode.or_else(|| default_mode(self.kind));
        (0..self.count)
            .map(|i| {
                // Drawn in this order (size, then the date) so a listing never changes.
                let size = match self.size {
                    Size::Fixed(s) => s,
                    Size::Range(min, max) => min + rng.below(max - min + 1),
                };
                let mtime = match self.mtime {
                    Mtime::None => None,
                    Mtime::Fixed(t) => Some(t),
                    Mtime::Series { end, every, jitter } => {
                        let back = (self.count - i) as i64 * every;
                        let jitter = if jitter > 0 { rng.below(jitter as u64 / 60 + 1) as i64 * 60 } else { 0 };
                        Some(end - back - jitter)
                    }
                };
                Entry { name: self.pattern.name(self.start + i), kind: self.kind, size, mtime, mode, owner: None, group: None }
            })
            .collect()
    }
}

/// FNV-1a, for stable pseudo-random contents.
pub fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// A small xorshift stream seeded from a hash.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}
