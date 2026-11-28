//! Properties of the window as a whole: any run of what a user does on the demo (picking hosts,
//! rows by names that are there and that aren't, the history, the filter and search, sorting,
//! hidden files, the palette, file operations, the bug report and its recording, Escape and the
//! keys) never panics the app, and after every step the selection names only rows on screen,
//! the host on screen is a known, open one, the history stays within its limit, a recording
//! takes at most its fifty steps, and the report's link keeps within its budget.
//!
//! The run's drawing is shared with `record_props.rs`, which records such runs.

use std::collections::BTreeSet;

use dromon_core::{HostId, RemotePath, SortKey};
use hegel::TestCase;
use hegel::generators as gs;
use rune::TextEdit;

use super::keys::FileKey;
use super::report::{About, ReportAction, URL_BUDGET, record::MAX_STEPS};
use super::*;
use crate::fixture;

/// A clock that stands still: the fixture's is shared by the process, and other tests move it.
fn still() -> i64 {
    0
}

/// The app on the demo, its clock still and its version pinned.
pub(super) fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.wall_ms = still;
    s.report.about = Some(About { version: "9.9.0".into(), os: "Linux".into(), arch: "x86_64".into(), desktop: None });
    s
}

/// Folders of the demo's hosts, some listed, some denied, some not there.
const PATHS: &[&str] = &[
    "/",
    "/var",
    "/var/www",
    "/var/www/site",
    "/var/www/site/releases",
    "/root",
    "/var/lib/mysql",
    "/home",
    "/home/deploy",
    "/srv/media",
    "/volume1",
    "/nope",
    "/var/www/site/index.html",
];

/// What a run did that a check must know.
#[derive(Debug, Default)]
pub(super) struct Run {
    /// Names clicked that weren't on screen: the view never sends those, but a click can
    /// race a listing, so they may stay selected until the rows change.
    pub ghosts: BTreeSet<String>,
    /// What was typed into a name, the filter, a search or ⌘L.
    pub typed: Vec<String>,
    /// Recording: no action that would end or replace the recording.
    pub recording: bool,
    /// The window's clock for type-to-select and search, in seconds.
    pub now: f64,
}

/// One thing the user does.
#[derive(Clone, Debug)]
pub(super) enum Do {
    Act(Action),
    Key(Shortcut, f64),
    /// Types into the name field (a rename or a new folder), if it's up.
    Name(String),
    /// Types into the palette's field.
    Palette(String),
}

/// A name someone might type: one hard to mistake for anything else (shaped `Abcdef12.ext`,
/// what the recording's privacy check looks for), or anything at all.
#[hegel::composite]
pub(super) fn typed(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(gs::from_regex(r"[A-Z][a-z]{4,7}[0-9]{2}(\.[a-z]{2,3})?"), gs::text().max_size(12)))
}

/// Whether `s` is a name `typed` drew to be told apart: `Abcdef12`, maybe with an extension.
pub(super) fn distinctive(s: &str) -> Option<&str> {
    let stem = s.split_once('.').map_or(s, |(stem, _)| stem);
    let b = stem.as_bytes();
    let shaped = b.len() >= 7
        && b[0].is_ascii_uppercase()
        && b[1..b.len() - 2].iter().all(u8::is_ascii_lowercase)
        && b[b.len() - 2..].iter().all(u8::is_ascii_digit);
    shaped.then_some(stem)
}

fn rows(s: &App) -> Vec<String> {
    s.conn().map(|c| c.row_names().into_iter().map(str::to_owned).collect()).unwrap_or_default()
}

/// A row on screen, if there's one.
fn row(tc: &TestCase, s: &App) -> Option<String> {
    let rows = rows(s);
    (!rows.is_empty()).then(|| tc.draw(gs::sampled_from(rows)))
}

/// A row on screen mostly, else a name that isn't one.
fn row_or_not(tc: &TestCase, s: &App) -> String {
    match row(tc, s) {
        Some(r) if tc.draw(gs::weighted_booleans(0.8)) => r,
        _ => tc.draw(typed()),
    }
}

fn host(tc: &TestCase, s: &App) -> HostId {
    let mut ids: Vec<HostId> = s.hosts.hosts.iter().map(|h| h.id.clone()).collect();
    ids.push(HostId("nope".into()));
    tc.draw(gs::sampled_from(ids))
}

fn browse(tc: &TestCase, s: &App, run: &mut Run) -> Do {
    let shift = tc.draw(gs::booleans());
    Do::Act(match tc.draw(gs::integers::<u8>().max_value(15)) {
        0 | 1 => Action::SelectHost(host(tc, s)),
        2 => Action::Open(row_or_not(tc, s)),
        3 | 4 => match row(tc, s) {
            Some(name) => Action::Click { name, extend: shift, toggle: tc.draw(gs::booleans()) },
            None => Action::SelectAll,
        },
        5 => {
            let name = tc.draw(typed());
            run.ghosts.insert(name.clone());
            Action::Click { name, extend: false, toggle: shift }
        }
        6 => Action::GoUp,
        7 => Action::Back,
        8 => Action::Forward,
        9 => {
            let path = tc.draw(gs::sampled_from(PATHS));
            Action::GoTo(RemotePath::new(if shift { path } else { "/var/www/site" }))
        }
        10 => Action::Move { delta: tc.draw(gs::sampled_from(vec![-16, -2, -1, 1, 3, 16])), extend: shift },
        11 => Action::End { last: tc.draw(gs::booleans()), extend: shift },
        12 => {
            run.now += tc.draw(gs::sampled_from(vec![0.1, 0.5, 2.0]));
            Action::Type { text: tc.draw(gs::from_regex("[a-zA-Z. ]")), now: run.now }
        }
        13 => Action::Sort(tc.draw(gs::sampled_from(vec![SortKey::Name, SortKey::Size, SortKey::Modified]))),
        14 => Action::ToggleHidden,
        _ => Action::OpenSelection,
    })
}

fn filter_and_more(tc: &TestCase, s: &App, run: &mut Run) -> Do {
    let text = tc.draw(typed());
    Do::Act(match tc.draw(gs::integers::<u8>().max_value(15)) {
        0 => Action::OpenFilter,
        1 => {
            run.typed.push(text.clone());
            Action::FilterChanged(text)
        }
        2 => Action::CloseFilter,
        3 | 4 => Action::Escape,
        5 => Action::Search(SearchAction::Open),
        6 => {
            run.now += 1.0;
            run.typed.push(text.clone());
            Action::Search(SearchAction::Typed { text, now: run.now })
        }
        7 => {
            run.now += 1.0;
            Action::Search(SearchAction::Tick(run.now))
        }
        8 => Action::Preview(PreviewAction::Toggle),
        9 => Action::Preview(PreviewAction::Close),
        10 => Action::Download,
        11 => Action::ToggleTransfers,
        12 => Action::OpenLog,
        13 => Action::DismissSheet,
        14 => Action::SubmitPrompt,
        _ => match s.active.clone() {
            Some(h) if tc.draw(gs::weighted_booleans(0.3)) => Action::Disconnect(h),
            _ => Action::DeclinePrompt,
        },
    })
}

fn file(tc: &TestCase, s: &App, run: &mut Run) -> Do {
    use FileAction as F;
    let on_row = |tc: &TestCase| row(tc, s).map(MenuKind::Row).unwrap_or(MenuKind::Empty);
    Do::Act(Action::File(match tc.draw(gs::integers::<u8>().max_value(14)) {
        0 => F::NewFolder,
        1 => F::Rename,
        2 | 3 => {
            let name = tc.draw(typed());
            run.typed.push(name.clone());
            return Do::Name(name);
        }
        4 => F::EditSubmit,
        5 => F::EditCancel,
        6 => F::Delete,
        7 => F::ConfirmDelete,
        8 => F::Refresh,
        9 => F::Properties,
        10 => tc.draw(gs::sampled_from(vec![F::CopyPath, F::Copy, F::Cut])),
        11 => F::OpenMenu { kind: on_row(tc), at: None },
        12 => F::CloseMenu,
        _ => {
            let id = tc.draw(gs::sampled_from(vec!["rename", "delete", "properties", "download", "copy-path"]));
            F::Pick { kind: on_row(tc), id: id.to_owned() }
        }
    }))
}

fn palette(tc: &TestCase, s: &App, run: &mut Run) -> Do {
    let item_keys: Vec<String> = s
        .palette_items()
        .into_iter()
        .map(|i| i.key)
        .filter(|k| !run.recording || !(k.contains("report") || k.contains("record")))
        .collect();
    Do::Act(Action::Palette(match tc.draw(gs::integers::<u8>().max_value(7)) {
        0 => PaletteAction::Toggle,
        1 => PaletteAction::OpenPath,
        2 => {
            let text = match tc.draw(gs::booleans()) {
                true => format!("/var/www/{}", tc.draw(typed())),
                false => tc.draw(gs::sampled_from(PATHS)).to_owned(),
            };
            run.typed.push(text.clone());
            return Do::Palette(text);
        }
        3 => PaletteAction::Move(tc.draw(gs::sampled_from(vec![-1, 1]))),
        4 | 5 => PaletteAction::Run,
        6 if !item_keys.is_empty() => PaletteAction::Pick(tc.draw(gs::sampled_from(item_keys))),
        _ => PaletteAction::Close,
    }))
}

fn key(tc: &TestCase, run: &mut Run) -> Do {
    use Shortcut as K;
    let files =
        [FileKey::NewFolder, FileKey::Rename, FileKey::Delete, FileKey::Copy, FileKey::Refresh, FileKey::Properties];
    let sc = match tc.draw(gs::integers::<u8>().max_value(12)) {
        0 => tc.draw(gs::sampled_from(vec![K::Back, K::Forward, K::Up, K::Open, K::SelectAll, K::ToggleHidden])),
        1 => tc.draw(gs::sampled_from(vec![K::Filter, K::Search, K::Download, K::Escape, K::Preview])),
        2 => K::Move { delta: tc.draw(gs::sampled_from(vec![-1, 1])), extend: tc.draw(gs::booleans()) },
        3 => K::Page { down: tc.draw(gs::booleans()), extend: tc.draw(gs::booleans()) },
        4 => K::End { last: tc.draw(gs::booleans()), extend: tc.draw(gs::booleans()) },
        5 | 6 => K::Type(tc.draw(gs::from_regex("[a-z]"))),
        7 => K::File(tc.draw(gs::sampled_from(files.to_vec()))),
        8 => K::Host(tc.draw(gs::integers::<usize>().max_value(10))),
        9 => tc.draw(gs::sampled_from(vec![K::QuickConnect, K::Palette, K::GoToPath, K::ShowTransfers])),
        10 => K::ContextMenu,
        _ => K::Escape,
    };
    run.now += tc.draw(gs::sampled_from(vec![0.2, 1.5]));
    Do::Key(sc, run.now)
}

fn report(tc: &TestCase) -> Do {
    let all = vec![
        ReportAction::Open(None),
        ReportAction::Record,
        ReportAction::StopRecording,
        ReportAction::Cancel,
        ReportAction::Copy,
        ReportAction::OpenGitHub,
    ];
    Do::Act(Action::Report(tc.draw(gs::sampled_from(all))))
}

/// The next thing the user does, drawn from what's on screen now.
pub(super) fn next(tc: &TestCase, s: &App, run: &mut Run) -> Do {
    let reports = if run.recording { 0 } else { 1 };
    match tc.draw(gs::integers::<u8>().max_value(11 + reports)) {
        0..=3 => browse(tc, s, run),
        4 | 5 => filter_and_more(tc, s, run),
        6 | 7 => file(tc, s, run),
        8 | 9 => palette(tc, s, run),
        10 | 11 => key(tc, run),
        _ => report(tc),
    }
}

/// Does `what` in `s`, as the view would.
pub(super) fn perform(s: &mut App, what: Do) {
    match what {
        Do::Act(action) => {
            s.act(action);
        }
        Do::Key(sc, now) => {
            s.shortcut(sc, now);
        }
        Do::Name(text) => {
            if let Some(field) = s.files.edit.field_mut() {
                field.edit = TextEdit::new(&text);
                s.act(Action::File(FileAction::EditChanged));
            }
        }
        Do::Palette(text) => {
            s.ui.palette.edit = TextEdit::new(&text);
            s.act(Action::Palette(PaletteAction::Edited));
        }
    }
}

/// What holds after every step.
fn check(s: &App, run: &Run) {
    for (host, c) in &s.conns {
        let rows: BTreeSet<&str> = c.row_names().into_iter().collect();
        let known = |n: &str| rows.contains(n) || run.ghosts.contains(n);
        assert!(c.selection.iter().all(known), "{host}: a selected name isn't a row on screen");
        for held in [c.selection.cursor(), c.selection.anchor()].into_iter().flatten() {
            assert!(known(held), "{host}: the cursor or anchor {held:?} isn't a row on screen");
        }
        // What Back and Forward lead to: at most the history's limit each way.
        assert!(c.history.recent().len() <= 2 * history::HISTORY_LIMIT);
    }
    if let Some(host) = &s.active {
        assert!(s.hosts.get(host).is_some() && s.conns.contains_key(host), "{host} on screen isn't open");
    }
    assert!(s.recording_steps().is_none_or(|n| n <= MAX_STEPS));
    if let Some(d) = &s.report.draft {
        assert!(d.url.len() <= URL_BUDGET, "the link has {} characters", d.url.len());
    }
}

// Each case builds the app on the demo and takes up to thirty steps: fewer cases than the
// default keep this to seconds.
#[hegel::test(test_cases = 50)]
fn any_run_of_actions_keeps_the_window_whole(tc: TestCase) {
    let mut s = demo();
    let mut run = Run::default();
    let first = start(&tc);
    perform(&mut s, first);
    let steps = tc.draw(gs::integers::<usize>().min_value(1).max_value(30));
    for i in 0..steps {
        let what = next(&tc, &s, &mut run);
        tc.note(&format!("{i}: {what:?}"));
        perform(&mut s, what);
        check(&s, &run);
    }
}

/// The first step of a run: a host with folders, so the run goes somewhere.
pub(super) fn start(tc: &TestCase) -> Do {
    let host = tc.draw(gs::sampled_from(vec!["prod", "prod", "nas", "staging", "pi"]));
    Do::Act(Action::SelectHost(HostId(host.into())))
}
