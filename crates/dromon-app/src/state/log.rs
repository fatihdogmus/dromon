//! A connection's log (§6.8 "Connection log"): ssh's stderr and our own lifecycle notes, the
//! newest [`LOG_LINES`] of them, each stamped with when it arrived. Never holds secrets: the
//! service only sends what ssh printed and what it did.

use std::collections::VecDeque;

use crate::fmt;

/// Lines kept per host; older ones fall off the front.
pub const LOG_LINES: usize = 2_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    /// Milliseconds since the epoch.
    pub at_ms: i64,
    pub text: String,
}

/// A ring buffer of [`LogLine`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogBuffer {
    lines: VecDeque<LogLine>,
    cap: usize,
    /// Bumped on every push, so the log sheet can tell it changed.
    pub version: u64,
}

impl Default for LogBuffer {
    fn default() -> Self {
        LogBuffer::with_capacity(LOG_LINES)
    }
}

impl LogBuffer {
    pub fn with_capacity(cap: usize) -> LogBuffer {
        LogBuffer { lines: VecDeque::new(), cap: cap.max(1), version: 0 }
    }

    pub fn push(&mut self, at_ms: i64, text: impl Into<String>) {
        if self.lines.len() == self.cap {
            self.lines.pop_front();
        }
        self.lines.push_back(LogLine { at_ms, text: text.into() });
        self.version = self.version.wrapping_add(1);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    #[cfg(test)]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &LogLine> + '_ {
        self.lines.iter()
    }

    /// Every line as `HH:MM:SS.mmm  text`, one per line: what the sheet shows and Copy all copies.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&fmt::clock_ms(line.at_ms));
            out.push_str("  ");
            out.push_str(&line.text);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_newest_lines() {
        let mut log = LogBuffer::with_capacity(3);
        for i in 0..5 {
            log.push(i, format!("line {i}"));
        }
        assert_eq!(log.len(), 3);
        let texts: Vec<&str> = log.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["line 2", "line 3", "line 4"]);
        assert_eq!(log.version, 5);
    }

    #[test]
    fn default_capacity_is_two_thousand() {
        let mut log = LogBuffer::default();
        for i in 0..2_500 {
            log.push(i, "x");
        }
        assert_eq!(log.len(), LOG_LINES);
        assert_eq!(log.iter().next().unwrap().at_ms, 500);
    }

    #[test]
    fn text_has_one_stamped_line_each() {
        let mut log = LogBuffer::default();
        log.push(1_795_615_200_123, "debug1: Connecting to prod");
        log.push(1_795_615_201_000, "Authenticated");
        assert_eq!(log.to_text(), "14:00:00.123  debug1: Connecting to prod\n14:00:01.000  Authenticated");
    }
}
