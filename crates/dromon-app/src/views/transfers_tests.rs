//! What a transfer row says (§6.8).

use std::time::Duration;

use dromon_core::{HostId, RemotePath};
use dromon_transfer::{ConflictView, JobId, JobKind, JobView, Progress, TaskState};

use super::{details, status_line};

fn job(state: TaskState, done: u64, total: u64) -> JobView {
    JobView {
        id: JobId(1),
        kind: JobKind::Download,
        host: HostId("prod".into()),
        title: "photos".into(),
        state,
        progress: Progress { done, total },
        files_done: 0,
        files_total: 3,
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

#[test]
fn each_state_has_its_line() {
    let mut running = job(TaskState::Running, 12_400_000, 80_000_000);
    running.speed = Some(8_100_000.0);
    running.eta = Some(Duration::from_secs(9));
    let cases = [
        (running, "12.4 / 80 MB · 8.1 MB/s · about 9 s"),
        (job(TaskState::Running, 0, 80_000_000), "0 / 80 MB"),
        (job(TaskState::Queued, 0, 0), "Queued"),
        (job(TaskState::Preparing { files_found: 0 }, 0, 0), "Preparing…"),
        (job(TaskState::Preparing { files_found: 1 }, 0, 0), "Preparing… 1 file"),
        (job(TaskState::Preparing { files_found: 1204 }, 0, 0), "Preparing… 1,204 files"),
        (job(TaskState::Paused, 5, 10), "Paused"),
        (job(TaskState::Done, 80_000_000, 80_000_000), "Done · 80 MB"),
        (job(TaskState::Failed { error: "Permission denied".into() }, 0, 9), "Failed: Permission denied"),
        (job(TaskState::Cancelled, 3, 9), "Cancelled"),
    ];
    for (j, want) in cases {
        assert_eq!(status_line(&j), want, "{:?}", j.state);
    }
    let mut nothing = job(TaskState::Done, 0, 0);
    nothing.files_total = 0;
    assert_eq!(status_line(&nothing), "Done");
}

#[test]
fn a_failed_row_says_why_not_how_many() {
    let failed = |title: &str, errors: &[&str]| JobView {
        title: title.into(),
        errors: errors.iter().map(|e| (*e).to_owned()).collect(),
        ..job(TaskState::Failed { error: format!("{} files failed", errors.len()) }, 0, 9)
    };
    // One file: its error, without its name again when it's the row's title.
    let denied = "You don't have permission to write in /home/u/.gradle";
    let pem = format!("key.pem: {denied}");
    assert_eq!(status_line(&failed("key.pem", &[&pem])), format!("Failed: {denied}"));
    // One file of a folder: its name says which.
    assert_eq!(status_line(&failed("photos", &["a.jpg: denied"])), "Failed: a.jpg: denied");
    // Several: how many, and the first.
    let several = failed("photos", &["a.jpg: denied", "b.jpg: denied", "c.jpg: gone"]);
    assert_eq!(status_line(&several), "3 files failed: a.jpg: denied");
    // Some files done, some failed: still a failure, with why (the queue reports it Failed).
    let mut partial = failed("site", &["a.html: denied", "b.css: denied"]);
    (partial.files_done, partial.files_total, partial.progress) = (3, 5, Progress { done: 6, total: 9 });
    partial.skipped = vec!["cache: a link to a folder".into()];
    assert_eq!(status_line(&partial), "2 files failed: a.html: denied · 1 skipped");
    // The tooltip still lists them all.
    assert_eq!(details(&several).unwrap(), "a.jpg: denied\nb.jpg: denied\nc.jpg: gone");
}

#[test]
fn waiting_and_clashes_say_so_before_the_numbers() {
    let mut j = job(TaskState::Running, 1, 2);
    j.waiting_for_connection = true;
    assert_eq!(status_line(&j), "Waiting for connection");
    j.state = TaskState::Paused;
    assert_eq!(status_line(&j), "Paused", "the user's pause wins");
    let mut j = job(TaskState::Preparing { files_found: 1 }, 0, 0);
    j.conflict = Some(ConflictView {
        name: "index.html".into(),
        folder: RemotePath::new("/var/www"),
        existing_is_dir: false,
        existing: Default::default(),
        incoming: Default::default(),
        more: 0,
    });
    assert_eq!(status_line(&j), "Waiting for you: ‘index.html’ already exists");
}

#[test]
fn what_was_left_out_shows_in_the_line_and_the_tooltip() {
    let mut j = job(TaskState::Done, 5, 5);
    assert_eq!(details(&j), None);
    j.skipped = vec!["cache: a link to a folder".into(), "sock: not a regular file".into()];
    assert_eq!(status_line(&j), "Done · 5 bytes · 2 skipped");
    j.errors = vec!["a.txt: Permission denied".into()];
    assert_eq!(details(&j).unwrap(), "a.txt: Permission denied\ncache: a link to a folder\nsock: not a regular file");
    j.skipped = (0..20).map(|i| format!("f{i}")).collect();
    let d = details(&j).unwrap();
    assert_eq!(d.lines().count(), 9);
    assert!(d.ends_with("and 13 more"), "{d}");
}
