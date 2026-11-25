//! Job kinds and the task state machine. See `.plans/dromon-plan.md` §4.6.

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    Download,
    Upload,
    /// Copy within one host.
    RemoteCopy,
    /// Move within one host (a rename).
    Move,
    CrossHostCopy,
    CrossHostMove,
}

/// Direction of a job relative to the local machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Download,
    Upload,
    /// Remote → remote; never touches the local disk.
    Remote,
}

impl JobKind {
    /// Direction of the job: `Download` → `Download`, `Upload` → `Upload`, everything else
    /// (`RemoteCopy`, `Move`, `CrossHostCopy`, `CrossHostMove`) → `Remote`.
    pub fn direction(self) -> Direction {
        match self {
            Self::Download => Direction::Download,
            Self::Upload => Direction::Upload,
            Self::RemoteCopy | Self::Move | Self::CrossHostCopy | Self::CrossHostMove => Direction::Remote,
        }
    }

    /// Whether the job is listed under the popover's **Uploads** tab. The popover only has
    /// Downloads and Uploads tabs; every job that writes to a remote (Upload, RemoteCopy, Move,
    /// CrossHostCopy, CrossHostMove) goes under Uploads, and only Download under Downloads.
    pub fn in_uploads_tab(self) -> bool {
        !matches!(self, Self::Download)
    }
}

/// Lifecycle state of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Queued,
    /// Scanning a folder ("Preparing… 1,204 files").
    Preparing {
        files_found: u64,
    },
    Running,
    Paused,
    Done,
    Failed {
        error: String,
    },
    Cancelled,
}

impl TaskState {
    /// Queued, Preparing, Running or Paused.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Preparing { .. } | Self::Running | Self::Paused
        )
    }

    /// Done, Failed or Cancelled.
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Done | Self::Failed { .. } | Self::Cancelled)
    }
}

/// Input to the state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEvent {
    Start,
    ScanProgress(u64),
    ScanDone,
    Pause,
    Resume,
    Complete,
    Fail(String),
    Cancel,
    Retry,
}

/// Applies `ev` to `state`. Returns the new state, or `None` if the transition is invalid (the
/// caller keeps the old state).
///
/// Graph:
/// - Queued → Preparing (`Start`) | Cancelled
/// - Preparing → Preparing (`ScanProgress`) | Running (`ScanDone`) | Paused | Failed | Cancelled
/// - Running → Paused | Done (`Complete`) | Failed | Cancelled
/// - Paused → Running (`Resume`) | Cancelled
/// - Failed → Queued (`Retry`)
/// - Done and Cancelled are terminal.
pub fn step(state: &TaskState, ev: TaskEvent) -> Option<TaskState> {
    use TaskEvent as E;
    use TaskState as S;
    match (state, ev) {
        (S::Queued, E::Start) => Some(S::Preparing { files_found: 0 }),
        (S::Queued, E::Cancel) => Some(S::Cancelled),

        (S::Preparing { .. }, E::ScanProgress(n)) => Some(S::Preparing { files_found: n }),
        (S::Preparing { .. }, E::ScanDone) => Some(S::Running),
        (S::Preparing { .. }, E::Pause) => Some(S::Paused),
        (S::Preparing { .. }, E::Fail(error)) => Some(S::Failed { error }),
        (S::Preparing { .. }, E::Cancel) => Some(S::Cancelled),

        (S::Running, E::Pause) => Some(S::Paused),
        (S::Running, E::Complete) => Some(S::Done),
        (S::Running, E::Fail(error)) => Some(S::Failed { error }),
        (S::Running, E::Cancel) => Some(S::Cancelled),

        (S::Paused, E::Resume) => Some(S::Running),
        (S::Paused, E::Cancel) => Some(S::Cancelled),

        (S::Failed { .. }, E::Retry) => Some(S::Queued),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TaskEvent as E;
    use TaskState as S;

    fn all_states() -> Vec<TaskState> {
        vec![
            S::Queued,
            S::Preparing { files_found: 7 },
            S::Running,
            S::Paused,
            S::Done,
            S::Failed {
                error: "boom".into(),
            },
            S::Cancelled,
        ]
    }

    fn all_events() -> Vec<TaskEvent> {
        vec![
            E::Start,
            E::ScanProgress(42),
            E::ScanDone,
            E::Pause,
            E::Resume,
            E::Complete,
            E::Fail("err".into()),
            E::Cancel,
            E::Retry,
        ]
    }

    /// The complete set of legal edges: (from, event, expected result).
    fn legal() -> Vec<(TaskState, TaskEvent, TaskState)> {
        let failed = |e: &str| S::Failed { error: e.into() };
        vec![
            (S::Queued, E::Start, S::Preparing { files_found: 0 }),
            (S::Queued, E::Cancel, S::Cancelled),
            (
                S::Preparing { files_found: 7 },
                E::ScanProgress(42),
                S::Preparing { files_found: 42 },
            ),
            (S::Preparing { files_found: 7 }, E::ScanDone, S::Running),
            (S::Preparing { files_found: 7 }, E::Pause, S::Paused),
            (
                S::Preparing { files_found: 7 },
                E::Fail("err".into()),
                failed("err"),
            ),
            (S::Preparing { files_found: 7 }, E::Cancel, S::Cancelled),
            (S::Running, E::Pause, S::Paused),
            (S::Running, E::Complete, S::Done),
            (S::Running, E::Fail("err".into()), failed("err")),
            (S::Running, E::Cancel, S::Cancelled),
            (S::Paused, E::Resume, S::Running),
            (S::Paused, E::Cancel, S::Cancelled),
            (failed("boom"), E::Retry, S::Queued),
        ]
    }

    #[test]
    fn every_legal_edge() {
        for (from, ev, to) in legal() {
            assert_eq!(step(&from, ev.clone()), Some(to), "{from:?} --{ev:?}-->");
        }
    }

    #[test]
    fn every_other_pair_is_invalid() {
        let legal = legal();
        let mut checked = 0;
        for from in all_states() {
            for ev in all_events() {
                let is_legal = legal.iter().any(|(f, e, _)| {
                    std::mem::discriminant(f) == std::mem::discriminant(&from)
                        && std::mem::discriminant(e) == std::mem::discriminant(&ev)
                });
                if !is_legal {
                    assert_eq!(step(&from, ev.clone()), None, "{from:?} --{ev:?}-->");
                    checked += 1;
                }
            }
        }
        // 7 states x 9 events = 63 pairs, 14 legal.
        assert_eq!(checked, 63 - 14);
    }

    #[test]
    fn terminal_states_accept_nothing() {
        for from in [S::Done, S::Cancelled] {
            for ev in all_events() {
                assert_eq!(step(&from, ev), None);
            }
        }
    }

    #[test]
    fn failed_only_accepts_retry_and_keeps_nothing() {
        let failed = S::Failed { error: "x".into() };
        for ev in all_events() {
            let r = step(&failed, ev.clone());
            if ev == E::Retry {
                assert_eq!(r, Some(S::Queued));
            } else {
                assert_eq!(r, None, "{ev:?}");
            }
        }
    }

    #[test]
    fn full_happy_path_with_pause() {
        let mut s = S::Queued;
        for ev in [
            E::Start,
            E::ScanProgress(10),
            E::ScanDone,
            E::Pause,
            E::Resume,
            E::Complete,
        ] {
            s = step(&s, ev).unwrap();
        }
        assert_eq!(s, S::Done);
    }

    #[test]
    fn fail_retry_cycle() {
        let mut s = S::Running;
        s = step(&s, E::Fail("net".into())).unwrap();
        assert_eq!(
            s,
            S::Failed {
                error: "net".into()
            }
        );
        s = step(&s, E::Retry).unwrap();
        assert_eq!(s, S::Queued);
        s = step(&s, E::Start).unwrap();
        assert_eq!(s, S::Preparing { files_found: 0 });
    }

    #[test]
    fn active_and_finished_flags() {
        let cases = [
            (S::Queued, true, false),
            (S::Preparing { files_found: 0 }, true, false),
            (S::Running, true, false),
            (S::Paused, true, false),
            (S::Done, false, true),
            (
                S::Failed {
                    error: String::new(),
                },
                false,
                true,
            ),
            (S::Cancelled, false, true),
        ];
        for (s, active, finished) in cases {
            assert_eq!(s.is_active(), active, "{s:?}");
            assert_eq!(s.is_finished(), finished, "{s:?}");
        }
    }

    #[test]
    fn job_kind_direction_and_tab() {
        let cases = [
            (JobKind::Download, Direction::Download, false),
            (JobKind::Upload, Direction::Upload, true),
            (JobKind::RemoteCopy, Direction::Remote, true),
            (JobKind::Move, Direction::Remote, true),
            (JobKind::CrossHostCopy, Direction::Remote, true),
            (JobKind::CrossHostMove, Direction::Remote, true),
        ];
        for (kind, dir, uploads_tab) in cases {
            assert_eq!(kind.direction(), dir, "{kind:?}");
            assert_eq!(kind.in_uploads_tab(), uploads_tab, "{kind:?}");
        }
    }
}
