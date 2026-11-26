//! Several main windows on the real service (manager, connection tasks, askpass, SFTP) against
//! the fake ssh of `tests.rs`: one connection for both windows, each window's folder listed and
//! polled for it, prompts in one window.

use std::time::Duration;

use dromon_core::{ConnStatus, PromptAnswer, RemotePath};

use super::tests::{Fixture, host};
use crate::state::{Command, ServiceEvent, WindowId};

const A: WindowId = WindowId(0);
const B: WindowId = WindowId(1);

fn window(visible: bool, focused: bool) -> Command {
    Command::Window { visible, focused }
}

/// A refresh of `path` holding `name`.
async fn refreshed(fx: &mut Fixture, path: &RemotePath, name: &str) {
    let (path, name) = (path.clone(), name.to_owned());
    fx.until(&format!("a refresh of {path} with {name}"), |ev| match ev {
        ServiceEvent::Refreshed { path: p, entries, .. } if *p == path && entries.iter().any(|e| e.name == name) => Some(()),
        _ => None,
    })
    .await
}

/// Waits for `Opened` and says which window it was for.
async fn opened(fx: &mut Fixture) -> Option<WindowId> {
    fx.until_for("Opened", |to, ev| matches!(ev, ServiceEvent::Opened { .. }).then_some(to)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_windows_on_one_host_in_two_folders_are_both_polled_and_a_hidden_one_waits() {
    let Some(mut fx) = Fixture::start() else { return };
    std::fs::create_dir_all(fx.local("sub")).unwrap();
    let start = fx.sign_in().await;
    let sub = start.join("sub");
    // B asks for the host A opened: it opens in B alone, on the same connection (no prompt).
    let mark = fx.seen.len();
    fx.send_from(B, Command::Connect { host: host("pw") });
    assert_eq!(opened(&mut fx).await, Some(B));
    assert!(!fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Prompt(_) | ServiceEvent::Status { .. })));

    // Each lists its own folder, asked for at once: both arrive whole.
    fx.send_from(A, Command::List { host: host("pw"), path: start.clone() });
    fx.send_from(B, Command::List { host: host("pw"), path: sub.clone() });
    assert_eq!(fx.listing(&start).await, [".hidden", "a.txt", "sub"]);
    assert_eq!(fx.listing(&sub).await, Vec::<String>::new());

    // Both show: a file made in each folder arrives as a refresh of that folder.
    fx.send_from(A, window(true, true));
    fx.send_from(B, window(true, false));
    std::fs::write(fx.local("in-a.txt"), "").unwrap();
    std::fs::write(fx.local("sub/in-b.txt"), "").unwrap();
    refreshed(&mut fx, &start, "in-a.txt").await;
    refreshed(&mut fx, &sub, "in-b.txt").await;

    // B is hidden: its folder waits, A's is still polled.
    fx.send_from(B, window(false, false));
    std::fs::write(fx.local("sub/while-hidden.txt"), "").unwrap();
    std::fs::write(fx.local("a-again.txt"), "").unwrap();
    refreshed(&mut fx, &start, "a-again.txt").await;
    let in_sub = sub.clone();
    let about_sub = move |ev: &ServiceEvent| matches!(ev, ServiceEvent::Refreshed { path, .. } if *path == in_sub);
    assert!(fx.none_for(Duration::from_secs(5), &about_sub).await, "nothing for the hidden window's folder");
    // B shows again: its folder catches up.
    fx.send_from(B, window(true, false));
    refreshed(&mut fx, &sub, "while-hidden.txt").await;

    // B closes: its folder isn't polled any more; A's still is.
    fx.send_from(B, Command::WindowClosed);
    std::fs::write(fx.local("sub/after-close.txt"), "").unwrap();
    std::fs::write(fx.local("a-last.txt"), "").unwrap();
    refreshed(&mut fx, &start, "a-last.txt").await;
    assert!(fx.none_for(Duration::from_secs(5), about_sub).await, "a closed window's folder isn't polled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_shows_in_the_focused_window_and_answering_it_there_closes_it_everywhere() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.send_from(A, window(true, false));
    fx.send_from(B, window(true, true));
    // A connects while B has the focus: the prompt is B's.
    fx.send_from(A, Command::Connect { host: host("pw") });
    let (to, p) = fx.until_for("a prompt", |to, ev| if let ServiceEvent::Prompt(p) = ev { Some((to, p.clone())) } else { None }).await;
    assert_eq!(to, Some(B));
    // A wrong answer in B: asked again in B, even with A focused now.
    fx.send_from(A, window(true, true));
    fx.send_from(B, window(true, false));
    fx.send_from(B, Command::AnswerPrompt { id: p.id, answer: PromptAnswer::Text("wrong".into()) });
    let closed = fx.until_for("its PromptClosed", |to, ev| matches!(ev, ServiceEvent::PromptClosed(id) if *id == p.id).then_some(to)).await;
    assert_eq!(closed, None, "closed in every window");
    let (to, again) = fx.until_for("the prompt again", |to, ev| if let ServiceEvent::Prompt(p) = ev { Some((to, p.clone())) } else { None }).await;
    assert_eq!(to, Some(B));
    fx.send_from(B, Command::AnswerPrompt { id: again.id, answer: PromptAnswer::Text("secret".into()) });
    // Up: the host opens in every window (its first connection).
    fx.status("pw", ConnStatus::Ready).await;
    assert_eq!(opened(&mut fx).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_that_closes_hands_its_prompt_on() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.send_from(A, window(true, true));
    fx.send_from(B, window(true, false));
    fx.send_from(A, Command::Connect { host: host("pw") });
    let (to, p) = fx.until_for("a prompt", |to, ev| if let ServiceEvent::Prompt(p) = ev { Some((to, p.clone())) } else { None }).await;
    assert_eq!(to, Some(A));
    // A closes before answering: nobody is focused, the window that connected is gone, so it
    // shows in every window that's left.
    fx.send_from(A, Command::WindowClosed);
    let (to, moved) = fx.until_for("the prompt again", |to, ev| if let ServiceEvent::Prompt(p) = ev { Some((to, p.clone())) } else { None }).await;
    assert_eq!((to, moved.id), (None, p.id), "the same prompt");
    fx.send_from(B, Command::AnswerPrompt { id: p.id, answer: PromptAnswer::Text("secret".into()) });
    assert_eq!(opened(&mut fx).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_prompts_sends_the_open_prompt_to_the_asking_window_and_answering_there_closes_it_in_both() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.send_from(A, window(true, true));
    fx.send_from(B, window(true, false));
    fx.send_from(A, Command::Connect { host: host("pw") });
    let (to, p) = fx.until_for("a prompt", |to, ev| if let ServiceEvent::Prompt(p) = ev { Some((to, p.clone())) } else { None }).await;
    assert_eq!(to, Some(A), "the focused window's");
    // B's Sign in: the same prompt, to B alone.
    fx.send_from(B, Command::ShowPrompts { host: host("pw") });
    let (to, again) = fx.until_for("the prompt in B", |to, ev| if let ServiceEvent::Prompt(p) = ev { Some((to, p.clone())) } else { None }).await;
    assert_eq!((to, again.id), (Some(B), p.id));
    // A wrong answer in B closes it in every window, and ssh asks again in B (the asking
    // window is the host's target now), though A is focused.
    fx.send_from(B, Command::AnswerPrompt { id: p.id, answer: PromptAnswer::Text("wrong".into()) });
    let closed = fx.until_for("its PromptClosed", |to, ev| matches!(ev, ServiceEvent::PromptClosed(id) if *id == p.id).then_some(to)).await;
    assert_eq!(closed, None, "closed in every window");
    let (to, next) = fx.until_for("the prompt again", |to, ev| if let ServiceEvent::Prompt(p) = ev { Some((to, p.clone())) } else { None }).await;
    assert_eq!(to, Some(B));
    // Signing in from B: up for both.
    fx.send_from(B, Command::AnswerPrompt { id: next.id, answer: PromptAnswer::Text("secret".into()) });
    fx.status("pw", ConnStatus::Ready).await;
    assert_eq!(opened(&mut fx).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_prompts_with_no_open_prompt_sends_nothing() {
    let Some(mut fx) = Fixture::start() else { return };
    fx.sign_in().await;
    let mark = fx.seen.len();
    fx.send_from(B, Command::ShowPrompts { host: host("pw") });
    assert!(fx.none_for(Duration::from_secs(1), |ev| matches!(ev, ServiceEvent::Prompt(_))).await);
    assert!(!fx.saw_since(mark, |ev| matches!(ev, ServiceEvent::Prompt(_))));
}
