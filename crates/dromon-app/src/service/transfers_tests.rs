//! The engine against connections that drop at chosen moments. Each test does what
//! `Engine::run` does, one input at a time, so the order of a task's outcome and the hosts'
//! `HostDown` / `HostUp` is the test's choice:
//!
//! - a drop is staged by swapping the engine's session of a host for a dead one right before an
//!   action is carried out (the connection is gone, its `HostDown` isn't here yet);
//! - the outcome of that action's task is held, then handed over before the `HostDown`, between
//!   it and the `HostUp` (with a new session), after the `HostUp`, or, for two hosts, after one
//!   is back and before the other drops ([`Order`]);
//! - then the job runs on, clashes answered Keep both, and must end Done with every file once in
//!   place.
//!
//! Live sessions are `ssh -s … sftp` to the sshd fixture (`dromon-testkit`; skipped without
//! Docker unless `DROMON_REQUIRE_DOCKER=1`); dead and silent ones are in-memory pipes.

use std::path::Path;

use dromon_core::{EntryKind, HostId, RemotePath};
use dromon_testkit::{Fixture, Role, file_sha1, openssh};
use dromon_transfer::{ConflictChoice, ConflictView, JobView, RemoteItem, TaskState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use super::*;

#[derive(Clone)]
struct Quiet;

impl Sink for Quiet {
    fn emit(&self, _: ServiceEvent) {}
}

fn a() -> HostId {
    HostId("a".into())
}

fn b() -> HostId {
    HostId("b".into())
}

/// Longer than any task should take to report back: past it, the task hangs.
const HUNG: Duration = Duration::from_secs(30);
/// The seeded `a/x.bin`'s mtime.
const MTIME: i64 = 1_600_000_000;

/// A session over an in-memory pipe whose far end answers the version handshake, then is
/// dropped (every request fails at once: a dead connection) or, with `keep`, stays silent
/// (requests time out: a hung one). Keep the returned end for as long as it should stay silent.
async fn fake_session(keep: bool) -> (Sftp, Option<tokio::io::DuplexStream>) {
    let (client, mut server) = tokio::io::duplex(1 << 16);
    let handshake = tokio::spawn(async move {
        let len = server.read_u32().await.unwrap();
        let mut init = vec![0; len as usize];
        server.read_exact(&mut init).await.unwrap();
        // SSH_FXP_VERSION, version 3, no extensions.
        server.write_all(&[0, 0, 0, 5, 2, 0, 0, 0, 3]).await.unwrap();
        server
    });
    let sftp = Sftp::new(client).await.unwrap();
    let server = handshake.await.unwrap();
    (sftp, keep.then_some(server))
}

async fn dead_session() -> Sftp {
    let (sftp, _) = fake_session(false).await;
    assert_eq!(sftp.home().await, Err(Error::ConnectionLost));
    sftp
}

/// The action a drop cuts off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Name,
    FreeRemoteName,
    Scan,
    Start,
    Tidy,
}

#[derive(Clone, Debug)]
struct Cut {
    kind: Kind,
    /// Only this item's action (any when `None`).
    item: Option<usize>,
    /// The hosts whose connection drops.
    hosts: Vec<HostId>,
}

fn cut(kind: Kind, item: Option<usize>, hosts: &[HostId]) -> Cut {
    Cut { kind, item, hosts: hosts.to_vec() }
}

impl Cut {
    fn matches(&self, action: &Action) -> bool {
        let (kind, item) = match action {
            Action::Name { item, .. } => (Kind::Name, Some(*item)),
            Action::FreeRemoteName { item, .. } => (Kind::FreeRemoteName, Some(*item)),
            Action::Scan { item, .. } => (Kind::Scan, Some(*item)),
            Action::Start { .. } => (Kind::Start, None),
            Action::Tidy { item, .. } => (Kind::Tidy, Some(*item)),
            Action::Abort { .. } | Action::Cleanup { .. } => return false,
        };
        kind == self.kind && (self.item.is_none() || self.item == item)
    }
}

/// When the cut-off task's outcome arrives.
#[derive(Clone, Copy, Debug)]
enum Order {
    BeforeDown,
    BetweenDownAndUp,
    AfterUp,
    /// Two hosts: the first is back before the second drops.
    Staggered,
}

const ORDERS: [Order; 3] = [Order::BeforeDown, Order::BetweenDownAndUp, Order::AfterUp];
const ORDERS_TWO_HOSTS: [Order; 4] = [Order::BeforeDown, Order::BetweenDownAndUp, Order::AfterUp, Order::Staggered];

enum Step {
    Outcome,
    Down(HostId),
    Up(HostId),
}

fn steps(order: Order, hosts: &[HostId]) -> Vec<Step> {
    let downs = || hosts.iter().cloned().map(Step::Down);
    let ups = || hosts.iter().rev().cloned().map(Step::Up);
    match (order, hosts) {
        (Order::BeforeDown, _) => std::iter::once(Step::Outcome).chain(downs()).chain(ups()).collect(),
        (Order::BetweenDownAndUp, _) => downs().chain([Step::Outcome]).chain(ups()).collect(),
        (Order::Staggered, [first, second]) => vec![
            Step::Down(first.clone()),
            Step::Up(first.clone()),
            Step::Outcome,
            Step::Down(second.clone()),
            Step::Up(second.clone()),
        ],
        (Order::AfterUp | Order::Staggered, _) => downs().chain(ups()).chain([Step::Outcome]).collect(),
    }
}

/// Whether `outcome` is the report of `action`'s task (a file's: of run `run`).
fn answers(outcome: &Outcome, action: &Action, run: u64) -> bool {
    match (outcome, action) {
        (Outcome::Exists { job, item, .. }, Action::Name { job: j, item: i })
        | (Outcome::FreeName { job, item, .. }, Action::FreeRemoteName { job: j, item: i })
        | (Outcome::Scanned { job, item, .. }, Action::Scan { job: j, item: i, .. })
        | (Outcome::Tidied { job, item, .. }, Action::Tidy { job: j, item: i }) => (job, item) == (j, i),
        (Outcome::Finished { job, file, run: r, .. }, Action::Start { job: j, file: f, .. }) => {
            (job, file, *r) == (j, f, run)
        }
        _ => false,
    }
}

/// The jobs the tests run, on the seeded folders (`a/x.bin` dated [`MTIME`], `a/dir/inner/
/// one.txt`, `b/x.bin` a copy of `a/x.bin`; locally `up.txt` and `tree/sub/deep.txt`).
#[derive(Clone, Copy, Debug)]
enum Job {
    /// `a/x.bin` and `a/dir` from host a into the local `dl`.
    Download,
    /// `up.txt` and `tree` into `b` on host a.
    Upload,
    /// `a/x.bin` (clashes: Keep both) and `a/dir` into `b` on host a.
    Copy,
    /// The same from host a to host b.
    CrossCopy,
    CrossMove,
}

/// One test run: an engine with hosts a and b up on live sessions, and its folders.
struct Rig {
    engine: Engine<Quiet>,
    fx: &'static Fixture,
    ssh: Vec<Child>,
    /// This run's folder on the server.
    dir: RemotePath,
    local: tempfile::TempDir,
    /// Every clash the job showed.
    conflicts: Vec<ConflictView>,
}

impl Rig {
    async fn new(name: &str) -> Option<Rig> {
        let fx = dromon_testkit::fixture(name).await?;
        let dir = fx.path(Role::Key, &format!("scratch/engine-{name}-{}", std::process::id()));
        fx.exec(&[
            "sh",
            "-c",
            &format!(
                "D='{dir}'; rm -rf \"$D\"; set -e; mkdir -p \"$D/a/dir/inner\" \"$D/b\" \"$D/c\"; \
                 head -c 262144 /dev/urandom >\"$D/a/x.bin\"; TZ=UTC0 touch -m -t 202009131226.40 \"$D/a/x.bin\"; \
                 cp \"$D/a/x.bin\" \"$D/b/x.bin\"; printf 'one\\n' >\"$D/a/dir/inner/one.txt\"; chown -R {} \"$D\"",
                fx.owner(Role::Key)
            ),
        ])
        .unwrap();
        let local = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(local.path().join("tree/sub")).unwrap();
        std::fs::write(local.path().join("up.txt"), "up\n").unwrap();
        std::fs::write(local.path().join("tree/sub/deep.txt"), "deep\n").unwrap();
        let mut rig = Rig { engine: Engine::new(Quiet), fx, ssh: Vec::new(), dir, local, conflicts: Vec::new() };
        for host in [a(), b()] {
            let sftp = rig.live().await;
            rig.engine.on_message(EngineMsg::HostUp(host, sftp));
        }
        Some(rig)
    }

    /// A new session to the fixture.
    async fn live(&mut self) -> Sftp {
        let mut ssh = Command::new(openssh("ssh"))
            .args(self.fx.ssh_options())
            .args(["-s", &self.fx.destination(Role::Key), "sftp"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn ssh");
        let (stdout, stdin) = (ssh.stdout.take().unwrap(), ssh.stdin.take().unwrap());
        self.ssh.push(ssh);
        Sftp::new(tokio::io::join(stdout, stdin)).await.expect("SFTP session")
    }

    fn item(&self, rel: &str, kind: EntryKind) -> RemoteItem {
        RemoteItem { path: self.dir.join(rel), kind, size: 0 }
    }

    fn request(&self, job: Job) -> JobRequest {
        let picked = vec![self.item("a/x.bin", EntryKind::File), self.item("a/dir", EntryKind::Dir)];
        let into = self.dir.join("b");
        match job {
            Job::Download => JobRequest::Download { host: a(), items: picked, into: self.local.path().join("dl") },
            Job::Upload => {
                let items = vec![self.local.path().join("up.txt"), self.local.path().join("tree")];
                JobRequest::Upload { host: a(), items, into }
            }
            Job::Copy => JobRequest::RemoteCopy { host: a(), items: picked, into },
            Job::CrossCopy => JobRequest::CrossHostCopy { from: a(), items: picked, to: b(), into },
            Job::CrossMove => JobRequest::CrossHostMove { from: a(), items: picked, to: b(), into },
        }
    }

    fn view(&self, job: JobId) -> JobView {
        self.engine.queue.view(job, self.engine.now()).expect("the job is listed")
    }

    async fn next(&mut self) -> Outcome {
        let next = tokio::time::timeout(HUNG, self.engine.outcomes.1.recv()).await;
        next.expect("no task reported back: one hangs").expect("the engine holds a sender")
    }

    /// [`Engine::drive`], answering clashes with Keep both; carries out the first action `cut`
    /// matches on dead sessions of its hosts, and returns it with its run number and hosts.
    async fn drive(&mut self, job: JobId, cut: &mut Option<Cut>) -> Option<(Action, u64, Vec<HostId>)> {
        let mut hit = None;
        loop {
            if let Some(clash) = self.view(job).conflict {
                self.conflicts.push(clash);
                self.engine.queue.resolve(job, ConflictChoice::KeepBoth, true);
            }
            let actions = self.engine.queue.poll(self.engine.now());
            if actions.is_empty() {
                return hit;
            }
            for action in actions {
                if hit.is_none()
                    && let Some(c) = cut.take_if(|c| c.matches(&action))
                {
                    for host in &c.hosts {
                        let dead = dead_session().await;
                        self.engine.sessions.insert(host.clone(), dead);
                    }
                    hit = Some((action.clone(), self.engine.runs + 1, c.hosts));
                }
                self.engine.carry_out_one(action);
            }
        }
    }

    /// Runs `req` to the end with `cut`'s drop staged in `order`; the job's view at the end.
    async fn run(&mut self, req: JobRequest, cut: Cut, order: Order) -> JobView {
        let job = self.engine.queue.add(req.clone(), self.engine.now());
        self.engine.requests.insert(job, req);
        let mut cut = Some(cut);
        loop {
            if let Some((action, run, hosts)) = self.drive(job, &mut cut).await {
                // Outcomes of the other tasks come in meanwhile, as in `Engine::run`.
                let outcome = loop {
                    let outcome = self.next().await;
                    if answers(&outcome, &action, run) {
                        break outcome;
                    }
                    self.engine.on_outcome(outcome);
                };
                let mut held = Some(outcome);
                for step in steps(order, &hosts) {
                    match step {
                        Step::Outcome => self.engine.on_outcome(held.take().unwrap()),
                        Step::Down(host) => self.engine.on_message(EngineMsg::HostDown(host)),
                        Step::Up(host) => {
                            let sftp = self.live().await;
                            self.engine.on_message(EngineMsg::HostUp(host, sftp));
                        }
                    }
                    self.drive(job, &mut None).await;
                }
                continue;
            }
            let view = self.view(job);
            if view.state.is_finished() {
                assert!(cut.is_none(), "nothing was cut off: {cut:?}");
                return view;
            }
            match tokio::time::timeout(HUNG, self.engine.outcomes.1.recv()).await {
                Ok(Some(outcome)) => self.engine.on_outcome(outcome),
                _ => panic!("the job is stuck: nothing is under way; {view:#?}"),
            }
        }
    }

    /// Every path in the run's folder on the server, sorted.
    fn tree(&self) -> Vec<String> {
        let out = self.fx.exec(&["sh", "-c", &format!("cd '{}' && find . | LC_ALL=C sort", self.dir)]).unwrap();
        out.lines().map(str::to_owned).collect()
    }

    fn sha1(&self, rel: &str) -> String {
        self.fx.sha1(self.dir.join(rel).as_str()).unwrap()
    }

    /// The run of `job` came out right: files once, in place, with their bytes.
    fn check(&self, job: Job, view: &JobView, case: &str) {
        assert_eq!(view.state, TaskState::Done, "{case}: {view:#?}");
        let orig = self.sha1("b/x.bin");
        let seeded = ["./a/dir", "./a/dir/inner", "./a/dir/inner/one.txt", "./a/x.bin"];
        let kept_both = ["./b/dir", "./b/dir/inner", "./b/dir/inner/one.txt", "./b/x (1).bin", "./b/x.bin"];
        let mut want: Vec<&str> = vec![".", "./a"];
        match job {
            Job::Download => {
                want.extend(seeded.iter().chain(&["./b", "./b/x.bin"]));
                let dl = self.local.path().join("dl");
                assert_eq!(local_tree(&dl), ["dir", "dir/inner", "dir/inner/one.txt", "x.bin"], "{case}");
                assert_eq!(file_sha1(&dl.join("x.bin")).unwrap(), orig, "{case}");
            }
            Job::Upload => {
                want.extend(seeded.iter().chain(&[
                    "./b",
                    "./b/tree",
                    "./b/tree/sub",
                    "./b/tree/sub/deep.txt",
                    "./b/up.txt",
                    "./b/x.bin",
                ]));
            }
            Job::Copy | Job::CrossCopy => want.extend(seeded.iter().chain(&["./b"]).chain(&kept_both)),
            Job::CrossMove => want.extend(["./b"].iter().chain(&kept_both)),
        }
        want.push("./c");
        want.sort_unstable();
        assert_eq!(self.tree(), want, "{case}");
        if matches!(job, Job::Copy | Job::CrossCopy | Job::CrossMove) {
            assert_eq!(self.sha1("b/x (1).bin"), orig, "{case}");
            // The clash shows the item being copied as it is on its host, read on its session.
            let clash = self.conflicts.first().unwrap_or_else(|| panic!("{case}: no clash"));
            assert_eq!(clash.incoming.mtime, Some(MTIME), "{case}: {clash:?}");
        }
    }
}

fn local_tree(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            out.push(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            if path.is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Runs every `(job, cut)` in every order (the two-host ones too when the cut drops two hosts),
/// each on a fresh rig; checks each comes out right.
async fn cases(test: &str, cases: &[(Job, Cut)]) {
    for (n, (job, cut)) in cases.iter().enumerate() {
        let orders: &[Order] = if cut.hosts.len() == 2 { &ORDERS_TWO_HOSTS } else { &ORDERS };
        for order in orders {
            let case = format!("{test}-{n}-{order:?}");
            let Some(mut rig) = Rig::new(&case).await else { return };
            let req = rig.request(*job);
            let view = rig.run(req, cut.clone(), *order).await;
            rig.check(*job, &view, &format!("{case} ({job:?}, {cut:?})"));
        }
    }
}

/// The interleaving that left jobs in Preparing for good: Keep both lists the folder for a free
/// name; the connection drops under it, so the listing fails; the work ran again as a name
/// check, whose answer the item (renaming, no longer naming) ignored. Dead sessions only, no
/// fixture: the replay must be the listing again, in every order.
#[tokio::test(flavor = "current_thread")]
async fn keep_both_cut_off_by_a_drop_lists_the_folder_again() {
    for order in ORDERS {
        let mut engine = Engine::new(Quiet);
        engine.on_message(EngineMsg::HostUp(a(), dead_session().await));
        // A copy into its own folder keeps both without asking: straight to the free name.
        let job = own_folder_copy(&mut engine);
        engine.drive();
        let first = engine.outcomes.1.recv().await.unwrap();
        assert!(matches!(&first, Outcome::FreeName { dst: Err(Error::ConnectionLost), .. }), "{order:?}");
        let mut held = Some(first);
        for step in steps(order, &[a()]) {
            match step {
                Step::Outcome => engine.on_outcome(held.take().unwrap()),
                Step::Down(host) => engine.on_message(EngineMsg::HostDown(host)),
                Step::Up(host) => engine.on_message(EngineMsg::HostUp(host, dead_session().await)),
            }
            engine.drive();
        }
        let again = tokio::time::timeout(HUNG, engine.outcomes.1.recv()).await.expect("the work ran again").unwrap();
        assert!(
            matches!(again, Outcome::FreeName { job: j, item: 0, .. } if j == job),
            "{order:?}: the replay must list the folder again, not check the name"
        );
    }
}

/// A copy into its own folder on host a: its one item goes straight to `FreeRemoteName`.
fn own_folder_copy(engine: &mut Engine<Quiet>) -> JobId {
    let item = RemoteItem { path: RemotePath::new("/d/x.bin"), kind: EntryKind::File, size: 1 };
    let req = JobRequest::RemoteCopy { host: a(), items: vec![item], into: RemotePath::new("/d") };
    let job = engine.queue.add(req.clone(), engine.now());
    engine.requests.insert(job, req);
    job
}

#[tokio::test(flavor = "current_thread")]
async fn work_cut_off_for_a_job_cancelled_meanwhile_is_dropped() {
    let mut engine = Engine::new(Quiet);
    engine.on_message(EngineMsg::HostUp(a(), dead_session().await));
    let job = own_folder_copy(&mut engine);
    engine.drive();
    let cut_off = engine.outcomes.1.recv().await.unwrap();
    engine.on_outcome(cut_off);
    assert_eq!(engine.deferred.len(), 1, "waits for the host");
    engine.on_message(EngineMsg::Command(TransferCommand::Cancel(job)));
    engine.on_message(EngineMsg::HostDown(a()));
    engine.on_message(EngineMsg::HostUp(a(), dead_session().await));
    engine.drive();
    assert!(engine.deferred.is_empty());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(engine.outcomes.1.try_recv().is_err(), "nothing ran again for the cancelled job");
}

#[tokio::test(flavor = "current_thread")]
async fn keep_both_cut_off_by_a_drop_finishes() {
    let free_name = |hosts: &[HostId]| cut(Kind::FreeRemoteName, None, hosts);
    cases(
        "keep-both",
        &[(Job::Copy, free_name(&[a()])), (Job::CrossCopy, free_name(&[b()])), (Job::CrossMove, free_name(&[b()]))],
    )
    .await;
}

/// The check on the destination, and for a clash the read of the incoming item on its own host.
#[tokio::test(flavor = "current_thread")]
async fn name_checks_cut_off_by_a_drop_run_again() {
    let name = |hosts: &[HostId]| cut(Kind::Name, Some(0), hosts);
    cases(
        "name",
        &[
            (Job::Copy, name(&[a()])),
            (Job::Upload, name(&[a()])),
            (Job::CrossCopy, name(&[b()])),
            (Job::CrossCopy, name(&[a()])),
            (Job::CrossMove, name(&[a(), b()])),
        ],
    )
    .await;
}

/// The walk on the source and the folders made on the destination (item 1 is a folder).
#[tokio::test(flavor = "current_thread")]
async fn scans_cut_off_by_a_drop_run_again() {
    let scan = |hosts: &[HostId]| cut(Kind::Scan, Some(1), hosts);
    cases(
        "scan",
        &[
            (Job::Download, scan(&[a()])),
            (Job::Upload, scan(&[a()])),
            (Job::Copy, scan(&[a()])),
            (Job::CrossCopy, scan(&[a()])),
            (Job::CrossCopy, scan(&[b()])),
            (Job::CrossMove, scan(&[a(), b()])),
        ],
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn files_cut_off_by_a_drop_resume() {
    let start = |hosts: &[HostId]| cut(Kind::Start, None, hosts);
    cases(
        "start",
        &[
            (Job::Download, start(&[a()])),
            (Job::Upload, start(&[a()])),
            (Job::Copy, start(&[a()])),
            (Job::CrossCopy, start(&[a()])),
            (Job::CrossCopy, start(&[b()])),
            (Job::CrossMove, start(&[a(), b()])),
        ],
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tidying_cut_off_by_a_drop_runs_again() {
    cases("tidy", &[(Job::CrossMove, cut(Kind::Tidy, Some(1), &[a()]))]).await;
}

/// A file's copy finished just as its connection dropped: its report comes after the host is
/// back and the file was started again. That second run must stop (it would land the file a
/// second time, beside the first) and leave nothing behind.
#[tokio::test(flavor = "current_thread")]
async fn a_file_that_finished_as_its_connection_dropped_is_not_copied_again() {
    let Some(mut rig) = Rig::new("finished-as-dropped").await else { return };
    let req = JobRequest::CrossHostCopy {
        from: a(),
        items: vec![rig.item("a/x.bin", EntryKind::File)],
        to: b(),
        into: rig.dir.join("c"),
    };
    let job = rig.engine.queue.add(req.clone(), rig.engine.now());
    rig.engine.requests.insert(job, req);
    rig.drive(job, &mut None).await;
    let done = loop {
        match rig.next().await {
            outcome @ Outcome::Finished { result: Ok(()), .. } => break outcome,
            outcome => {
                rig.engine.on_outcome(outcome);
                rig.drive(job, &mut None).await;
            }
        }
    };
    rig.engine.on_message(EngineMsg::HostDown(b()));
    rig.drive(job, &mut None).await;
    let sftp = rig.live().await;
    rig.engine.on_message(EngineMsg::HostUp(b(), sftp));
    rig.drive(job, &mut None).await;
    assert_eq!(rig.engine.running.len(), 1, "started again");
    rig.engine.on_outcome(done);
    rig.drive(job, &mut None).await;
    assert_eq!(rig.view(job).state, TaskState::Done);
    assert!(rig.engine.running.is_empty());
    // Room for a second run to do harm, if it went on.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let tree = rig.tree();
    let c: Vec<&String> = tree.iter().filter(|p| p.starts_with("./c")).collect();
    assert_eq!(c, ["./c", "./c/x.bin"]);
    assert_eq!(rig.sha1("c/x.bin"), rig.sha1("a/x.bin"));
}

/// A request that times out on a connection that stays up: the task reports back (nothing
/// waits forever on a session that doesn't answer), and with no `HostUp` coming the work runs
/// again after [`RETRY_AFTER`]. No fixture.
#[tokio::test(flavor = "current_thread")]
async fn work_cut_off_on_a_connection_that_stays_up_runs_again_later() {
    let (silent, _server) = fake_session(true).await;
    silent.set_timeout(1);
    let mut engine = Engine::new(Quiet);
    engine.on_message(EngineMsg::HostUp(a(), silent));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "f\n").unwrap();
    let req = JobRequest::Upload { host: a(), items: vec![dir.path().join("f.txt")], into: RemotePath::new("/up") };
    let job = engine.queue.add(req.clone(), engine.now());
    engine.requests.insert(job, req);
    engine.drive();
    for round in 0..2 {
        let outcome =
            tokio::time::timeout(HUNG, engine.outcomes.1.recv()).await.expect("the check reports back").unwrap();
        assert!(matches!(&outcome, Outcome::Exists { existing: Err(Error::ConnectionLost), .. }), "round {round}");
        engine.on_outcome(outcome);
        engine.drive();
        assert_eq!(engine.deferred.len(), 1, "round {round}: waits for the host");
        assert!(!engine.retry_due(Instant::now()), "round {round}: not yet");
        assert!(engine.retry_due(Instant::now() + RETRY_AFTER), "round {round}: runs again");
        assert!(engine.deferred.is_empty());
    }
    assert!(matches!(engine.queue.view(job, engine.now()).unwrap().state, TaskState::Preparing { .. }));
}

/// The engine's own handling of commands that need no session.
mod per_host {
    use std::path::PathBuf;

    use dromon_core::{EntryKind, HostId, RemotePath};
    use dromon_transfer::{JobRequest, RemoteItem};

    use super::super::*;

    #[derive(Clone)]
    struct Quiet;

    impl Sink for Quiet {
        fn emit(&self, _: ServiceEvent) {}
    }

    #[tokio::test]
    async fn settings_change_the_per_host_limit_of_the_running_engine() {
        let mut engine = Engine::new(Quiet);
        assert_eq!(engine.queue.per_host(), PER_HOST);
        engine.on_message(EngineMsg::Command(TransferCommand::PerHost(6)));
        assert_eq!(engine.queue.per_host(), 6);
        // Out of range from a caller that didn't clamp: never zero, which would stop every host.
        engine.on_message(EngineMsg::Command(TransferCommand::PerHost(0)));
        assert_eq!(engine.queue.per_host(), 1);
        // The jobs that are there stay; the limit is for the next starts.
        let host = HostId("prod".into());
        let item = RemoteItem { path: RemotePath::new("/a"), kind: EntryKind::File, size: 1 };
        let req = JobRequest::Download { host, items: vec![item], into: PathBuf::from("/tmp/d") };
        engine.on_message(EngineMsg::Command(TransferCommand::Start(req)));
        engine.on_message(EngineMsg::Command(TransferCommand::PerHost(8)));
        assert_eq!(engine.queue.views(engine.now()).len(), 1);
        assert_eq!(engine.queue.per_host(), 8);
    }
}
