//! The taskbar and notifications as functions of the jobs (§2, §5 M2): counts, the bar's
//! states, the 10 s and focus rules, and what each kind of job's notification says.

use dromon_core::HostId;
use dromon_transfer::{ConflictView, JobId, JobKind, JobView, Progress, TaskState};

use super::*;

fn job(id: u64, kind: JobKind, state: TaskState) -> JobView {
    JobView {
        id: JobId(id),
        kind,
        host: HostId("prod".into()),
        title: format!("job {id}"),
        state,
        progress: Progress { done: 1, total: 10 },
        files_done: 0,
        files_total: 1,
        speed: None,
        eta: None,
        waiting_for_connection: false,
        conflict: None,
        reveal: None,
        errors: Vec::new(),
        skipped: Vec::new(),
        folders: Vec::new(),
    }
}

fn titled(kind: JobKind, title: &str, state: TaskState) -> JobView {
    JobView { title: title.into(), ..job(1, kind, state) }
}

fn failed(why: &str) -> TaskState {
    TaskState::Failed { error: why.into() }
}

fn clash() -> ConflictView {
    ConflictView {
        name: "index.html".into(),
        folder: dromon_core::RemotePath::new("/var/www"),
        existing_is_dir: false,
        existing: Default::default(),
        incoming: Default::default(),
        more: 0,
    }
}

#[test]
fn the_badge_counts_active_jobs_and_the_bar_says_how_they_go() {
    use JobKind::{Download, Upload};
    assert_eq!(dock(&[], false), Dock { count: 0, bar: Bar::None });
    let finished =
        [job(1, Download, TaskState::Done), job(2, Upload, failed("x")), job(3, Download, TaskState::Cancelled)];
    assert_eq!(dock(&finished, false), Dock { count: 0, bar: Bar::None }, "finished jobs don't count");

    let running = [
        job(1, Download, TaskState::Running),
        job(2, Upload, TaskState::Queued),
        job(3, Download, TaskState::Preparing { files_found: 4 }),
    ];
    assert_eq!(dock(&running, false), Dock { count: 3, bar: Bar::Busy });

    // Paused while another runs: still busy. Everything left waiting: paused.
    let mixed =
        [job(1, Download, TaskState::Running), job(2, Download, TaskState::Paused), job(3, Download, TaskState::Done)];
    assert_eq!(dock(&mixed, false), Dock { count: 2, bar: Bar::Busy });
    let mut offline = job(2, Download, TaskState::Running);
    offline.waiting_for_connection = true;
    let waiting = [job(1, Download, TaskState::Paused), offline.clone()];
    assert_eq!(dock(&waiting, false), Dock { count: 2, bar: Bar::Paused });

    // A name clash waits on the user: amber even while others run.
    let mut asking = job(2, Upload, TaskState::Preparing { files_found: 0 });
    asking.conflict = Some(clash());
    assert_eq!(dock(&[job(1, Download, TaskState::Running), asking], false), Dock { count: 2, bar: Bar::Paused });

    // A failure nobody saw yet is red, over anything else.
    assert_eq!(dock(&running, true), Dock { count: 3, bar: Bar::Error });
    assert_eq!(dock(&finished, true), Dock { count: 0, bar: Bar::Error });
}

#[test]
fn the_badge_tells_a_screen_reader_what_it_counts() {
    assert_eq!(Dock { count: 0, bar: Bar::None }.description(), "");
    assert_eq!(Dock { count: 1, bar: Bar::Busy }.description(), "1 transfer in progress");
    assert_eq!(Dock { count: 12, bar: Bar::Busy }.description(), "12 transfers in progress");
}

#[test]
fn each_kind_of_job_says_what_it_did_or_why_it_could_not() {
    use JobKind::*;
    let cases = [
        (titled(Download, "report.pdf", TaskState::Done), "Downloaded report.pdf", ""),
        (titled(Upload, "3 items", TaskState::Done), "Uploaded 3 items to prod", ""),
        (titled(RemoteCopy, "site", TaskState::Done), "Copied site on prod", ""),
        (titled(Move, "old", TaskState::Done), "Moved old on prod", ""),
        (titled(CrossHostCopy, "backup.tar", TaskState::Done), "Copied backup.tar to prod", ""),
        (titled(CrossHostMove, "logs", TaskState::Done), "Moved logs to prod", ""),
        (titled(Download, "syslog", failed("Permission denied")), "Couldn't download syslog", "Permission denied"),
        (titled(Upload, "hero.png", failed("Disk full")), "Couldn't upload hero.png to prod", "Disk full"),
        (titled(RemoteCopy, "site", failed("1 file failed")), "Couldn't copy site on prod", "1 file failed"),
        (titled(Move, "old", failed("x")), "Couldn't move old on prod", "x"),
        (titled(CrossHostCopy, "a", failed("y")), "Couldn't copy a to prod", "y"),
        (titled(CrossHostMove, "b", failed("z")), "Couldn't move b to prod", "z"),
    ];
    for (j, title, body) in cases {
        let n = notice(&j).unwrap_or_else(|| panic!("{:?} {:?} says something", j.kind, j.state));
        assert_eq!((n.title.as_str(), n.body.as_str()), (title, body), "{:?}", j.kind);
        assert_eq!((n.id.as_str(), n.job), ("transfer-1", JobId(1)));
    }
    let n = notice(&titled(Download, "syslog", failed("Permission denied"))).unwrap();
    assert_eq!(n.line(), "Couldn't download syslog: Permission denied");
    assert_eq!(notice(&titled(Download, "report.pdf", TaskState::Done)).unwrap().line(), "Downloaded report.pdf");

    // What was left out is the body.
    let mut j = titled(Download, "photos", TaskState::Done);
    j.skipped = vec!["a".into()];
    assert_eq!(notice(&j).unwrap().body, "1 item skipped");
    j.skipped = (0..1204).map(|i| format!("f{i}")).collect();
    assert_eq!(notice(&j).unwrap().body, "1,204 items skipped");

    // A cancel is the user's own doing; a job still going says nothing.
    for state in [TaskState::Cancelled, TaskState::Running, TaskState::Queued, TaskState::Paused] {
        assert_eq!(notice(&titled(Download, "x", state.clone())), None, "{state:?}");
    }
}

#[test]
fn only_a_long_job_ending_out_of_sight_is_worth_telling() {
    assert!(worth_telling(10_001, false));
    assert!(!worth_telling(10_000, false), "more than 10 s");
    assert!(!worth_telling(3_000, false));
    assert!(!worth_telling(60_000, true), "the user is looking");
}

/// The notifications held so far, as one line each.
fn told(w: &mut Watch) -> Vec<String> {
    w.os.take().into_iter().filter_map(|fx| if let OsEffect::Notify(n) = fx { Some(n.line()) } else { None }).collect()
}

/// The taskbar states held so far.
fn docks(w: &mut Watch) -> Vec<Dock> {
    w.os.take().into_iter().filter_map(|fx| if let OsEffect::Taskbar(d) = fx { Some(d) } else { None }).collect()
}

#[test]
fn a_job_that_ran_more_than_10_s_while_unfocused_is_notified_once() {
    let mut w = Watch::default();
    let start = vec![titled(JobKind::Download, "report.pdf", TaskState::Running)];
    w.update(&[], &start, 1_000);
    let mut later = start.clone();
    later[0].progress.done = 5;
    w.update(&start, &later, 6_000);
    let done = vec![titled(JobKind::Download, "report.pdf", TaskState::Done)];
    w.update(&later, &done, 11_001);
    assert_eq!(told(&mut w), ["Downloaded report.pdf"]);
    // The same snapshot again tells nothing more.
    w.update(&done, &done, 20_000);
    assert!(told(&mut w).is_empty());
}

#[test]
fn a_short_job_a_focused_window_and_a_job_that_arrives_finished_tell_nothing() {
    let run = |kind| vec![titled(kind, "a", TaskState::Running)];
    let end = |kind| vec![titled(kind, "a", TaskState::Done)];
    // Short.
    let mut w = Watch::default();
    w.update(&[], &run(JobKind::Upload), 0);
    w.update(&run(JobKind::Upload), &end(JobKind::Upload), 10_000);
    assert!(told(&mut w).is_empty());
    // Long, but the user is looking.
    let mut w = Watch::default();
    w.window_focus(true, &[]);
    w.update(&[], &run(JobKind::Upload), 0);
    w.update(&run(JobKind::Upload), &end(JobKind::Upload), 30_000);
    assert!(told(&mut w).is_empty());
    // Already over in the first snapshot the app sees: never timed.
    let mut w = Watch::default();
    w.update(&[], &end(JobKind::Download), 30_000);
    assert!(told(&mut w).is_empty());
    // Cancelled after a minute.
    let mut w = Watch::default();
    w.update(&[], &run(JobKind::Download), 0);
    w.update(&run(JobKind::Download), &[titled(JobKind::Download, "a", TaskState::Cancelled)], 60_000);
    assert!(told(&mut w).is_empty());
}

#[test]
fn a_retried_job_is_timed_from_its_retry() {
    let mut w = Watch::default();
    let run = vec![titled(JobKind::Download, "syslog", TaskState::Running)];
    let fail = vec![titled(JobKind::Download, "syslog", failed("Connection lost"))];
    w.update(&[], &run, 0);
    w.update(&run, &fail, 20_000);
    assert_eq!(told(&mut w), ["Couldn't download syslog: Connection lost"]);
    // Retried at 100 s, done 5 s later: short.
    w.update(&fail, &run, 100_000);
    w.update(&run, &[titled(JobKind::Download, "syslog", TaskState::Done)], 105_000);
    assert!(told(&mut w).is_empty());
}

#[test]
fn the_taskbar_changes_only_when_what_it_shows_does_and_a_failure_stays_red_until_seen() {
    let mut w = Watch::default();
    let a = |state| job(1, JobKind::Download, state);
    let b = |state| job(2, JobKind::Upload, state);
    // Nothing shown, nothing to say.
    w.update(&[], &[], 0);
    assert!(docks(&mut w).is_empty());
    let both = vec![a(TaskState::Running), b(TaskState::Running)];
    w.update(&[], &both, 0);
    assert_eq!(docks(&mut w), [Dock { count: 2, bar: Bar::Busy }]);
    // Progress alone changes nothing on the taskbar.
    let mut moved = both.clone();
    moved[0].progress.done = 9;
    w.update(&both, &moved, 500);
    assert!(docks(&mut w).is_empty());

    // One fails while the window is in the background: red, with one left.
    let one_failed = vec![a(failed("x")), b(TaskState::Running)];
    w.update(&moved, &one_failed, 1_000);
    assert_eq!(docks(&mut w), [Dock { count: 1, bar: Bar::Error }]);
    let all_over = vec![a(failed("x")), b(TaskState::Done)];
    w.update(&one_failed, &all_over, 2_000);
    assert_eq!(docks(&mut w), [Dock { count: 0, bar: Bar::Error }]);
    // Losing focus again changes nothing; gaining it is the user looking.
    w.window_focus(false, &all_over);
    assert!(docks(&mut w).is_empty());
    w.window_focus(true, &all_over);
    assert_eq!(docks(&mut w), [Dock { count: 0, bar: Bar::None }]);
    assert_eq!(w.shown(), &Dock::default());

    // A failure while focused never turns it red; clearing the failed job takes the red away.
    let mut w = Watch::default();
    w.window_focus(true, &[]);
    w.update(&[], &[a(TaskState::Running)], 0);
    w.update(&[a(TaskState::Running)], &[a(failed("x"))], 1_000);
    assert_eq!(docks(&mut w), [Dock { count: 1, bar: Bar::Busy }, Dock { count: 0, bar: Bar::None }]);
    let mut w = Watch::default();
    w.update(&[], &[a(TaskState::Running)], 0);
    w.update(&[a(TaskState::Running)], &[a(failed("x"))], 1_000);
    w.update(&[a(failed("x"))], &[], 2_000);
    assert_eq!(docks(&mut w).last(), Some(&Dock::default()));
}

#[test]
fn a_connected_sink_gets_the_effects_and_nothing_is_held() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let got: Rc<RefCell<Vec<OsEffect>>> = Rc::default();
    let mut w = Watch::default();
    let sink = got.clone();
    w.os.connect(move |fx| sink.borrow_mut().push(fx));
    w.update(&[], &[job(1, JobKind::Download, TaskState::Queued)], 0);
    assert_eq!(*got.borrow(), [OsEffect::Taskbar(Dock { count: 1, bar: Bar::Busy })]);
    assert!(w.os.held.is_empty());
}

#[test]
fn without_a_sink_only_the_newest_effects_are_held() {
    let mut w = Watch::default();
    let jobs = |n: u64| (0..n).map(|i| job(i, JobKind::Download, TaskState::Running)).collect::<Vec<_>>();
    let mut before = Vec::new();
    for n in 1..=100 {
        let now = jobs(n);
        w.update(&before, &now, 0);
        before = now;
    }
    assert_eq!(w.os.held.len(), 64);
    assert_eq!(w.os.held.last(), Some(&OsEffect::Taskbar(Dock { count: 100, bar: Bar::Busy })));
}
