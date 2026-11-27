//! `Fixture::exec` from an async test: while a long command runs, the runtime's other tasks keep
//! running (in the server matrix, the other servers' SFTP sessions, whose replies would otherwise
//! wait out their timeout and fail as a lost connection).

use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn exec_leaves_the_runtimes_other_tasks_running() {
    let Some(fx) = dromon_testkit::fixture("exec_leaves_the_runtimes_other_tasks_running").await else { return };
    let ticker = tokio::spawn(async {
        let start = Instant::now();
        let (mut last, mut longest) = (Instant::now(), Duration::ZERO);
        while start.elapsed() < Duration::from_secs(3) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            longest = longest.max(last.elapsed());
            last = Instant::now();
        }
        longest
    });
    // From a task on the runtime's only worker, as a matrix check calls it.
    tokio::spawn(async move { fx.exec(&["sleep", "2"]).unwrap() }).await.unwrap();
    let longest = ticker.await.unwrap();
    assert!(longest < Duration::from_secs(1), "a task waited {longest:?} while exec ran");
}
