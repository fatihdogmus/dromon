//! Properties of back and forward, after any visits, backs, forwards and landings: the history
//! stays within its limit, Back then Forward (or Forward then Back) returns where you were and
//! changes nothing, a visit forgets forward and leads back to where it left, and the recent
//! folders are each once, newest first: where Forward leads, then where Back leads.

use hegel::TestCase;
use hegel::generators as gs;

use super::*;

/// Folders from a small set, so the trail comes back to them.
fn folder() -> impl hegel::PrintableGenerator<RemotePath> {
    use hegel::Generator;
    gs::from_regex("(/[a-c]{1,2}){0,3}").map(|s| RemotePath::new(&s))
}

/// A connection's history and the folder it's in.
struct Machine {
    history: NavHistory,
    here: RemotePath,
}

#[hegel::state_machine]
impl Machine {
    #[rule(weight = 4.0)]
    fn visit(&mut self, tc: TestCase) {
        let to = tc.draw(folder());
        // The view visits only a folder it isn't in (`ConnView::go`).
        tc.assume(to != self.here);
        let from = std::mem::replace(&mut self.here, to);
        self.history.visit(from.clone());
        assert!(!self.history.can_forward(), "a visit forgets forward");
        assert_eq!(self.history.back.last(), Some(&from), "Back leads where the visit left");
    }

    #[rule]
    fn back(&mut self, _: TestCase) {
        let was = self.here.clone();
        if let Some(to) = self.history.back(was.clone()) {
            self.here = to;
            assert_eq!(self.history.recent().first(), Some(&was), "the folder Back left comes first");
        }
    }

    #[rule]
    fn forward(&mut self, _: TestCase) {
        if let Some(to) = self.history.forward(self.here.clone()) {
            self.here = to;
        }
    }

    #[rule]
    fn landed(&mut self, tc: TestCase) {
        let at = tc.draw(folder());
        let before = self.history.clone();
        self.history.landed(&at);
        let mut want = before;
        if want.back.last() == Some(&at) {
            want.back.pop();
        }
        assert_eq!(self.history, want, "landing drops only a way back to where it landed");
        self.here = at;
    }

    #[rule]
    fn back_then_forward(&mut self, _: TestCase) {
        let before = self.history.clone();
        if let Some(to) = self.history.back(self.here.clone()) {
            assert_eq!(self.history.forward(to), Some(self.here.clone()), "Forward returns where you were");
            assert_eq!(self.history, before, "and changes nothing");
        }
    }

    #[rule]
    fn forward_then_back(&mut self, _: TestCase) {
        let before = self.history.clone();
        if let Some(to) = self.history.forward(self.here.clone()) {
            assert_eq!(self.history.back(to), Some(self.here.clone()), "Back returns where you were");
            assert_eq!(self.history, before, "and changes nothing");
        }
    }

    #[invariant(always_run)]
    fn it_stays_within_its_limit(&self, _: TestCase) {
        assert!(self.history.back.len() <= HISTORY_LIMIT, "{} back", self.history.back.len());
        assert!(self.history.forward.len() <= HISTORY_LIMIT, "{} forward", self.history.forward.len());
        assert_eq!(self.history.can_back(), !self.history.back.is_empty());
        assert_eq!(self.history.can_forward(), !self.history.forward.is_empty());
    }

    #[invariant(always_run)]
    fn recent_folders_come_once_newest_first(&self, _: TestCase) {
        let recent = self.history.recent();
        for (i, p) in recent.iter().enumerate() {
            assert!(!recent[..i].contains(p), "{p:?} twice in {recent:?}");
        }
        let h = &self.history;
        let mut want: Vec<RemotePath> = Vec::new();
        for p in h.forward.iter().rev().chain(h.back.iter().rev()) {
            if !want.contains(p) {
                want.push(p.clone());
            }
        }
        assert_eq!(recent, want);
    }
}

#[hegel::test]
fn back_and_forward_keep_their_rules_through_any_steps(tc: TestCase) {
    // Long enough, with visits weighted up, to pass the limit of a hundred.
    let machine = Machine { history: NavHistory::default(), here: RemotePath::root() };
    hegel::stateful::machine(machine).steps(400).run(tc);
}
