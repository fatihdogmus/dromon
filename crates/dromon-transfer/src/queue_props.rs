//! Properties of the pure queue, as a state machine beside a small model: jobs of every kind on
//! a few hosts, hosts going up and down, polls whose work is answered the way the engine would
//! (in any order), the user's controls, and stale facts for jobs that are over or unknown.
//! After every step: no host runs more files than the limit lets it, nothing starts while one
//! of its job's hosts is down, a cancelled job never starts again, the queue runs exactly the
//! files the model started, and the views and the active count agree with the model.
//!
//! The machine draws file sizes small enough that their sums fit a u64. That a running job never
//! panics on the sizes a server may send (up to `u64::MAX`) is its own property, at the end.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use dromon_core::{EntryKind, HostId, RemotePath};
use hegel::generators as gs;
use hegel::{Generator, TestCase};

use super::job::FileState;
use super::{Action, ItemDst, Queue};
use crate::{
    ConflictChoice, Existing, FileId, FilePlan, JobId, JobKind, JobRequest, JobView, RemoteItem, Scan, Side, TaskState,
};

const HOSTS: [&str; 3] = ["a", "b", "c"];

const KINDS: [JobKind; 6] = [
    JobKind::Download,
    JobKind::Upload,
    JobKind::RemoteCopy,
    JobKind::Move,
    JobKind::CrossHostCopy,
    JobKind::CrossHostMove,
];

#[hegel::composite]
fn hosts(tc: &TestCase) -> HostId {
    HostId(tc.draw(gs::sampled_from(HOSTS.as_slice())).to_owned())
}

/// Picked remote items, some of them in (or above) the folders the jobs go into, so the
/// same-host rules (into its own folder, into itself) come up.
#[hegel::composite]
fn remote_items(tc: &TestCase) -> RemoteItem {
    RemoteItem {
        path: RemotePath::new(tc.draw(gs::sampled_from(["/src/a", "/src/d", "/dst/a", "/dst", "/"].as_slice()))),
        kind: tc.draw(gs::sampled_from([EntryKind::File, EntryKind::Dir].as_slice())),
        size: tc.draw(gs::integers::<u64>().max_value(1 << 20)),
    }
}

#[hegel::composite]
fn requests(tc: &TestCase) -> JobRequest {
    let kind = tc.draw(gs::sampled_from(KINDS.as_slice()));
    let (from, to) = (tc.draw(hosts()), tc.draw(hosts()));
    let items = tc.draw(gs::vecs(remote_items()).min_size(1).max_size(3));
    let into = RemotePath::new(tc.draw(gs::sampled_from(["/dst", "/src", "/src/d/sub", "/"].as_slice())));
    match kind {
        JobKind::Download => JobRequest::Download { host: from, items, into: PathBuf::from("/l") },
        JobKind::Upload => {
            let local = gs::sampled_from(["/home/u/a.txt", "/home/u/d", "/home/u/..", "/"].as_slice());
            let items = tc.draw(gs::vecs(local.map(PathBuf::from)).min_size(1).max_size(3));
            JobRequest::Upload { host: from, items, into }
        }
        JobKind::RemoteCopy => JobRequest::RemoteCopy { host: from, items, into },
        JobKind::Move => JobRequest::Move { host: from, items, into },
        JobKind::CrossHostCopy => JobRequest::CrossHostCopy { from, items, to, into },
        JobKind::CrossHostMove => JobRequest::CrossHostMove { from, items, to, into },
    }
}

#[hegel::composite]
fn sides(tc: &TestCase) -> Side {
    Side { size: tc.draw(gs::optional(gs::integers::<u64>())), mtime: tc.draw(gs::optional(gs::integers::<i64>())) }
}

/// What a name check finds at the destination: nothing, a file or a folder.
#[hegel::composite]
fn existing(tc: &TestCase) -> Option<Existing> {
    let is_dir = tc.draw(gs::booleans());
    let side = tc.draw(sides());
    tc.draw(gs::optional(gs::just(Existing { is_dir, side })))
}

/// One file a scan finds for a job of `kind`.
#[hegel::composite]
fn file_plans(tc: &TestCase, kind: JobKind) -> FilePlan {
    let n = tc.draw(gs::integers::<u8>());
    let size = tc.draw(hegel::one_of!(gs::integers::<u64>().max_value(100), gs::integers::<u64>().max_value(1 << 40)));
    let remote = RemotePath::new(&format!("/x/f{n}"));
    match kind {
        JobKind::Download => {
            FilePlan::Download { src: remote, dst: PathBuf::from(format!("/l/f{n}")), size, mtime: None }
        }
        JobKind::Upload => FilePlan::Upload {
            src: PathBuf::from(format!("/home/u/f{n}")),
            dst: remote,
            size,
            mtime: None,
            overwrite: false,
        },
        _ => FilePlan::Copy {
            src: RemotePath::new(&format!("/src/f{n}")),
            dst: remote,
            size,
            mtime: None,
            overwrite: false,
        },
    }
}

/// The job an action is for.
fn job_of(a: &Action) -> JobId {
    match a {
        Action::Name { job, .. }
        | Action::FreeRemoteName { job, .. }
        | Action::Scan { job, .. }
        | Action::Start { job, .. }
        | Action::Abort { job, .. }
        | Action::Cleanup { job, .. }
        | Action::Tidy { job, .. } => *job,
    }
}

struct QueueMachine {
    queue: Queue,
    /// The engine's clock.
    now: Duration,
    up: BTreeSet<HostId>,
    /// Every job ever added: its kind and the hosts it needs.
    jobs: BTreeMap<JobId, (JobKind, Vec<HostId>)>,
    /// The jobs the queue still holds, oldest first.
    listed: Vec<JobId>,
    next_id: u64,
    /// Work handed out and not answered yet (never `Abort` or `Cleanup`, which need no answer).
    work: Vec<Action>,
    /// The files the model started and that have not ended, with the hosts each counts against.
    running: BTreeMap<(JobId, FileId), Vec<HostId>>,
    /// The most files each host may run until the next poll: the limit, or what already ran
    /// when a lowered limit holds new starts back.
    allowed: BTreeMap<HostId, usize>,
    cancelled: BTreeSet<JobId>,
}

impl QueueMachine {
    fn new(per_host: usize) -> QueueMachine {
        QueueMachine {
            queue: Queue::new(per_host),
            now: Duration::ZERO,
            up: BTreeSet::new(),
            jobs: BTreeMap::new(),
            listed: Vec::new(),
            next_id: 1,
            work: Vec::new(),
            running: BTreeMap::new(),
            allowed: BTreeMap::new(),
            cancelled: BTreeSet::new(),
        }
    }

    fn tick(&mut self, tc: &TestCase) {
        self.now += Duration::from_millis(tc.draw(gs::integers::<u64>().max_value(3_000)));
    }

    /// A job id: mostly one the queue holds, else one it gave out (maybe cleared since) or one
    /// it never did.
    fn any_job(&self, tc: &TestCase) -> JobId {
        if !self.listed.is_empty() && tc.draw(gs::weighted_booleans(0.8)) {
            return tc.draw(gs::sampled_from(self.listed.as_slice()));
        }
        JobId(tc.draw(gs::integers::<u64>().min_value(1).max_value(self.next_id + 2)))
    }

    /// Files running in the queue itself, read from its jobs.
    fn queue_running(&self) -> BTreeSet<(JobId, FileId)> {
        let mut out = BTreeSet::new();
        for j in &self.queue.jobs {
            for (n, _) in j.files.iter().enumerate().filter(|(_, f)| f.state == FileState::Running) {
                out.insert((j.id, FileId(n as u32)));
            }
        }
        out
    }

    fn running_on(&self, host: &HostId) -> usize {
        self.running.values().filter(|hosts| hosts.contains(host)).count()
    }

    /// Takes one action a poll handed out, checking it was allowed.
    fn handed_out(&mut self, a: Action) {
        let job = job_of(&a);
        let (kind, hosts) = self.jobs.get(&job).cloned().unwrap_or_else(|| panic!("{a:?} for a job never added"));
        assert!(!self.cancelled.contains(&job), "{a:?} for a cancelled job");
        assert!(hosts.iter().all(|h| self.up.contains(h)), "{a:?} while one of {hosts:?} is down");
        match &a {
            Action::Start { job, file, .. } => {
                assert!(self.running.insert((*job, *file), hosts).is_none(), "{a:?} while it runs");
            }
            Action::Scan { .. } => assert_ne!(kind, JobKind::Move, "a same-host move is never scanned"),
            Action::Abort { .. } | Action::Cleanup { .. } => panic!("{a:?} from a poll"),
            Action::Name { .. } | Action::FreeRemoteName { .. } | Action::Tidy { .. } => {}
        }
        self.work.push(a);
    }

    /// Takes what `pause` or `cancel` returned: an `Abort` for each running file of `job`, then
    /// for a cancel at most one `Cleanup`, last.
    fn stopped(&mut self, job: JobId, actions: Vec<Action>) {
        let n = actions.len();
        for (i, a) in actions.into_iter().enumerate() {
            match a {
                Action::Abort { job: j, file } => {
                    assert_eq!(j, job);
                    assert!(self.running.remove(&(j, file)).is_some(), "{a:?} for a file that doesn't run");
                    self.work.retain(|w| !matches!(w, Action::Start { job, file: f, .. } if *job == j && *f == file));
                }
                Action::Cleanup { job: j, ref plans } => {
                    assert!(j == job && i + 1 == n && !plans.is_empty(), "{a:?}");
                }
                other => panic!("{other:?} from pause or cancel"),
            }
        }
    }

    /// Polls, takes what it hands out, and sets each host's allowance until the next poll.
    fn poll_once(&mut self) {
        let before: Vec<usize> = HOSTS.iter().map(|h| self.running_on(&HostId((*h).to_owned()))).collect();
        let per_host = self.queue.per_host();
        for a in self.queue.poll(self.now) {
            self.handed_out(a);
        }
        for (h, ran) in HOSTS.iter().zip(before) {
            self.allowed.insert(HostId((*h).to_owned()), per_host.max(ran));
        }
        let again = self.queue.poll(self.now);
        assert!(again.is_empty(), "a second poll without new facts: {again:?}");
    }

    /// Answers one piece of work as the engine would.
    fn answer_one(&mut self, tc: &TestCase, a: Action) {
        self.tick(tc);
        match a {
            Action::Name { job, item } => self.answer_name(tc, job, item),
            Action::FreeRemoteName { job, item } => {
                let path = RemotePath::new(&format!("/dst/free-{}-{item}", job.0));
                self.queue.named(job, item, ItemDst::Remote { path, overwrite: false });
            }
            Action::Scan { .. } => self.answer_scan(tc, a),
            Action::Start { .. } => self.answer_file(tc, a),
            Action::Tidy { job, item } => self.queue.tidied(job, item),
            Action::Abort { .. } | Action::Cleanup { .. } => unreachable!("never kept as work"),
        }
    }

    fn answer_name(&mut self, tc: &TestCase, job: JobId, item: usize) {
        if self.jobs[&job].0 == JobKind::Download {
            self.queue.named(job, item, ItemDst::Local(PathBuf::from(format!("/l/{}-{item}", job.0))));
        } else {
            let found = tc.draw(existing());
            let incoming = tc.draw(sides());
            self.queue.exists(job, item, found, incoming);
        }
    }

    fn answer_scan(&mut self, tc: &TestCase, scan: Action) {
        let Action::Scan { job, item, .. } = scan else { unreachable!("a scan") };
        // Mostly a scan that found files; sometimes an empty folder, a failure or progress.
        match tc.draw(gs::integers::<u8>().max_value(5)) {
            n @ 0..=3 => {
                let found = gs::vecs(file_plans(self.jobs[&job].0)).min_size(usize::from(n > 0)).max_size(4);
                let files = tc.draw(found);
                let skipped = tc.draw(gs::vecs(gs::just("x: a link to a folder".to_owned())).max_size(1));
                self.queue.scanned(job, item, Scan { files, skipped, ..Scan::default() });
            }
            4 => self.queue.item_failed(job, item, "permission denied".into()),
            _ => {
                self.queue.scan_progress(job, item, tc.draw(gs::integers::<u64>().max_value(1_000)));
                self.work.push(scan);
            }
        }
    }

    fn answer_file(&mut self, tc: &TestCase, start: Action) {
        let Action::Start { job, file, plan } = &start else { unreachable!("a start") };
        let (job, file) = (*job, *file);
        match tc.draw(gs::integers::<u8>().max_value(3)) {
            0 => {
                let done = tc.draw(gs::integers::<u64>().max_value(plan.size().saturating_mul(2)));
                self.queue.file_progress(job, file, done, self.now);
                self.work.push(start);
                return;
            }
            1 => self.queue.file_done(job, file, self.now),
            2 => self.queue.file_failed(job, file, "disk full".into(), self.now),
            _ => self.queue.file_interrupted(job, file),
        }
        self.running.remove(&(job, file));
    }

    /// One fact for `job` with these item and file indexes: an engine report or a control.
    fn fact(&mut self, tc: &TestCase, job: JobId, item: usize, file: FileId) {
        match tc.draw(gs::integers::<u8>().max_value(13)) {
            n @ 0..=9 => self.report(tc, job, item, file, n),
            10 => self.queue.resolve(job, ConflictChoice::Replace, true),
            11 => assert!(self.queue.pause(job).is_empty()),
            12 => assert!(self.queue.cancel(job).is_empty()),
            _ => self.queue.resume(job),
        }
    }

    /// The engine's report number `n`: on item `item` (0 to 5) or on file `file` (6 to 9).
    fn report(&mut self, tc: &TestCase, job: JobId, item: usize, file: FileId, n: u8) {
        let now = self.now;
        let q = &mut self.queue;
        match n {
            0 => q.named(job, item, ItemDst::Local(PathBuf::from("/l/stale"))),
            1 => q.exists(job, item, tc.draw(existing()), tc.draw(sides())),
            2 => q.scan_progress(job, item, 7),
            3 => q.scanned(job, item, Scan { files: vec![tc.draw(file_plans(JobKind::Download))], ..Scan::default() }),
            4 => q.item_failed(job, item, "gone".into()),
            5 => q.tidied(job, item),
            6 => q.file_progress(job, file, 1, now),
            7 => q.file_done(job, file, now),
            8 => q.file_failed(job, file, "gone".into(), now),
            _ => q.file_interrupted(job, file),
        }
    }

    fn check_view(&self, v: &JobView) {
        let (_, hosts) = &self.jobs[&v.id];
        assert!(v.progress.done <= v.progress.total, "{v:?}");
        assert!(v.files_done <= v.files_total, "{v:?}");
        assert!(v.speed.is_none_or(|s| s.is_finite() && s >= 0.0), "{v:?}");
        let moving = matches!(v.state, TaskState::Queued | TaskState::Preparing { .. } | TaskState::Running);
        assert_eq!(v.waiting_for_connection, moving && !hosts.iter().all(|h| self.up.contains(h)), "{v:?}");
        assert_eq!(v.state == TaskState::Cancelled, self.cancelled.contains(&v.id), "{v:?}");
    }
}

#[hegel::state_machine]
impl QueueMachine {
    #[rule(weight = 2)]
    fn add(&mut self, tc: TestCase) {
        let req = tc.draw(requests());
        let id = self.queue.add(req.clone(), self.now);
        assert_eq!(id, JobId(self.next_id), "ids count from 1");
        self.next_id += 1;
        self.jobs.insert(id, (req.kind(), req.hosts().into_iter().cloned().collect()));
        self.listed.push(id);
    }

    #[rule]
    fn host_up(&mut self, tc: TestCase) {
        let host = tc.draw(hosts());
        self.queue.host_up(&host);
        self.up.insert(host);
    }

    #[rule]
    fn host_down(&mut self, tc: TestCase) {
        let host = tc.draw(hosts());
        self.queue.host_down(&host);
        self.up.remove(&host);
    }

    #[rule(weight = 4)]
    fn poll(&mut self, _: TestCase) {
        self.poll_once();
    }

    #[rule(weight = 8)]
    fn answer(&mut self, tc: TestCase) {
        tc.assume(!self.work.is_empty());
        let i = tc.draw(gs::integers::<usize>().max_value(self.work.len() - 1));
        let a = self.work.remove(i);
        self.answer_one(&tc, a);
    }

    /// The engine catches up: a few rounds of polling and answering all the work.
    #[rule(weight = 3)]
    fn catch_up(&mut self, tc: TestCase) {
        for _ in 0..3 {
            self.poll_once();
            for a in std::mem::take(&mut self.work) {
                self.answer_one(&tc, a);
            }
        }
    }

    #[rule(weight = 2)]
    fn resolve(&mut self, tc: TestCase) {
        // Mostly a job that asks.
        let asking: Vec<JobId> =
            self.queue.views(self.now).iter().filter(|v| v.conflict.is_some()).map(|v| v.id).collect();
        let job = if asking.is_empty() { self.any_job(&tc) } else { tc.draw(gs::sampled_from(asking.as_slice())) };
        let choice = tc.draw(gs::sampled_from(
            [ConflictChoice::Replace, ConflictChoice::KeepBoth, ConflictChoice::Skip].as_slice(),
        ));
        let apply_all = tc.draw(gs::booleans());
        self.queue.resolve(job, choice, apply_all);
    }

    #[rule]
    fn pause(&mut self, tc: TestCase) {
        let job = self.any_job(&tc);
        let actions = self.queue.pause(job);
        self.stopped(job, actions);
    }

    #[rule]
    fn resume(&mut self, tc: TestCase) {
        let job = self.any_job(&tc);
        self.queue.resume(job);
    }

    #[rule]
    fn cancel(&mut self, tc: TestCase) {
        let job = self.any_job(&tc);
        let live = self.queue.is_live(job);
        let actions = self.queue.cancel(job);
        self.stopped(job, actions);
        if live {
            self.cancelled.insert(job);
            assert_eq!(self.queue.view(job, self.now).map(|v| v.state), Some(TaskState::Cancelled));
        }
    }

    #[rule]
    fn retry(&mut self, tc: TestCase) {
        let job = self.any_job(&tc);
        self.queue.retry(job);
    }

    #[rule]
    fn clear_finished(&mut self, _: TestCase) {
        let finished: BTreeSet<JobId> =
            self.queue.views(self.now).into_iter().filter(|v| v.state.is_finished()).map(|v| v.id).collect();
        self.queue.clear_finished();
        self.listed.retain(|id| !finished.contains(id));
    }

    #[rule(weight = 2)]
    fn set_per_host(&mut self, tc: TestCase) {
        let n = tc.draw(gs::integers::<usize>().max_value(4));
        self.queue.set_per_host(n);
        assert_eq!(self.queue.per_host(), n.max(1));
    }

    #[rule]
    fn stale_fact_for_a_job_that_is_over_or_unknown(&mut self, tc: TestCase) {
        let job = self.any_job(&tc);
        tc.assume(!self.queue.is_live(job));
        let item = tc.draw(gs::integers::<usize>().max_value(3));
        let file = FileId(tc.draw(gs::integers::<u32>().max_value(4)));
        let before = self.queue.views(self.now);
        self.fact(&tc, job, item, file);
        assert_eq!(self.queue.views(self.now), before, "a fact for {job:?}, which is over");
    }

    #[rule]
    fn fact_for_an_item_or_file_a_job_lacks(&mut self, tc: TestCase) {
        let job = self.any_job(&tc);
        let lengths = self.queue.jobs.iter().find(|j| j.id == job).map(|j| (j.items.len(), j.files.len()));
        tc.assume(self.queue.is_live(job));
        let (items, files) = lengths.expect("a live job is in the queue");
        let item = items + tc.draw(gs::integers::<usize>().max_value(2));
        let file = FileId((files + tc.draw(gs::integers::<usize>().max_value(2))) as u32);
        let n = tc.draw(gs::integers::<u8>().max_value(9));
        let before = self.queue.views(self.now);
        self.report(&tc, job, item, file, n);
        assert_eq!(self.queue.views(self.now), before, "a report on item {item} or {file:?} of {job:?}");
    }

    #[invariant(always_run)]
    fn no_host_runs_more_files_than_the_limit_allows(&self, _: TestCase) {
        for h in HOSTS {
            let host = HostId(h.to_owned());
            let running = self.running_on(&host);
            let allowed = self.allowed.get(&host).copied().unwrap_or(0);
            assert!(running <= allowed, "{running} files run on {host:?}, {allowed} allowed");
        }
    }

    #[invariant(always_run)]
    fn the_queue_runs_the_files_the_model_started(&self, _: TestCase) {
        let model: BTreeSet<(JobId, FileId)> = self.running.keys().copied().collect();
        assert_eq!(self.queue_running(), model);
    }

    #[invariant(always_run)]
    fn files_run_only_for_running_jobs(&self, _: TestCase) {
        for (job, file) in self.running.keys() {
            assert!(!self.cancelled.contains(job), "{file:?} of cancelled {job:?} runs");
            let state = self.queue.view(*job, self.now).map(|v| v.state);
            assert_eq!(state, Some(TaskState::Running), "{file:?} of {job:?} runs");
        }
    }

    #[invariant(always_run)]
    fn the_views_and_the_active_count_agree_with_the_model(&self, _: TestCase) {
        let views = self.queue.views(self.now);
        let ids: Vec<JobId> = views.iter().map(|v| v.id).collect();
        let newest_first: Vec<JobId> = self.listed.iter().rev().copied().collect();
        assert_eq!(ids, newest_first);
        assert_eq!(self.queue.active_count(), views.iter().filter(|v| v.state.is_active()).count());
        for v in &views {
            self.check_view(v);
        }
    }
}

#[hegel::test]
fn the_queue_keeps_its_rules_through_any_sequence_of_steps(tc: TestCase) {
    let per_host = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    let down = tc.draw(gs::btree_sets(hosts()).max_size(1));
    let mut m = QueueMachine::new(per_host);
    for h in HOSTS.map(|h| HostId(h.to_owned())).into_iter().filter(|h| !down.contains(h)) {
        m.queue.host_up(&h);
        m.up.insert(h);
    }
    // A file starts only once its job was added, named, scanned and polled: long runs get there.
    hegel::stateful::machine(m).steps(100).run(tc);
}

/// A file size as a server reports it: small, near the top, or anything.
#[hegel::composite]
fn server_sizes(tc: &TestCase) -> u64 {
    tc.draw(hegel::one_of!(
        gs::integers::<u64>().max_value(10),
        gs::sampled_from([u64::MAX / 2, u64::MAX].as_slice()),
        gs::integers::<u64>(),
    ))
}

#[hegel::test]
#[hegel::explicit_test_case(sizes = vec![1, u64::MAX])]
fn a_running_job_never_panics_on_the_sizes_a_server_sends(tc: TestCase) {
    let sizes = tc.draw(gs::vecs(server_sizes()).max_size(4));
    let (mut q, host, t0) = (Queue::new(8), HostId("a".into()), Duration::ZERO);
    q.host_up(&host);
    let items = vec![RemoteItem { path: RemotePath::new("/r/d"), kind: EntryKind::Dir, size: 0 }];
    let id = q.add(JobRequest::Download { host, items, into: PathBuf::from("/l") }, t0);
    q.poll(t0);
    q.named(id, 0, ItemDst::Local(PathBuf::from("/l/d")));
    q.poll(t0);
    let plan = |n: usize, size: u64| FilePlan::Download {
        src: RemotePath::new(&format!("/r/d/f{n}")),
        dst: PathBuf::from(format!("/l/d/f{n}")),
        size,
        mtime: None,
    };
    q.scanned(id, 0, Scan { files: sizes.iter().enumerate().map(|(n, s)| plan(n, *s)).collect(), ..Scan::default() });
    q.poll(t0);
    for (n, size) in sizes.iter().enumerate() {
        q.file_progress(id, FileId(n as u32), *size, Duration::from_secs(n as u64 + 1));
    }
    for v in q.views(Duration::from_secs(10)) {
        assert!(v.progress.done <= v.progress.total, "{v:?}");
    }
}
