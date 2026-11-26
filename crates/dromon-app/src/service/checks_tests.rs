//! Wake and focus probes, a wake during backoff, and verbose logging, on the real service
//! against the fake ssh of `tests.rs` (`freeze` stops its sftp-server, `down` refuses to
//! connect, `-v` prints a `debug1: verbose` line). The wakes come from the test, not the clocks.

use std::time::{Duration, Instant};

use dromon_core::ConnStatus;

use super::lifecycle::WOKE;
use super::tests::{Fixture, host};
use crate::state::{Command, ServiceEvent, WindowId};

fn focus(fx: &Fixture, focused: bool) {
    fx.send(Command::Window { visible: true, focused });
}

/// Stops the fake's sftp-server, and waits until it has.
async fn freeze(fx: &Fixture) {
    let flag = fx.dir.path().join("freeze");
    std::fs::write(&flag, "").unwrap();
    while flag.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn is_line(ev: &ServiceEvent, part: &str) -> bool {
    matches!(ev, ServiceEvent::Log { line, .. } if line.contains(part))
}

const UNANSWERED: &str = "didn't answer a check";

fn lost(ev: &ServiceEvent) -> bool {
    is_line(ev, UNANSWERED) || matches!(ev, ServiceEvent::Status { status: ConnStatus::Reconnecting { .. }, .. })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_stops_answering_is_lost_on_focus_and_on_wake() {
    let Some(mut fx) = Fixture::start() else { return };
    // No folder is listed, so no poll finds the silence first.
    fx.sign_in().await;

    // Focus: the probe gets no answer within 5 s, and the reconnect takes over (the password
    // from memory), long before ServerAlive would notice (45 s).
    let mark = fx.seen.len();
    freeze(&fx).await;
    let asked = Instant::now();
    focus(&fx, true);
    fx.until("the probe to give up", |ev| is_line(ev, UNANSWERED).then_some(())).await;
    let took = asked.elapsed();
    assert!(took >= Duration::from_millis(4500) && took < Duration::from_secs(9), "{took:?}");
    fx.status("pw", ConnStatus::Reconnecting { attempt: 1 }).await;
    fx.status("pw", ConnStatus::Ready).await;
    assert!(!fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Prompt(_))), "a reconnect doesn't ask again");

    // Silent again, and focus comes back within 30 s of that probe: no probe, so nothing
    // notices.
    freeze(&fx).await;
    focus(&fx, false);
    focus(&fx, true);
    assert!(fx.none_for(Duration::from_millis(6500), lost).await, "a second focus probe within 30 s");

    // A wake always probes: logged, then lost and back.
    let mark = fx.seen.len();
    fx.wake.send(Duration::from_secs(600)).unwrap();
    fx.until("the wake's log line", |ev| is_line(ev, WOKE).then_some(())).await;
    fx.until("the probe to give up", |ev| is_line(ev, UNANSWERED).then_some(())).await;
    fx.status("pw", ConnStatus::Reconnecting { attempt: 1 }).await;
    fx.status("pw", ConnStatus::Ready).await;
    assert!(!fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Prompt(_))), "a reconnect doesn't ask again");

    // Answering: a wake's probe says so, and nothing reconnects.
    let mark = fx.seen.len();
    fx.wake.send(Duration::from_secs(60)).unwrap();
    fx.until("the probe's answer", |ev| is_line(ev, "still answers").then_some(())).await;
    assert!(!fx.saw_since(mark, lost));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wake_during_backoff_reconnects_at_once_and_starts_the_schedule_over() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.sign_in().await;
    // The network goes away: the reconnects fail, 1 s, 2 s, 5 s apart, then wait 10 s.
    std::fs::write(fx.dir.path().join("down"), "").unwrap();
    std::fs::write(fx.dir.path().join("drop"), "").unwrap();
    fx.status("pw", ConnStatus::Reconnecting { attempt: 4 }).await;
    fx.until("the 10 s wait", |ev| is_line(ev, "Reconnecting in 10 s.").then_some(())).await;

    // It's back and the computer woke: no waiting out the 10 s.
    std::fs::remove_file(fx.dir.path().join("down")).unwrap();
    let woke = Instant::now();
    fx.wake.send(Duration::from_secs(600)).unwrap();
    fx.until("the wake's log line", |ev| is_line(ev, WOKE).then_some(())).await;
    fx.status("pw", ConnStatus::Reconnecting { attempt: 1 }).await;
    fx.status("pw", ConnStatus::Ready).await;
    let took = woke.elapsed();
    assert!(took < Duration::from_secs(5), "reconnected {took:?} after the wake");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_isnt_connected_ignores_wakes_and_focus() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.send(Command::Connect { host: host("hostkey") });
    let p = fx.prompt().await;
    fx.send(Command::AnswerPrompt { id: p.id, answer: dromon_core::PromptAnswer::Cancel });
    fx.status("hostkey", ConnStatus::Failed("Host key verification failed.".into())).await;
    fx.until("the last log line", |ev| is_line(ev, "Couldn't connect").then_some(())).await;
    fx.wake.send(Duration::from_secs(600)).unwrap();
    focus(&fx, true);
    let any = |ev: &ServiceEvent| matches!(ev, ServiceEvent::Log { .. } | ServiceEvent::Status { .. });
    assert!(fx.none_for(Duration::from_millis(1000), any).await, "nothing for a failed host");
}

/// The ssh command line of the next attempt, from its log line.
async fn next_run(fx: &mut Fixture) -> String {
    fx.until("ssh to run", |ev| match ev {
        ServiceEvent::Log { line, .. } if line.starts_with("Running ") => Some(line.clone()),
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verbose_logging_reconnects_with_dash_v_and_without() {
    let Some(mut fx) = Fixture::start() else { return };
    let start = fx.sign_in().await;
    assert!(!fx.seen.iter().any(|ev| is_line(ev, " -v ")), "not verbose by default");
    fx.send(Command::List { host: host("pw"), path: start.clone() });
    fx.listing(&start).await;

    // On: every window hears it, and the connection comes back at once with -v, signing in from
    // memory and listing the folder on screen again; ssh's debug lines reach the log.
    let mark = fx.seen.len();
    fx.send(Command::Verbose { host: host("pw"), on: true });
    let to = fx.until_for("Verbose", |to, ev| matches!(ev, ServiceEvent::Verbose { on: true, .. }).then_some(to)).await;
    assert_eq!(to, None, "to every window");
    let line = next_run(&mut fx).await;
    assert!(line.ends_with(" -v -s pw sftp"), "{line}");
    fx.status("pw", ConnStatus::Ready).await;
    assert_eq!(fx.listing(&start).await, [".hidden", "a.txt", "sub"]);
    assert!(fx.saw_since(mark, |ev| is_line(ev, "debug1: verbose output from the fake ssh")));
    assert!(!fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Prompt(_) | ServiceEvent::Opened { .. })));

    // A window that connects to it now hears it's on.
    const B: WindowId = WindowId(1);
    fx.send_from(B, Command::Connect { host: host("pw") });
    let to = fx.until_for("Verbose for B", |to, ev| matches!(ev, ServiceEvent::Verbose { on: true, .. }).then_some(to)).await;
    assert_eq!(to, Some(B));

    // Off: back without it.
    let mark = fx.seen.len();
    fx.send(Command::Verbose { host: host("pw"), on: false });
    let line = next_run(&mut fx).await;
    assert!(line.ends_with(" -s pw sftp") && !line.contains(" -v "), "{line}");
    fx.status("pw", ConnStatus::Ready).await;
    fx.listing(&start).await;
    assert!(!fx.saw_since(mark, |ev| is_line(ev, "debug1: verbose output")));

    // The same again changes nothing.
    let mark = fx.seen.len();
    fx.send(Command::Verbose { host: host("pw"), on: false });
    fx.until("Verbose", |ev| matches!(ev, ServiceEvent::Verbose { on: false, .. }).then_some(())).await;
    assert!(fx.none_for(Duration::from_millis(500), |ev| matches!(ev, ServiceEvent::Status { .. })).await);
    assert!(!fx.saw_since(mark, |ev| is_line(ev, "Running ")));

    // A host that isn't connected takes it at its next connect.
    fx.send(Command::Verbose { host: host("hostkey"), on: true });
    fx.until("Verbose", |ev| matches!(ev, ServiceEvent::Verbose { on: true, .. }).then_some(())).await;
    fx.send(Command::Connect { host: host("hostkey") });
    let line = next_run(&mut fx).await;
    assert!(line.ends_with(" -v -s hostkey sftp"), "{line}");
}
