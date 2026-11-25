//! The queue with remote-to-remote jobs: copies and moves on one host and between two.

use dromon_core::EntryKind;

use super::*;
use crate::RemoteItem;

const T0: Duration = Duration::ZERO;

fn host(name: &str) -> HostId {
    HostId(name.to_owned())
}

fn file(path: &str) -> RemoteItem {
    RemoteItem { path: RemotePath::new(path), kind: EntryKind::File, size: 10 }
}

fn folder(path: &str) -> RemoteItem {
    RemoteItem { path: RemotePath::new(path), kind: EntryKind::Dir, size: 4096 }
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// A request of `kind` from host a (to host b for the cross-host kinds).
fn request(kind: JobKind, items: Vec<RemoteItem>, into: &str) -> JobRequest {
    let (a, b, into) = (host("a"), host("b"), p(into));
    match kind {
        JobKind::RemoteCopy => JobRequest::RemoteCopy { host: a, items, into },
        JobKind::Move => JobRequest::Move { host: a, items, into },
        JobKind::CrossHostCopy => JobRequest::CrossHostCopy { from: a, items, to: b, into },
        JobKind::CrossHostMove => JobRequest::CrossHostMove { from: a, items, to: b, into },
        JobKind::Download | JobKind::Upload => unreachable!("remote kinds only"),
    }
}

const KINDS: [JobKind; 4] = [JobKind::RemoteCopy, JobKind::Move, JobKind::CrossHostCopy, JobKind::CrossHostMove];

fn copy_plan(name: &str, size: u64) -> FilePlan {
    FilePlan::Copy { src: p(&format!("/src/d/{name}")), dst: p(&format!("/dst/d/{name}")), size, mtime: Some(5), overwrite: false }
}

fn both_up(q: &mut Queue) {
    q.host_up(&host("a"));
    q.host_up(&host("b"));
}

fn starts(actions: &[Action]) -> Vec<FileId> {
    actions.iter().filter_map(|a| if let Action::Start { file, .. } = a { Some(*file) } else { None }).collect()
}

fn view(q: &Queue, id: JobId) -> JobView {
    q.view(id, T0).unwrap()
}

#[test]
fn hosts_of_each_request() {
    let req = request(JobKind::CrossHostMove, vec![file("/src/x")], "/dst");
    assert_eq!(req.hosts(), [&host("a"), &host("b")], "source first");
    assert_eq!((req.host(), req.source_host(), req.kind()), (&host("b"), &host("a"), JobKind::CrossHostMove));
    assert_eq!(request(JobKind::Move, vec![], "/dst").hosts(), [&host("a")]);
    let same = JobRequest::CrossHostCopy { from: host("a"), items: vec![], to: host("a"), into: p("/x") };
    assert_eq!(same.hosts(), [&host("a")]);
    for kind in KINDS {
        assert_eq!(request(kind, vec![file("/src/x"), file("/src/y")], "/dst").item_count(), 2);
        assert!(kind.in_uploads_tab());
        assert_eq!(kind.direction(), crate::Direction::Remote);
    }
}

#[test]
fn a_two_host_job_waits_for_both_and_counts_against_both_limits() {
    let mut q = Queue::new(2);
    let id = q.add(request(JobKind::CrossHostCopy, vec![folder("/src/d")], "/dst"), T0);
    q.host_up(&host("a"));
    assert!(q.poll(T0).is_empty(), "b is down");
    assert!(view(&q, id).waiting_for_connection);
    q.host_up(&host("b"));
    assert!(!view(&q, id).waiting_for_connection);
    assert_eq!(q.poll(T0), vec![Action::Name { job: id, item: 0 }]);
    q.exists(id, 0, None);
    let dst = ItemDst::Remote { path: p("/dst/d"), overwrite: false };
    assert_eq!(q.poll(T0), vec![Action::Scan { job: id, item: 0, dst }]);
    let v = view(&q, id);
    assert_eq!((v.host, v.reveal, v.kind), (host("b"), Some(Reveal::Remote(host("b"), p("/dst/d"))), JobKind::CrossHostCopy));

    // A download on b already runs one file: b has room for one more.
    let dl = q.add(JobRequest::Download { host: host("b"), items: vec![file("/r/x")], into: PathBuf::from("/l") }, T0);
    q.poll(T0);
    q.named(dl, 0, ItemDst::Local(PathBuf::from("/l/x")));
    q.poll(T0);
    let plan = FilePlan::Download { src: p("/r/x"), dst: PathBuf::from("/l/x"), size: 1, mtime: None };
    q.scanned(dl, 0, Scan { files: vec![plan], ..Scan::default() });
    assert_eq!(starts(&q.poll(T0)), [FileId(0)]);

    q.scanned(id, 0, Scan { files: vec![copy_plan("1", 1), copy_plan("2", 1), copy_plan("3", 1)], ..Scan::default() });
    assert_eq!(starts(&q.poll(T0)), [FileId(0)], "b's limit");
    q.file_done(dl, FileId(0), T0);
    assert_eq!(starts(&q.poll(T0)), [FileId(1)]);

    // Either host going down stops new work and makes it wait.
    q.host_down(&host("a"));
    assert!(view(&q, id).waiting_for_connection);
    q.file_done(id, FileId(0), T0);
    assert!(q.poll(T0).is_empty());
    q.host_up(&host("a"));
    assert_eq!(starts(&q.poll(T0)), [FileId(2)]);
}

#[test]
fn every_remote_kind_asks_on_a_clash_at_the_destination() {
    for kind in KINDS {
        let mut q = Queue::new(3);
        both_up(&mut q);
        let id = q.add(request(kind, vec![file("/src/a.txt"), folder("/src/d"), file("/src/c")], "/dst"), T0);
        assert_eq!(q.poll(T0).len(), 3, "{kind:?}: three names");
        q.exists(id, 0, Some(false));
        q.exists(id, 1, Some(true));
        q.exists(id, 2, Some(false));
        let c = view(&q, id).conflict.unwrap();
        assert_eq!(c, ConflictView { name: "a.txt".into(), folder: p("/dst"), existing_is_dir: false, more: 2 }, "{kind:?}");

        q.resolve(id, ConflictChoice::Replace, false);
        q.resolve(id, ConflictChoice::KeepBoth, false);
        q.resolve(id, ConflictChoice::Skip, false);
        assert_eq!(view(&q, id).skipped, ["c"], "{kind:?}");
        let actions = q.poll(T0);
        let replaced = ItemDst::Remote { path: p("/dst/a.txt"), overwrite: true };
        if kind == JobKind::Move {
            // A move needs no scan: Replace planned the rename at once.
            assert_eq!(actions, [Action::FreeRemoteName { job: id, item: 1 }], "{kind:?}");
            let plan = FilePlan::Move { src: p("/src/a.txt"), dst: p("/dst/a.txt"), size: 10, overwrite: true };
            assert_eq!(q.plan(id, FileId(0)), Some(&plan));
        } else {
            assert_eq!(
                actions,
                [Action::Scan { job: id, item: 0, dst: replaced }, Action::FreeRemoteName { job: id, item: 1 }],
                "{kind:?}"
            );
        }
        // Keep both: a free name, then on as usual.
        q.named(id, 1, ItemDst::Remote { path: p("/dst/d (1)"), overwrite: false });
        let next = q.poll(T0);
        if kind == JobKind::Move {
            // Every item is settled: both renames start.
            let plan = FilePlan::Move { src: p("/src/d"), dst: p("/dst/d (1)"), size: 0, overwrite: false };
            assert_eq!(starts(&next), [FileId(0), FileId(1)]);
            assert_eq!(next[1], Action::Start { job: id, file: FileId(1), plan });
        } else {
            assert_eq!(next, [Action::Scan { job: id, item: 1, dst: ItemDst::Remote { path: p("/dst/d (1)"), overwrite: false } }]);
        }
    }
}

#[test]
fn apply_to_all_answers_later_clashes_of_a_move() {
    let mut q = Queue::new(3);
    both_up(&mut q);
    let id = q.add(request(JobKind::CrossHostMove, vec![file("/src/a"), file("/src/b")], "/dst"), T0);
    q.poll(T0);
    q.exists(id, 0, Some(false));
    q.resolve(id, ConflictChoice::Skip, true);
    q.exists(id, 1, Some(false));
    let v = view(&q, id);
    assert_eq!((v.skipped.len(), v.state, v.conflict), (2, TaskState::Done, None));
}

#[test]
fn a_copy_into_its_own_folder_keeps_both_without_asking() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(request(JobKind::RemoteCopy, vec![file("/srv/a.txt"), file("/elsewhere/b.txt")], "/srv"), T0);
    assert_eq!(q.poll(T0), [Action::FreeRemoteName { job: id, item: 0 }, Action::Name { job: id, item: 1 }]);
    assert_eq!(view(&q, id).conflict, None);
    let dst = ItemDst::Remote { path: p("/srv/a (1).txt"), overwrite: false };
    q.named(id, 0, dst.clone());
    assert_eq!(q.poll(T0), [Action::Scan { job: id, item: 0, dst }]);

    // A cross-host copy whose two hosts are one is held to the same rule.
    let same = JobRequest::CrossHostCopy { from: host("a"), items: vec![file("/srv/a.txt")], to: host("a"), into: p("/srv") };
    let id = q.add(same, T0);
    assert_eq!(q.poll(T0), [Action::FreeRemoteName { job: id, item: 0 }]);
    // Between two hosts the same paths are unrelated: an ordinary name check.
    both_up(&mut q);
    let id = q.add(request(JobKind::CrossHostCopy, vec![file("/srv/a.txt")], "/srv"), T0);
    assert_eq!(q.poll(T0), [Action::Name { job: id, item: 0 }]);
}

#[test]
fn a_move_into_its_own_folder_is_skipped_as_already_there() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(request(JobKind::Move, vec![file("/srv/a.txt"), folder("/srv/d")], "/srv"), T0);
    assert!(q.poll(T0).is_empty());
    let v = view(&q, id);
    assert_eq!(v.state, TaskState::Done);
    assert_eq!(v.skipped, ["a.txt: already there", "d: already there"]);
    assert_eq!((v.files_total, v.errors.len(), v.reveal), (0, 0, None));
}

#[test]
fn a_folder_into_itself_fails_the_item() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(request(JobKind::Move, vec![folder("/srv/d"), file("/srv/x")], "/srv/d/sub"), T0);
    assert_eq!(q.poll(T0), [Action::Name { job: id, item: 1 }]);
    assert_eq!(view(&q, id).errors, ["d: can't move a folder into itself"]);
    q.exists(id, 1, None);
    q.poll(T0);
    q.file_done(id, FileId(0), T0);
    assert_eq!(view(&q, id).state, TaskState::Failed { error: "d: can't move a folder into itself".into() });
    // Retrying doesn't change the answer.
    q.retry(id);
    assert!(q.poll(T0).is_empty());
    assert!(matches!(view(&q, id).state, TaskState::Failed { .. }));

    let id = q.add(request(JobKind::RemoteCopy, vec![folder("/srv/d")], "/srv/d"), T0);
    assert!(q.poll(T0).is_empty());
    assert_eq!(view(&q, id).state, TaskState::Failed { error: "d: can't copy a folder into itself".into() });
    // /srv/dx is not inside /srv/d.
    let id = q.add(request(JobKind::Move, vec![folder("/srv/d")], "/srv/dx"), T0);
    assert_eq!(q.poll(T0), [Action::Name { job: id, item: 0 }]);
}

#[test]
fn a_move_is_one_file_per_item_done_at_once() {
    let mut q = Queue::new(3);
    q.host_up(&host("a"));
    let id = q.add(request(JobKind::Move, vec![folder("/srv/d"), file("/srv/f")], "/dst"), T0);
    q.poll(T0);
    q.exists(id, 0, None);
    assert!(matches!(view(&q, id).state, TaskState::Preparing { .. }));
    q.exists(id, 1, None);
    let v = view(&q, id);
    assert_eq!((v.state, v.files_total, v.progress.total), (TaskState::Running, 2, 10));
    assert_eq!(v.reveal, Some(Reveal::Remote(host("a"), p("/dst/d"))));
    let plans: Vec<FilePlan> =
        q.poll(T0).into_iter().filter_map(|a| if let Action::Start { plan, .. } = a { Some(plan) } else { None }).collect();
    assert_eq!(
        plans,
        [
            FilePlan::Move { src: p("/srv/d"), dst: p("/dst/d"), size: 0, overwrite: false },
            FilePlan::Move { src: p("/srv/f"), dst: p("/dst/f"), size: 10, overwrite: false },
        ]
    );
    q.file_done(id, FileId(0), T0);
    q.file_done(id, FileId(1), T0);
    assert_eq!(view(&q, id).state, TaskState::Done);
}

#[test]
fn a_cross_host_move_tidies_each_folder_before_it_is_done() {
    let mut q = Queue::new(3);
    both_up(&mut q);
    let items = vec![folder("/src/d"), folder("/src/empty"), file("/src/f")];
    let id = q.add(request(JobKind::CrossHostMove, items, "/dst"), T0);
    q.poll(T0);
    for n in 0..3 {
        q.exists(id, n, None);
    }
    q.poll(T0);
    q.scanned(id, 0, Scan { files: vec![copy_plan("1", 1), copy_plan("2", 1)], ..Scan::default() });
    // An empty folder has nothing to copy: it is tidied straight away.
    q.scanned(id, 1, Scan::default());
    assert_eq!(q.poll(T0), [Action::Tidy { job: id, item: 1 }]);
    q.scanned(id, 2, Scan { files: vec![copy_plan("f", 1)], ..Scan::default() });
    assert_eq!(starts(&q.poll(T0)), [FileId(0), FileId(1), FileId(2)]);
    q.tidied(id, 1);

    q.file_done(id, FileId(0), T0);
    q.file_failed(id, FileId(1), "size differs".into(), T0);
    q.file_done(id, FileId(2), T0); // a file item: nothing to tidy
    assert_eq!(view(&q, id).state, TaskState::Running, "waits for the tidy");
    assert_eq!(q.poll(T0), [Action::Tidy { job: id, item: 0 }]);
    assert!(q.poll(T0).is_empty());
    q.tidied(id, 0);
    assert_eq!(view(&q, id).state, TaskState::Failed { error: "1 file failed".into() });

    // A retry runs the failed file again, then tidies its folder again.
    q.retry(id);
    assert_eq!(starts(&q.poll(T0)), [FileId(1)]);
    q.file_done(id, FileId(1), T0);
    assert_eq!(q.poll(T0), [Action::Tidy { job: id, item: 0 }]);
    q.tidied(id, 0);
    assert_eq!(view(&q, id).state, TaskState::Done);
}

#[test]
fn copies_never_tidy_and_cancel_cleans_their_parts() {
    let mut q = Queue::new(3);
    both_up(&mut q);
    let id = q.add(request(JobKind::CrossHostCopy, vec![folder("/src/d")], "/dst"), T0);
    q.poll(T0);
    q.exists(id, 0, None);
    q.poll(T0);
    q.scanned(id, 0, Scan { files: vec![copy_plan("1", 100)], ..Scan::default() });
    q.poll(T0);
    q.file_progress(id, FileId(0), 50, T0);
    let actions = q.cancel(id);
    assert_eq!(actions, [Action::Abort { job: id, file: FileId(0) }, Action::Cleanup { job: id, plans: vec![copy_plan("1", 100)] }]);

    let id = q.add(request(JobKind::RemoteCopy, vec![folder("/src/d")], "/dst"), T0);
    q.poll(T0);
    q.exists(id, 0, None);
    q.poll(T0);
    q.scanned(id, 0, Scan::default());
    assert!(q.poll(T0).is_empty());
    assert_eq!(view(&q, id).state, TaskState::Done);
}
