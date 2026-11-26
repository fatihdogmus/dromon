use dromon_core::EntryKind;

use super::*;

const T0: Duration = Duration::ZERO;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn host(name: &str) -> HostId {
    HostId(name.to_owned())
}

fn remote_item(path: &str, size: u64) -> crate::RemoteItem {
    crate::RemoteItem { path: RemotePath::new(path), kind: EntryKind::File, size }
}

fn download(h: &str, names: &[&str]) -> JobRequest {
    JobRequest::Download {
        host: host(h),
        items: names.iter().map(|n| remote_item(&format!("/r/{n}"), 0)).collect(),
        into: PathBuf::from("/d"),
    }
}

fn upload(h: &str, names: &[&str]) -> JobRequest {
    JobRequest::Upload {
        host: host(h),
        items: names.iter().map(|n| PathBuf::from(format!("/home/u/{n}"))).collect(),
        into: RemotePath::new("/srv"),
    }
}

/// A file with no known size or date is at the destination.
fn file() -> Option<Existing> {
    Some(Existing { is_dir: false, side: Side::default() })
}

fn dl_plan(name: &str, size: u64) -> FilePlan {
    FilePlan::Download { src: RemotePath::new(&format!("/r/{name}")), dst: PathBuf::from(format!("/d/{name}")), size, mtime: None }
}

fn up_plan(name: &str, size: u64) -> FilePlan {
    FilePlan::Upload {
        src: PathBuf::from(format!("/home/u/{name}")),
        dst: RemotePath::new(&format!("/srv/{name}")),
        size,
        mtime: None,
        overwrite: false,
    }
}

fn scan(files: Vec<FilePlan>) -> Scan {
    Scan { files, ..Scan::default() }
}

fn local(name: &str) -> ItemDst {
    ItemDst::Local(PathBuf::from(format!("/d/{name}")))
}

fn starts(actions: &[Action]) -> Vec<(JobId, FileId)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Start { job, file, .. } => Some((*job, *file)),
            _ => None,
        })
        .collect()
}

/// A download job on an up host whose items were all named and scanned into `files[item]`.
/// The job is Running afterwards (when there are files); nothing has been started yet.
fn running_download(q: &mut Queue, h: &str, names: &[&str], files: &[&[(&str, u64)]]) -> JobId {
    q.host_up(&host(h));
    let id = q.add(download(h, names), T0);
    q.poll(T0);
    for (n, name) in names.iter().enumerate() {
        q.named(id, n, local(name));
    }
    q.poll(T0);
    for (n, fs) in files.iter().enumerate() {
        q.scanned(id, n, scan(fs.iter().map(|(f, s)| dl_plan(f, *s)).collect()));
    }
    id
}

fn view(q: &Queue, id: JobId) -> JobView {
    q.view(id, T0).unwrap()
}

#[test]
fn single_file_download_end_to_end() {
    let mut q = Queue::new(3);
    let h = host("a");
    let id = q.add(download("a", &["report.pdf"]), T0);
    assert_eq!(id, JobId(1));
    assert_eq!(view(&q, id).state, TaskState::Queued);

    // Hosts start down: nothing happens.
    assert!(q.poll(T0).is_empty());
    q.host_up(&h);
    assert_eq!(q.poll(T0), vec![Action::Name { job: id, item: 0 }]);
    assert_eq!(view(&q, id).state, TaskState::Preparing { files_found: 0 });

    q.named(id, 0, local("report.pdf"));
    // `named` returns nothing; the scan comes with the next poll.
    assert_eq!(q.poll(T0), vec![Action::Scan { job: id, item: 0, dst: local("report.pdf") }]);
    assert_eq!(view(&q, id).reveal, Some(Reveal::Local(PathBuf::from("/d/report.pdf"))));

    q.scan_progress(id, 0, 1);
    assert_eq!(view(&q, id).state, TaskState::Preparing { files_found: 1 });
    q.scanned(id, 0, scan(vec![dl_plan("report.pdf", 1000)]));
    assert_eq!(view(&q, id).state, TaskState::Running);
    assert_eq!(view(&q, id).progress, Progress { done: 0, total: 1000 });

    let actions = q.poll(T0);
    assert_eq!(actions, vec![Action::Start { job: id, file: FileId(0), plan: dl_plan("report.pdf", 1000) }]);
    assert_eq!(q.plan(id, FileId(0)), Some(&dl_plan("report.pdf", 1000)));

    q.file_progress(id, FileId(0), 400, secs(1));
    let v = view(&q, id);
    assert_eq!((v.progress.done, v.files_done, v.files_total), (400, 0, 1));
    q.file_done(id, FileId(0), secs(2));
    let v = view(&q, id);
    assert_eq!(v.state, TaskState::Done);
    assert_eq!((v.progress, v.files_done, v.title.as_str()), (Progress { done: 1000, total: 1000 }, 1, "report.pdf"));
    assert!(v.errors.is_empty());
    assert_eq!(q.active_count(), 0);
    assert!(q.poll(secs(3)).is_empty());
}

#[test]
fn folder_download_two_items_file_ids_across_items() {
    let mut q = Queue::new(10);
    let id = running_download(&mut q, "a", &["photos", "notes.txt"], &[&[("p1", 10), ("p2", 20)], &[("n", 5)]]);
    let v = view(&q, id);
    assert_eq!((v.title.as_str(), v.files_total, v.progress.total), ("2 items", 3, 35));
    assert_eq!(q.plan(id, FileId(0)), Some(&dl_plan("p1", 10)));
    assert_eq!(q.plan(id, FileId(2)), Some(&dl_plan("n", 5)));
    assert_eq!(q.plan(id, FileId(3)), None);
    assert_eq!(starts(&q.poll(T0)), vec![(id, FileId(0)), (id, FileId(1)), (id, FileId(2))]);
}

#[test]
fn job_waits_for_every_item_to_be_scanned() {
    let mut q = Queue::new(3);
    let id = running_download(&mut q, "a", &["x", "y"], &[&[("x1", 1)]]);
    assert!(matches!(view(&q, id).state, TaskState::Preparing { .. }));
    assert!(starts(&q.poll(T0)).is_empty());
    q.scanned(id, 1, scan(vec![dl_plan("y1", 1)]));
    assert_eq!(view(&q, id).state, TaskState::Running);
}

#[test]
fn per_host_limit_and_next_file_after_one_finishes() {
    let mut q = Queue::new(2);
    let files: Vec<(String, u64)> = (0..5).map(|n| (format!("f{n}"), 10)).collect();
    let refs: Vec<(&str, u64)> = files.iter().map(|(n, s)| (n.as_str(), *s)).collect();
    let id = running_download(&mut q, "a", &["dir"], &[&refs]);
    assert_eq!(starts(&q.poll(T0)), vec![(id, FileId(0)), (id, FileId(1))]);
    assert!(q.poll(T0).is_empty());
    q.file_done(id, FileId(0), secs(1));
    assert_eq!(starts(&q.poll(secs(1))), vec![(id, FileId(2))]);
    q.file_failed(id, FileId(1), "boom".into(), secs(1));
    assert_eq!(starts(&q.poll(secs(1))), vec![(id, FileId(3))]);
}

#[test]
fn the_limit_is_per_host_across_jobs_and_hosts_are_independent() {
    let mut q = Queue::new(2);
    let a1 = running_download(&mut q, "a", &["d"], &[&[("a1", 1), ("a2", 1)]]);
    let a2 = running_download(&mut q, "a", &["e"], &[&[("a3", 1)]]);
    let b = running_download(&mut q, "b", &["f"], &[&[("b1", 1), ("b2", 1), ("b3", 1)]]);
    // (The helper's own polls already started a1's two files.) Host b has its own budget.
    assert_eq!(q.plan(a1, FileId(1)), Some(&dl_plan("a2", 1)));
    let started = starts(&q.poll(T0));
    assert_eq!(started, vec![(b, FileId(0)), (b, FileId(1))]);
    q.file_done(a1, FileId(0), secs(1));
    assert_eq!(starts(&q.poll(secs(1))), vec![(a2, FileId(0))]);
    q.file_done(b, FileId(0), secs(1));
    assert_eq!(starts(&q.poll(secs(1))), vec![(b, FileId(2))]);
}

#[test]
fn a_changed_limit_applies_to_new_starts_and_running_files_keep_going() {
    let mut q = Queue::new(2);
    let files: Vec<(String, u64)> = (0..8).map(|n| (format!("f{n}"), 10)).collect();
    let refs: Vec<(&str, u64)> = files.iter().map(|(n, s)| (n.as_str(), *s)).collect();
    let id = running_download(&mut q, "a", &["dir"], &[&refs]);
    assert_eq!(starts(&q.poll(T0)), vec![(id, FileId(0)), (id, FileId(1))]);
    // Raised: the next poll fills up to the new limit.
    q.set_per_host(4);
    assert_eq!(starts(&q.poll(T0)), vec![(id, FileId(2)), (id, FileId(3))]);
    // Lowered: nothing running is stopped, and nothing new starts until fewer than 1 run.
    q.set_per_host(1);
    assert!(q.poll(T0).is_empty());
    assert_eq!(view(&q, id).state, TaskState::Running);
    for f in 0..3 {
        q.file_done(id, FileId(f), secs(1));
        assert!(q.poll(secs(1)).is_empty(), "{} still run", 3 - f);
    }
    q.file_done(id, FileId(3), secs(1));
    assert_eq!(starts(&q.poll(secs(1))), vec![(id, FileId(4))]);
    // Zero would stop everything for good: it counts as 1.
    q.set_per_host(0);
    assert_eq!(q.per_host(), 1);
    q.file_done(id, FileId(4), secs(2));
    assert_eq!(starts(&q.poll(secs(2))), vec![(id, FileId(5))]);
}

#[test]
fn nothing_starts_while_the_host_is_down() {
    let mut q = Queue::new(3);
    let id = running_download(&mut q, "a", &["d"], &[&[("f", 1)]]);
    q.host_down(&host("a"));
    assert!(q.poll(T0).is_empty());
    // A job of a down host is not even named.
    let other = q.add(download("a", &["z"]), T0);
    assert!(q.poll(T0).is_empty());
    assert_eq!(view(&q, other).state, TaskState::Queued);
    q.host_up(&host("a"));
    let actions = q.poll(T0);
    assert_eq!(actions.len(), 2);
    assert!(actions.contains(&Action::Name { job: other, item: 0 }));
    assert_eq!(starts(&actions), vec![(id, FileId(0))]);
}

#[test]
fn upload_without_clash_scans_into_the_folder() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(upload("a", &["a.txt"]), T0);
    assert_eq!(q.poll(T0), vec![Action::Name { job: id, item: 0 }]);
    q.exists(id, 0, None, Side::default());
    let dst = ItemDst::Remote { path: RemotePath::new("/srv/a.txt"), overwrite: false };
    assert_eq!(q.poll(T0), vec![Action::Scan { job: id, item: 0, dst }]);
    assert_eq!(view(&q, id).reveal, Some(Reveal::Remote(host("a"), RemotePath::new("/srv/a.txt"))));
    assert_eq!(view(&q, id).conflict, None);
    q.scanned(id, 0, scan(vec![up_plan("a.txt", 7)]));
    assert_eq!(q.poll(T0), vec![Action::Start { job: id, file: FileId(0), plan: up_plan("a.txt", 7) }]);
    q.file_done(id, FileId(0), secs(1));
    assert_eq!(view(&q, id).state, TaskState::Done);
}

#[test]
fn upload_conflicts_are_shown_one_at_a_time() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(upload("a", &["a", "b", "c"]), T0);
    q.poll(T0);
    let (old, new) = (Side { size: Some(10), mtime: Some(100) }, Side { size: Some(20), mtime: Some(200) });
    let folder = Side { size: None, mtime: Some(300) };
    q.exists(id, 0, Some(Existing { is_dir: false, side: old }), new);
    q.exists(id, 1, Some(Existing { is_dir: true, side: folder }), folder);
    q.exists(id, 2, None, Side::default());
    let c = view(&q, id).conflict.unwrap();
    assert_eq!(
        c,
        ConflictView {
            name: "a".into(),
            folder: RemotePath::new("/srv"),
            existing_is_dir: false,
            existing: old,
            incoming: new,
            more: 1
        }
    );
    // Replace the first: the next one comes up, with its own sizes and dates.
    q.resolve(id, ConflictChoice::Replace, false);
    let c = view(&q, id).conflict.unwrap();
    assert_eq!((c.name.as_str(), c.existing_is_dir, c.more), ("b", true, 0));
    assert_eq!((c.existing, c.incoming), (folder, folder));
    assert_eq!(
        q.poll(T0),
        vec![
            Action::Scan {
                job: id,
                item: 2,
                dst: ItemDst::Remote { path: RemotePath::new("/srv/c"), overwrite: false }
            },
            Action::Scan {
                job: id,
                item: 0,
                dst: ItemDst::Remote { path: RemotePath::new("/srv/a"), overwrite: true }
            },
        ]
    );
}

#[test]
fn keep_both_asks_for_a_free_name_then_scans() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(upload("a", &["a.txt"]), T0);
    q.poll(T0);
    q.exists(id, 0, file(), Side::default());
    assert!(q.poll(T0).is_empty()); // waiting for the user
    q.resolve(id, ConflictChoice::KeepBoth, false);
    assert_eq!(view(&q, id).conflict, None);
    assert_eq!(q.poll(T0), vec![Action::FreeRemoteName { job: id, item: 0 }]);
    let dst = ItemDst::Remote { path: RemotePath::new("/srv/a (1).txt"), overwrite: false };
    q.named(id, 0, dst.clone());
    assert_eq!(q.poll(T0), vec![Action::Scan { job: id, item: 0, dst }]);
}

#[test]
fn skip_leaves_the_item_out_and_an_all_skipped_job_is_done() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(upload("a", &["a.txt"]), T0);
    q.poll(T0);
    q.exists(id, 0, file(), Side::default());
    q.resolve(id, ConflictChoice::Skip, false);
    let v = view(&q, id);
    assert_eq!(v.skipped, vec!["a.txt".to_owned()]);
    assert_eq!(v.state, TaskState::Done);
    assert_eq!(v.conflict, None);
}

#[test]
fn resolve_without_a_conflict_is_ignored() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(upload("a", &["a.txt"]), T0);
    q.poll(T0);
    q.resolve(id, ConflictChoice::Skip, true);
    q.exists(id, 0, file(), Side::default()); // apply_all was not stored
    assert!(view(&q, id).conflict.is_some());
}

#[test]
fn apply_all_answers_other_conflicts_now_and_later_ones() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(upload("a", &["a", "b", "c", "d"]), T0);
    q.poll(T0);
    q.exists(id, 0, file(), Side::default());
    q.exists(id, 1, file(), Side::default());
    q.resolve(id, ConflictChoice::Skip, true);
    assert_eq!(view(&q, id).conflict, None);
    assert_eq!(view(&q, id).skipped, vec!["a".to_owned(), "b".to_owned()]);
    // A later clash is answered at once.
    q.exists(id, 2, Some(Existing { is_dir: true, side: Side::default() }), Side::default());
    assert_eq!(view(&q, id).skipped.len(), 3);
    assert_eq!(view(&q, id).conflict, None);

    // apply_all with Replace answers later items with Replace.
    let id2 = q.add(upload("a", &["x", "y"]), T0);
    q.poll(T0);
    q.exists(id2, 0, file(), Side::default());
    q.resolve(id2, ConflictChoice::Replace, true);
    q.exists(id2, 1, file(), Side::default());
    let scans: Vec<_> = q.poll(T0).into_iter().filter(|a| matches!(a, Action::Scan { job, .. } if *job == id2)).collect();
    assert_eq!(scans.len(), 2);
    assert!(scans.iter().all(|a| matches!(a, Action::Scan { dst: ItemDst::Remote { overwrite: true, .. }, .. })));
}

#[test]
fn a_job_with_no_files_is_done() {
    let mut q = Queue::new(3);
    let id = running_download(&mut q, "a", &["empty"], &[&[]]);
    let v = view(&q, id);
    assert_eq!(v.state, TaskState::Done);
    assert_eq!(v.files_total, 0);
    assert!(q.poll(T0).is_empty());

    // A job with no items at all is done on the first poll.
    let none = q.add(download("a", &[]), T0);
    q.poll(T0);
    assert_eq!(view(&q, none).state, TaskState::Done);
}

#[test]
fn item_failures() {
    let mut q = Queue::new(3);
    // Only failures: the job fails with the first error.
    let id = running_download(&mut q, "a", &["x", "y"], &[]);
    q.item_failed(id, 0, "permission denied".into());
    assert!(matches!(view(&q, id).state, TaskState::Preparing { .. }));
    q.item_failed(id, 1, "gone".into());
    let v = view(&q, id);
    assert_eq!(v.state, TaskState::Failed { error: "x: permission denied".into() });
    assert_eq!(v.errors, vec!["x: permission denied".to_owned(), "y: gone".to_owned()]);

    // One item fails, the other has files: the job runs, then fails.
    let id = running_download(&mut q, "b", &["x", "y"], &[]);
    q.item_failed(id, 0, "nope".into());
    q.scanned(id, 1, scan(vec![dl_plan("y1", 1)]));
    assert_eq!(view(&q, id).state, TaskState::Running);
    q.poll(T0);
    q.file_done(id, FileId(0), secs(1));
    assert_eq!(view(&q, id).state, TaskState::Failed { error: "x: nope".into() });
}

#[test]
fn file_failure_then_retry_reruns_only_failed_files() {
    let mut q = Queue::new(5);
    let id = running_download(&mut q, "a", &["d"], &[&[("a", 100), ("b", 100), ("c", 100)]]);
    q.poll(T0);
    q.file_done(id, FileId(0), secs(1));
    q.file_progress(id, FileId(1), 60, secs(1));
    q.file_failed(id, FileId(1), "disk full".into(), secs(1));
    assert_eq!(view(&q, id).state, TaskState::Running);
    q.file_done(id, FileId(2), secs(1));
    let v = view(&q, id);
    assert_eq!(v.state, TaskState::Failed { error: "1 file failed".into() });
    assert_eq!(v.errors, vec!["b: disk full".to_owned()]);
    assert_eq!(v.progress.done, 260);
    assert!(q.poll(secs(2)).is_empty());

    q.retry(id);
    let v = view(&q, id);
    assert_eq!(v.state, TaskState::Running);
    assert!(v.errors.is_empty());
    assert_eq!(v.progress.done, 260); // `done` kept
    assert_eq!(starts(&q.poll(secs(2))), vec![(id, FileId(1))]);
    q.file_done(id, FileId(1), secs(3));
    assert_eq!(view(&q, id).state, TaskState::Done);

    q.retry(id); // not Failed: ignored
    assert_eq!(view(&q, id).state, TaskState::Done);
}

#[test]
fn several_failed_files_are_counted() {
    let mut q = Queue::new(5);
    let id = running_download(&mut q, "a", &["d"], &[&[("a", 1), ("b", 1)]]);
    q.poll(T0);
    q.file_failed(id, FileId(0), "x".into(), T0);
    q.file_failed(id, FileId(1), "y".into(), T0);
    assert_eq!(view(&q, id).state, TaskState::Failed { error: "2 files failed".into() });
}

#[test]
fn retry_names_failed_items_again_without_rescanning_the_others() {
    let mut q = Queue::new(5);
    let id = running_download(&mut q, "a", &["x", "y"], &[]);
    q.scanned(id, 0, scan(vec![dl_plan("x1", 10)]));
    q.item_failed(id, 1, "denied".into());
    q.poll(T0);
    q.file_done(id, FileId(0), secs(1));
    assert!(matches!(view(&q, id).state, TaskState::Failed { .. }));

    q.retry(id);
    assert!(matches!(view(&q, id).state, TaskState::Preparing { files_found: 1 }));
    assert_eq!(q.poll(T0), vec![Action::Name { job: id, item: 1 }]);
    q.named(id, 1, local("y"));
    q.poll(T0);
    q.scanned(id, 1, scan(vec![dl_plan("y1", 5)]));
    assert_eq!(starts(&q.poll(T0)), vec![(id, FileId(1))]); // x1 is Done and does not re-run
}

#[test]
fn pause_running_aborts_files_and_resume_restarts_them() {
    let mut q = Queue::new(2);
    let id = running_download(&mut q, "a", &["d"], &[&[("a", 100), ("b", 100), ("c", 100)]]);
    q.poll(T0);
    q.file_progress(id, FileId(0), 50, secs(1));
    let actions = q.pause(id);
    assert_eq!(actions, vec![Action::Abort { job: id, file: FileId(0) }, Action::Abort { job: id, file: FileId(1) }]);
    assert_eq!(view(&q, id).state, TaskState::Paused);
    assert_eq!(view(&q, id).progress.done, 50);
    assert!(q.poll(secs(2)).is_empty());
    assert_eq!(q.active_count(), 1);
    assert!(q.pause(id).is_empty());

    q.resume(id);
    assert_eq!(view(&q, id).state, TaskState::Running);
    assert_eq!(starts(&q.poll(secs(3))), vec![(id, FileId(0)), (id, FileId(1))]);
    // A paused job is not waiting for the connection.
    q.pause(id);
    q.host_down(&host("a"));
    assert!(!view(&q, id).waiting_for_connection);
}

#[test]
fn pause_while_preparing_resumes_to_preparing() {
    let mut q = Queue::new(3);
    let id = running_download(&mut q, "a", &["x", "y"], &[&[("x1", 1)]]);
    assert!(q.pause(id).is_empty());
    assert_eq!(view(&q, id).state, TaskState::Paused);
    q.resume(id);
    assert_eq!(view(&q, id).state, TaskState::Preparing { files_found: 1 });

    // The engine may report a scan while paused: it is recorded, nothing starts.
    q.pause(id);
    q.scanned(id, 1, scan(vec![dl_plan("y1", 1)]));
    assert_eq!(view(&q, id).state, TaskState::Paused);
    assert!(q.poll(T0).is_empty());
    q.resume(id);
    assert_eq!(view(&q, id).state, TaskState::Running);
    assert_eq!(starts(&q.poll(T0)).len(), 2);
}

#[test]
fn parked_scans_wait_while_paused() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(download("a", &["x"]), T0);
    q.poll(T0);
    q.pause(id);
    q.named(id, 0, local("x"));
    assert!(q.poll(T0).is_empty());
    q.resume(id);
    assert_eq!(q.poll(T0), vec![Action::Scan { job: id, item: 0, dst: local("x") }]);
}

#[test]
fn cancel_aborts_cleans_up_and_ignores_later_facts() {
    let mut q = Queue::new(2);
    let id = running_download(&mut q, "a", &["d"], &[&[("a", 100), ("b", 100), ("c", 100), ("d", 100)]]);
    q.poll(T0);
    q.file_done(id, FileId(0), secs(1)); // Done: no cleanup
    q.poll(secs(1)); // starts file 2
    q.file_progress(id, FileId(1), 30, secs(1));
    q.file_interrupted(id, FileId(1)); // Pending with done > 0
    q.poll(secs(1)); // file 1 again, file 2 is pending without progress
    let actions = q.cancel(id);
    assert_eq!(view(&q, id).state, TaskState::Cancelled);
    assert_eq!(actions.len(), 3, "{actions:?}");
    assert!(actions.contains(&Action::Abort { job: id, file: FileId(1) }));
    assert!(actions.contains(&Action::Abort { job: id, file: FileId(2) }));
    assert_eq!(actions[2], Action::Cleanup { job: id, plans: vec![dl_plan("b", 100), dl_plan("c", 100)] });

    q.file_done(id, FileId(1), secs(2));
    q.file_failed(id, FileId(2), "x".into(), secs(2));
    q.scanned(id, 0, scan(vec![dl_plan("z", 1)]));
    q.resume(id);
    let v = view(&q, id);
    assert_eq!((v.state, v.errors.len(), v.files_done), (TaskState::Cancelled, 0, 1));
    assert!(q.poll(secs(2)).is_empty());
    assert!(q.cancel(id).is_empty());
}

#[test]
fn cancel_before_anything_ran_has_no_cleanup() {
    let mut q = Queue::new(2);
    q.host_up(&host("a"));
    let id = q.add(download("a", &["x"]), T0);
    assert!(q.is_live(id));
    assert!(q.cancel(id).is_empty());
    assert_eq!(view(&q, id).state, TaskState::Cancelled);
    assert!(!q.is_live(id), "work cut off for it isn't run again");
    assert!(!q.is_live(JobId(99)));
}

#[test]
fn host_down_and_up_with_interrupted_files() {
    let mut q = Queue::new(2);
    let id = running_download(&mut q, "a", &["d"], &[&[("a", 100), ("b", 100), ("c", 100)]]);
    q.poll(T0);
    q.file_progress(id, FileId(0), 40, secs(1));
    assert!(!view(&q, id).waiting_for_connection);

    q.host_down(&host("a"));
    let v = view(&q, id);
    assert!(v.waiting_for_connection);
    assert_eq!(v.state, TaskState::Running); // keeps its state
    assert!(q.poll(secs(2)).is_empty());
    q.file_interrupted(id, FileId(0));
    q.file_interrupted(id, FileId(1));
    assert_eq!(view(&q, id).progress.done, 40);
    assert!(q.poll(secs(2)).is_empty());

    q.host_up(&host("a"));
    assert!(!view(&q, id).waiting_for_connection);
    assert_eq!(starts(&q.poll(secs(3))), vec![(id, FileId(0)), (id, FileId(1))]);
}

#[test]
fn reservations_last_while_the_job_is_active() {
    let mut q = Queue::new(2);
    let id = running_download(&mut q, "a", &["x.bin"], &[&[("x.bin", 1)]]);
    assert!(q.is_reserved_local(Path::new("/d/x.bin")));
    assert!(q.is_reserved_local(Path::new("/d/x.bin.dromon-part")));
    assert!(!q.is_reserved_local(Path::new("/d/y.bin")));
    q.poll(T0);
    q.file_done(id, FileId(0), secs(1));
    assert!(!q.is_reserved_local(Path::new("/d/x.bin")));
    assert!(!q.is_reserved_local(Path::new("/d/x.bin.dromon-part")));

    let id = running_download(&mut q, "a", &["z"], &[&[("z", 1)]]);
    assert!(q.is_reserved_local(Path::new("/d/z")));
    q.cancel(id);
    assert!(!q.is_reserved_local(Path::new("/d/z")));
}

#[test]
fn views_are_newest_first_with_titles() {
    let mut q = Queue::new(2);
    let a = q.add(download("a", &["one.txt"]), T0);
    let b = q.add(upload("a", &["p", "q", "r"]), T0);
    let views = q.views(T0);
    assert_eq!(views.iter().map(|v| v.id).collect::<Vec<_>>(), vec![b, a]);
    assert_eq!((views[0].title.as_str(), views[0].kind), ("3 items", crate::JobKind::Upload));
    assert_eq!(views[1].title, "one.txt");
    assert!(views[1].waiting_for_connection); // queued on a down host
    assert_eq!(q.view(JobId(99), T0), None);
}

#[test]
fn progress_grows_while_preparing_and_eta_needs_two_seconds_of_data() {
    let mut q = Queue::new(2);
    let id = running_download(&mut q, "a", &["d"], &[&[("a", 600), ("b", 400)]]);
    assert_eq!(view(&q, id).progress, Progress { done: 0, total: 1000 });
    q.poll(T0);
    assert_eq!(view(&q, id).speed, None);
    q.file_progress(id, FileId(0), 100, secs(10));
    q.file_progress(id, FileId(0), 200, secs(11));
    let v = q.view(id, secs(11)).unwrap();
    assert_eq!(v.speed, Some(100.0));
    assert_eq!(v.eta, None); // 1 s of data
    q.file_progress(id, FileId(0), 400, secs(13));
    let v = q.view(id, secs(13)).unwrap();
    assert_eq!(v.speed, Some(100.0));
    assert_eq!(v.eta, Some(secs(6)));
    // Progress is clamped to the file's size.
    q.file_progress(id, FileId(0), 9999, secs(14));
    assert_eq!(view(&q, id).progress.done, 600);
    // Not Running: no speed or eta.
    q.pause(id);
    let v = q.view(id, secs(14)).unwrap();
    assert_eq!((v.speed, v.eta), (None, None));
}

#[test]
fn poll_is_idempotent_and_unknown_ids_are_ignored() {
    let mut q = Queue::new(2);
    let id = running_download(&mut q, "a", &["d"], &[&[("a", 1), ("b", 1), ("c", 1)]]);
    assert_eq!(starts(&q.poll(T0)).len(), 2);
    assert!(q.poll(T0).is_empty());
    assert!(q.poll(secs(5)).is_empty());

    let ghost = JobId(42);
    q.named(ghost, 0, local("x"));
    q.exists(ghost, 0, None, Side::default());
    q.resolve(ghost, ConflictChoice::Skip, true);
    q.scan_progress(ghost, 0, 3);
    q.scanned(ghost, 0, Scan::default());
    q.item_failed(ghost, 0, "x".into());
    q.file_progress(ghost, FileId(0), 1, T0);
    q.file_done(ghost, FileId(0), T0);
    q.file_failed(ghost, FileId(0), "x".into(), T0);
    q.file_interrupted(ghost, FileId(0));
    assert!(q.pause(ghost).is_empty() && q.cancel(ghost).is_empty());
    q.resume(ghost);
    q.retry(ghost);
    // Out-of-range items and files.
    q.named(id, 7, local("x"));
    q.scanned(id, 7, Scan::default());
    q.item_failed(id, 7, "x".into());
    q.file_done(id, FileId(99), T0);
    assert_eq!(view(&q, id).state, TaskState::Running);
}

#[test]
fn clear_finished_removes_done_failed_and_cancelled_jobs() {
    let mut q = Queue::new(2);
    let done = running_download(&mut q, "a", &["a"], &[&[]]);
    let failed = running_download(&mut q, "a", &["b"], &[&[("f", 1)]]);
    let cancelled = q.add(download("a", &["c"]), T0);
    let paused = running_download(&mut q, "a", &["d"], &[&[("g", 1)]]);
    let running = running_download(&mut q, "a", &["e"], &[&[("h", 1)]]);
    q.cancel(cancelled);
    q.pause(paused);
    q.poll(T0);
    q.file_failed(failed, FileId(0), "x".into(), T0);
    assert_eq!(view(&q, done).state, TaskState::Done);
    q.clear_finished();
    let ids: Vec<JobId> = q.views(T0).iter().map(|v| v.id).collect();
    assert_eq!(ids, vec![running, paused]);
}
