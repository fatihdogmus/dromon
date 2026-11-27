//! The live updates' machine, its debounce and what the watch's lines mean.

use super::*;

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// `events` from `phase`, each step's effects in turn.
fn run(mut phase: Phase, events: impl IntoIterator<Item = Event>) -> (Phase, Vec<Effect>) {
    let mut all = Vec::new();
    for ev in events {
        all.extend(step(&mut phase, ev));
    }
    (phase, all)
}

fn target(path: &str) -> Event {
    Event::Target(Some(p(path)))
}

fn logged(effects: &[Effect]) -> Vec<&str> {
    effects.iter().filter_map(|fx| if let Effect::Log(l) = fx { Some(l.as_str()) } else { None }).collect()
}

#[test]
fn a_folder_on_screen_starts_a_watch_which_relists_once_it_is_up() {
    let mut phase = Phase::Idle;
    assert_eq!(step(&mut phase, target("/w")), [Effect::Spawn(p("/w"))]);
    assert_eq!((phase.clone(), phase.shown()), (Phase::Starting(p("/w")), Live::Off), "not live until it watches");
    assert_eq!(step(&mut phase, Event::Changed), [], "lines before the watch starts aren't its");
    // Up: whatever changed since the listing is polled once.
    assert_eq!(step(&mut phase, Event::Ready), [Effect::Changed]);
    assert_eq!(phase.shown(), Live::On(p("/w")));
    assert_eq!(phase.watching(), Some(&p("/w")));
    assert_eq!(step(&mut phase, Event::Changed), [Effect::Changed]);
    assert_eq!(step(&mut phase, Event::Ready), [], "a second marker means nothing");
    assert_eq!(step(&mut phase, target("/w")), [], "the same folder: the watch goes on");
}

#[test]
fn navigation_moves_the_watch_and_nothing_on_screen_stops_it() {
    let (mut phase, _) = run(Phase::Idle, [target("/a"), Event::Ready]);
    assert_eq!(step(&mut phase, target("/b")), [Effect::Kill, Effect::Spawn(p("/b"))]);
    assert_eq!(phase, Phase::Starting(p("/b")));
    assert_eq!(step(&mut phase, target("/c")), [Effect::Kill, Effect::Spawn(p("/c"))], "even before it's up");
    assert_eq!(step(&mut phase, Event::Target(None)), [Effect::Kill]);
    assert_eq!(phase, Phase::Idle);
    assert_eq!(step(&mut phase, Event::Target(None)), []);
    // Hidden while watching: stopped; shown again: started again.
    let (mut phase, _) = run(Phase::Idle, [target("/a"), Event::Ready]);
    assert_eq!(step(&mut phase, Event::Target(None)), [Effect::Kill]);
    assert_eq!(step(&mut phase, target("/a")), [Effect::Spawn(p("/a"))]);
}

#[test]
fn a_server_without_inotifywait_is_not_asked_again_this_session() {
    let (mut phase, fx) = run(Phase::Idle, [target("/a"), Event::Missing]);
    assert_eq!(phase, Phase::Unavailable(LiveUnavailable::NoInotify));
    assert_eq!(fx[1], Effect::Kill);
    assert_eq!(logged(&fx), ["No live updates: inotifywait isn't installed on this server."]);
    assert_eq!(phase.shown(), Live::Unavailable(LiveUnavailable::NoInotify));
    for ev in [target("/b"), Event::Target(None), target("/a"), Event::Exited, Event::Changed, Event::Ready] {
        assert_eq!(step(&mut phase, ev.clone()), [], "{ev:?}");
    }
    assert_eq!(phase, Phase::Unavailable(LiveUnavailable::NoInotify));
    // The session came up again: asked afresh.
    assert_eq!(step(&mut phase, Event::Reset), []);
    assert_eq!(phase, Phase::Idle);
    assert_eq!(step(&mut phase, target("/b")), [Effect::Spawn(p("/b"))]);
}

#[test]
fn a_sign_in_only_the_user_could_answer_gives_up_silently_for_the_session() {
    let (phase, fx) = run(Phase::Idle, [target("/a"), Event::CantSignIn]);
    assert_eq!(phase, Phase::Unavailable(LiveUnavailable::SignIn));
    assert_eq!(
        fx,
        [
            Effect::Spawn(p("/a")),
            Effect::Kill,
            Effect::Log("No live updates: it needs a sign-in Dromon can't answer by itself.".into())
        ]
    );
    // ssh then exits: nothing more.
    let (phase, fx) = run(phase, [Event::Exited, target("/b")]);
    assert_eq!((phase, fx), (Phase::Unavailable(LiveUnavailable::SignIn), vec![]));
}

#[test]
fn a_watch_that_never_starts_is_unavailable_and_one_that_ends_leaves_its_folder_to_polls() {
    let (phase, fx) = run(Phase::Idle, [target("/a"), Event::Exited]);
    assert_eq!(phase, Phase::Unavailable(LiveUnavailable::Failed));
    assert_eq!(logged(&fx), ["No live updates: the server didn't start watching the folder."]);

    let (mut phase, _) = run(Phase::Idle, [target("/a"), Event::Ready]);
    let fx = step(&mut phase, Event::Exited);
    assert_eq!(fx, [Effect::Kill, Effect::Log("Live updates stopped; changes show every few seconds.".into())]);
    assert_eq!((phase.clone(), phase.shown()), (Phase::Ended(p("/a")), Live::Off));
    assert_eq!(step(&mut phase, Event::Changed), []);
    assert_eq!(step(&mut phase, target("/a")), [], "not again on the same folder");
    assert_eq!(step(&mut phase, target("/b")), [Effect::Spawn(p("/b"))], "another folder: tried again");
    let (mut phase, _) = run(Phase::Idle, [target("/a"), Event::Ready, Event::Exited]);
    assert_eq!(step(&mut phase, Event::Target(None)), []);
    assert_eq!(step(&mut phase, target("/a")), [Effect::Spawn(p("/a"))], "after leaving it");
}

#[test]
fn the_session_coming_up_again_restarts_whatever_ran() {
    for events in [vec![target("/a")], vec![target("/a"), Event::Ready]] {
        let (mut phase, _) = run(Phase::Idle, events);
        assert_eq!(step(&mut phase, Event::Reset), [Effect::Kill]);
        assert_eq!(phase, Phase::Idle);
    }
    let (mut phase, _) = run(Phase::Idle, [target("/a"), Event::Ready, Event::Exited]);
    assert_eq!(step(&mut phase, Event::Reset), []);
    assert_eq!(step(&mut phase, target("/a")), [Effect::Spawn(p("/a"))]);
}

#[test]
fn the_watch_lines_are_its_markers_or_a_change() {
    assert_eq!(event_of(b"dromon-watch-ready\n"), Event::Ready);
    assert_eq!(event_of(b"dromon-watch-ready\r\n"), Event::Ready);
    assert_eq!(event_of(b"dromon-watch-missing\n"), Event::Missing);
    for line in [&b"CREATE\n"[..], b"MOVED_FROM\n", b"CLOSE_WRITE,CLOSE\n", b"Q_OVERFLOW\n", b"\n", b"x"] {
        assert_eq!(event_of(line), Event::Changed, "{line:?}");
    }
    assert_eq!(event_of(b"dromon-watch-ready-not"), Event::Changed);
}

#[test]
fn changes_gather_into_one_poll_a_moment_after_the_first() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut d = Debounce::default();
    assert_eq!(d.due(), None);
    assert!(!d.take(t0));
    d.changed(t0);
    d.changed(t0 + ms(100));
    assert_eq!(d.due(), Some(t0 + DEBOUNCE), "a burst doesn't push it out");
    assert!(!d.take(t0 + ms(149)));
    assert!(d.take(t0 + ms(150)));
    assert_eq!(d.due(), None, "taken");
    // A change during that poll starts the next one.
    d.changed(t0 + ms(160));
    assert_eq!(d.due(), Some(t0 + ms(160) + DEBOUNCE));
    d.clear();
    assert_eq!(d.due(), None);
}
