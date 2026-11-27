//! Live updates (`live.rs`, `conn_live.rs`) on the real service against the fake ssh of
//! `tests.rs`, whose watch runs a fake `inotifywait` in a local shell: the second session signs
//! in from memory, goes live, moves with the folder on screen, gives up silently on a question
//! only the user could answer, and leaves the host to polls when the server can't or the watch
//! ends. The same against a real sshd is `tests/live_e2e.rs`.

use std::time::Duration;

use dromon_core::{ConnStatus, RemotePath};
use tokio::time::Instant;

use super::tests::{Fixture, host};
use crate::state::{Command, Live, LiveUnavailable, ServiceEvent};

/// Far below a poll's interval (2 s focused, 8 s while live).
const AT_ONCE: Duration = Duration::from_secs(1);

impl Fixture {
    /// The server has (the fake) `inotifywait` from now on.
    fn with_inotify(&self) {
        std::fs::write(self.dir.path().join("inotify"), "").unwrap();
    }

    async fn live(&mut self, want: Live) {
        self.until(&format!("{want:?}"), |ev| match ev {
            ServiceEvent::Live { live, .. } if *live == want => Some(()),
            _ => None,
        })
        .await
    }

    /// `name` arrives in a refresh of `path`; how long that took.
    async fn refresh_with(&mut self, path: &RemotePath, name: &str) -> Duration {
        let start = Instant::now();
        self.until(&format!("{name} in a refresh"), |ev| match ev {
            ServiceEvent::Refreshed { path: p, entries, .. } if p == path && entries.iter().any(|e| e.name == name) => {
                Some(())
            }
            _ => None,
        })
        .await;
        start.elapsed()
    }

    fn prompts_since(&self, mark: usize) -> usize {
        self.seen[mark..].iter().filter(|ev| matches!(ev, ServiceEvent::Prompt(_))).count()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_password_host_goes_live_from_memory_and_changes_show_at_once() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.with_inotify();
    let start = fx.sign_in().await;
    let mark = fx.seen.len();
    fx.send(Command::Window { visible: true, focused: true });
    fx.send(Command::List { host: host("pw"), path: start.clone() });
    fx.listing(&start).await;
    fx.live(Live::On(start.clone())).await;
    assert_eq!(fx.prompts_since(mark), 0, "the watch signed in from memory");

    std::fs::write(fx.local("new.txt"), "x").unwrap();
    let took = fx.refresh_with(&start, "new.txt").await;
    assert!(took < AT_ONCE, "took {took:?}");
    std::fs::remove_file(fx.local("new.txt")).unwrap();
    let gone = Instant::now();
    fx.until("the delete in a refresh", |ev| match ev {
        ServiceEvent::Refreshed { entries, .. } if !entries.iter().any(|e| e.name == "new.txt") => Some(()),
        _ => None,
    })
    .await;
    assert!(gone.elapsed() < AT_ONCE, "took {:?}", gone.elapsed());

    // Navigation moves the watch.
    let sub = start.join("sub");
    fx.send(Command::List { host: host("pw"), path: sub.clone() });
    fx.live(Live::On(sub.clone())).await;
    std::fs::write(fx.local("sub/inner.txt"), "x").unwrap();
    let took = fx.refresh_with(&sub, "inner.txt").await;
    assert!(took < AT_ONCE, "took {took:?}");

    // Hidden: it stops; shown again: it starts again.
    fx.send(Command::Window { visible: false, focused: false });
    fx.live(Live::Off).await;
    fx.send(Command::Window { visible: true, focused: true });
    fx.live(Live::On(sub.clone())).await;
    // The setting off and on.
    fx.send(Command::LiveUpdates { on: false });
    fx.live(Live::Off).await;
    fx.send(Command::LiveUpdates { on: true });
    fx.live(Live::On(sub.clone())).await;
    assert_eq!(fx.prompts_since(mark), 0);
    let leaked = fx.seen.iter().any(|ev| format!("{ev:?}").contains("secret"));
    assert!(!leaked, "an answer reached an event");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watch_that_ends_leaves_the_folder_to_polls() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.with_inotify();
    let start = fx.sign_in().await;
    fx.send(Command::Window { visible: true, focused: true });
    fx.send(Command::List { host: host("pw"), path: start.clone() });
    fx.live(Live::On(start.clone())).await;
    std::fs::write(fx.dir.path().join("live-stop"), "").unwrap();
    fx.live(Live::Off).await;
    assert!(
        fx.seen
            .iter()
            .any(|ev| matches!(ev, ServiceEvent::Log { line, .. } if line.starts_with("Live updates stopped")))
    );
    // Polled as before: within the focused interval, not at once.
    std::fs::write(fx.local("polled.txt"), "x").unwrap();
    let took = fx.refresh_with(&start, "polled.txt").await;
    assert!(took < Duration::from_secs(4), "took {took:?}");
    assert_eq!(fx.prompts_since(0), 1, "only the session's own password");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_without_inotifywait_polls_and_says_why() {
    let Some(mut fx) = Fixture::start() else { return };
    let start = fx.sign_in().await;
    fx.send(Command::Window { visible: true, focused: true });
    fx.send(Command::List { host: host("pw"), path: start.clone() });
    fx.live(Live::Unavailable(LiveUnavailable::NoInotify)).await;
    std::fs::write(fx.local("polled.txt"), "x").unwrap();
    let took = fx.refresh_with(&start, "polled.txt").await;
    assert!(took < Duration::from_secs(4), "took {took:?}");
    // Not asked again on another folder.
    let mark = fx.seen.len();
    fx.send(Command::List { host: host("pw"), path: start.join("sub") });
    let tried = |ev: &ServiceEvent| matches!(ev, ServiceEvent::Live { .. });
    assert!(fx.none_for(Duration::from_millis(500), tried).await, "{:#?}", &fx.seen[mark..]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_question_only_the_user_could_answer_is_never_shown_for_the_watch() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.with_inotify();
    fx.send(Command::Connect { host: host("hostkey") });
    let p = fx.prompt().await;
    fx.answer(&p, "yes");
    fx.status("hostkey", ConnStatus::Ready).await;
    let mark = fx.seen.len();
    fx.send(Command::Window { visible: true, focused: true });
    fx.send(Command::List { host: host("hostkey"), path: fx.root() });
    fx.live(Live::Unavailable(LiveUnavailable::SignIn)).await;
    assert_eq!(fx.prompts_since(mark), 0, "{:#?}", &fx.seen[mark..]);
    let quiet = |ev: &ServiceEvent| matches!(ev, ServiceEvent::Prompt(_) | ServiceEvent::Live { .. });
    assert!(fx.none_for(Duration::from_millis(500), quiet).await, "and isn't tried again");
    // The session itself is untouched.
    let dropped = |ev: &ServiceEvent| matches!(ev, ServiceEvent::Status { status, .. } if *status != ConnStatus::Ready);
    assert!(!fx.saw_since(mark, dropped));
}
