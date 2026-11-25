//! Back and forward for one connection, as a browser keeps them.

use dromon_core::RemotePath;

/// How far back a connection remembers.
pub const HISTORY_LIMIT: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavHistory {
    back: Vec<RemotePath>,
    forward: Vec<RemotePath>,
}

impl NavHistory {
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Leaving `from` for a new folder: it becomes the way back, and forward is forgotten.
    pub fn visit(&mut self, from: RemotePath) {
        if self.back.last() != Some(&from) {
            self.back.push(from);
        }
        if self.back.len() > HISTORY_LIMIT {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    /// Where Back goes from `current`, which becomes the way forward.
    pub fn back(&mut self, current: RemotePath) -> Option<RemotePath> {
        let to = self.back.pop()?;
        self.forward.push(current);
        Some(to)
    }

    /// Where Forward goes from `current`, which becomes the way back.
    pub fn forward(&mut self, current: RemotePath) -> Option<RemotePath> {
        let to = self.forward.pop()?;
        self.back.push(current);
        Some(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RemotePath {
        RemotePath::new(s)
    }

    #[test]
    fn back_and_forward_walk_the_trail() {
        let mut h = NavHistory::default();
        assert!(!h.can_back() && !h.can_forward());
        h.visit(p("/a"));
        h.visit(p("/a/b"));
        // Now at /a/b/c.
        assert_eq!(h.back(p("/a/b/c")), Some(p("/a/b")));
        assert_eq!(h.back(p("/a/b")), Some(p("/a")));
        assert_eq!(h.back(p("/a")), None);
        assert_eq!(h.forward(p("/a")), Some(p("/a/b")));
        assert_eq!(h.forward(p("/a/b")), Some(p("/a/b/c")));
        assert_eq!(h.forward(p("/a/b/c")), None);
    }

    #[test]
    fn a_new_visit_forgets_forward() {
        let mut h = NavHistory::default();
        h.visit(p("/a"));
        assert_eq!(h.back(p("/b")), Some(p("/a")));
        assert!(h.can_forward());
        h.visit(p("/a"));
        assert!(!h.can_forward());
    }

    #[test]
    fn history_is_bounded() {
        let mut h = NavHistory::default();
        for i in 0..(HISTORY_LIMIT + 20) {
            h.visit(p(&format!("/{i}")));
        }
        let mut n = 0;
        let mut cur = p("/now");
        while let Some(prev) = h.back(cur.clone()) {
            cur = prev;
            n += 1;
        }
        assert_eq!(n, HISTORY_LIMIT);
        assert_eq!(cur, p("/20"));
    }
}
