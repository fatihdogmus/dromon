//! The display preferences driven headless (§6.6, §6.9): reduce motion, from the OS
//! (`Runtime::set_system_reduced_motion`) or forced by the app (`UiState::access`), turns every
//! slide, scale, shake and sweep the app has (but the list's row animations, `views/list.rs`)
//! into an instant change or a cross-fade; increased contrast and reduce transparency switch
//! the tokens the views paint with.

use dromon_core::{ConnStatus, HostId, Prompt, PromptId, PromptKind};
use dromon_transfer::{JobId, JobKind, JobView, Progress, TaskState};
use rune::{ElementInfo, Env, Runtime};

use crate::state::{Action, App, DropTarget, FileAction, MenuKind, ServiceEvent};
use crate::theme::Access;
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);

/// Where reduce motion comes from, if anywhere.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Motion {
    Full,
    /// The OS asks (macOS Reduce motion, Windows' animation effects off, the XDG portal).
    System,
    /// The app forces it (`UiState::access.reduce_motion`).
    Forced,
}

const REDUCED: [Motion; 2] = [Motion::System, Motion::Forced];

fn runtime(motion: Motion, start: fn(&mut App)) -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.set_system_reduced_motion(motion == Motion::System);
    rt.update(move |s, _| {
        start(s);
        s.ui.access.reduce_motion = motion == Motion::Forced;
    });
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

/// `k` on screen (not on its way out); `k*` is the first whose key starts with `k`.
fn get(rt: &Runtime<App>, k: &str) -> ElementInfo {
    let hit = |key: &str| match k.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => key == k,
    };
    rt.elements().into_iter().find(|e| e.key.as_deref().is_some_and(hit) && !e.exiting).unwrap_or_else(|| panic!("no element {k:?}"))
}

/// How far `e` is drawn from where it lands, in points.
fn off(e: &ElementInfo) -> f32 {
    e.rect.iter().zip(&e.target).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
}

/// After `change`, 60 ms in: element `k`'s distance from its place, its scale and opacity.
fn early(motion: Motion, start: fn(&mut App), change: impl FnOnce(&mut App) + 'static, k: &str) -> (f32, f32, f32) {
    let mut rt = runtime(motion, start);
    // As a click or the service's answer would: animated (a change nothing in the window
    // caused lands at once in an app's window).
    rt.update(move |s, tx| {
        tx.animated();
        change(s);
    });
    rt.frame(0.0);
    rt.frame(0.06);
    let e = get(&rt, k);
    let seen = (off(&e), e.scale, e.opacity);
    settle(&mut rt);
    let e = get(&rt, k);
    assert!(off(&e) < 0.01 && (e.scale - 1.0).abs() < 1e-3 && e.opacity > 0.999, "{k} lands where it belongs ({motion:?})");
    seen
}

/// A surface that slides or scales in normally only fades in under reduce motion.
fn fades_in_place(start: fn(&mut App), change: fn(&mut App), k: &str) {
    let (d, scale, _) = early(Motion::Full, start, change, k);
    assert!(d > 1.0 || scale < 0.99, "{k} moves in with full motion (off {d}, scale {scale})");
    only_fades(start, change, k);
}

/// Under reduce motion, `k` appears where it lands, at full size, fading in.
fn only_fades(start: fn(&mut App), change: fn(&mut App), k: &str) {
    for m in REDUCED {
        let (d, scale, opacity) = early(m, start, change, k);
        assert!(d < 0.01, "{k} doesn't travel under reduce motion ({m:?}): {d} pt off");
        assert!((scale - 1.0).abs() < 1e-3, "{k} doesn't scale under reduce motion ({m:?}): {scale}");
        assert!(opacity < 0.99, "{k} cross-fades in under reduce motion ({m:?}): opacity {opacity}");
    }
}

fn prod() -> HostId {
    HostId("prod".into())
}

// A sheet, a menu or the first toast mounts with its layer, and Rune plays only the layer's
// fade then; a sheet that takes another's place drops, and a toast joining others rises.

#[test]
fn a_sheet_fades_in_instead_of_dropping() {
    only_fades(scenes::welcome, scenes::auth_password, "sheet");
    // Closing the window while it's up: the quit sheet takes the sign-in sheet's place.
    let quit = |s: &mut App| {
        s.apply(ServiceEvent::Transfers(vec![running(1, 10)]));
        s.close_requested();
    };
    fades_in_place(scenes::auth_password, quit, "quit");
}

#[test]
fn the_transfers_popover_fades_in_instead_of_growing() {
    fades_in_place(scenes::toolbar_ring, |s| drop(s.act(Action::ToggleTransfers)), "transfers");
}

#[test]
fn the_drawn_menu_fades_in_instead_of_growing() {
    let open = |s: &mut App| drop(s.act(Action::File(FileAction::OpenMenu { kind: MenuKind::Empty, at: Some((640.0, 560.0)) })));
    only_fades(scenes::listing, open, "ctx-menu");
}

/// Deletes `name` on prod (nothing answers).
fn delete(s: &mut App, name: &str) {
    s.act(Action::Click { name: name.into(), extend: false, toggle: false });
    s.act(Action::File(FileAction::Delete));
    s.act(Action::File(FileAction::ConfirmDelete));
}

/// One delete running (its toast up), nothing answering.
fn deleting(s: &mut App) {
    scenes::listing(s);
    s.backend = Box::new(crate::state::Silent::default());
    delete(s, "assets");
}

#[test]
fn a_toast_fades_in_instead_of_rising() {
    only_fades(scenes::listing, scenes::delete_failed, "toast-op-*");
    // A second delete's toast joins the first one's.
    fades_in_place(deleting, |s| delete(s, "uploads"), "toast-op-2");
}

#[test]
fn the_drop_pill_fades_in_instead_of_rising() {
    let hover = |s: &mut App| drop(s.act(Action::DropHover { target: DropTarget::List, over: true, count: 3 }));
    fades_in_place(scenes::listing, hover, "drop-pill-row");
}

#[test]
fn the_drag_pill_fades_in_instead_of_rising() {
    let over = |s: &mut App| {
        s.act(Action::File(FileAction::DragStart("index.html".into())));
        s.act(Action::File(FileAction::DragOver { target: Some(crate::state::RowTarget::Folder("uploads".into())), copy: false }));
    };
    fades_in_place(scenes::listing, over, "drop-pill-row");
}

/// The password was wrong: ssh asks again, and the sheet's field shakes once.
fn signed_in_wrong(s: &mut App) {
    s.act(Action::SubmitPrompt);
    let error = Some(dromon_ssh::WRONG_PASSWORD.to_owned());
    let text = "deploy@prod.example.com's password: ".into();
    s.apply(ServiceEvent::Prompt(Prompt { id: PromptId(2), host: prod(), kind: PromptKind::Password, text, error }));
}

/// package-lock.json being renamed (its field up, unchanged).
fn renaming(s: &mut App) {
    scenes::listing(s);
    s.act(Action::Click { name: "package-lock.json".into(), extend: false, toggle: false });
    s.act(Action::File(FileAction::Rename));
}

/// Renamed to a name that's taken: the field stays, with the error, and shakes once.
fn renamed_to_a_taken_name(s: &mut App) {
    s.files.field_edit().replace_text("package.json");
    s.files.field_edit().mark_edited();
    s.act(Action::File(FileAction::EditSubmit));
}

#[test]
fn a_wrong_answer_does_not_shake_the_field_under_reduce_motion() {
    let sign_in = |m| early(m, scenes::auth_password, signed_in_wrong, "sheet-field-2").0;
    let rename = |m| early(m, renaming, renamed_to_a_taken_name, "edit-field-1").0;
    assert!(sign_in(Motion::Full) > 1.0, "the sign-in field shakes");
    assert!(rename(Motion::Full) > 1.0, "the name field shakes");
    for m in REDUCED {
        assert!(sign_in(m) < 0.01, "the sign-in field doesn't shake ({m:?})");
        assert!(rename(m) < 0.01, "the name field doesn't shake ({m:?})");
    }
}

fn running(id: u64, done: u64) -> JobView {
    JobView {
        id: JobId(id),
        kind: JobKind::Download,
        host: prod(),
        title: "site".into(),
        state: TaskState::Running,
        progress: Progress { done, total: 100 },
        files_done: 0,
        files_total: 1,
        speed: None,
        eta: None,
        waiting_for_connection: false,
        conflict: None,
        reveal: None,
        errors: Vec::new(),
        skipped: Vec::new(),
        folders: Vec::new(),
    }
}

/// Whether anything inside `k` is still animating 60 ms after progress goes from 10% to 40%.
fn sweeping(motion: Motion, k: &str) -> bool {
    let mut rt = runtime(motion, |s| {
        scenes::listing(s);
        s.apply(ServiceEvent::Transfers(vec![running(1, 10)]));
        s.act(Action::ToggleTransfers);
    });
    rt.update(|s, tx| {
        tx.animated();
        s.apply(ServiceEvent::Transfers(vec![running(1, 40)]));
    });
    rt.frame(0.0);
    rt.frame(0.06);
    let path = get(&rt, k).path;
    rt.elements().iter().any(|e| e.path.starts_with(&path) && e.moving)
}

#[test]
fn progress_jumps_instead_of_sweeping_under_reduce_motion() {
    assert!(sweeping(Motion::Full, "tb-ring"), "the toolbar ring sweeps");
    assert!(sweeping(Motion::Full, "tr-bar-1"), "a row's bar grows");
    for m in REDUCED {
        assert!(!sweeping(m, "tb-ring"), "the ring jumps ({m:?})");
        assert!(!sweeping(m, "tr-bar-1"), "the bar jumps ({m:?})");
    }
}

/// The connecting skeleton's opacity at a few moments of its 1.6 s pulse.
fn pulse(motion: Motion) -> Vec<f32> {
    let mut rt = runtime(motion, scenes::connecting);
    (0..4)
        .map(|_| {
            rt.frame(0.4);
            get(&rt, "skeleton").opacity
        })
        .collect()
}

#[test]
fn loops_hold_still_under_reduce_motion() {
    let spread = |v: &[f32]| v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min);
    assert!(spread(&pulse(Motion::Full)) > 0.1, "the skeleton pulses");
    for m in REDUCED {
        assert!(spread(&pulse(m)) < 1e-3, "the skeleton holds still ({m:?}): {:?}", pulse(m));
    }
}

#[test]
fn the_view_hands_the_preferences_to_every_part() {
    // The OS's reduce motion and increased contrast, and what the app forces on, all count.
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| scenes::listing(s));
    rt.frame(0.0);
    let border = |rt: &Runtime<App>| get(rt, "tb-filter").border_color.expect("the filter field has an outline");
    let plain = border(&rt);
    rt.set_system_increased_contrast(true);
    rt.frame(0.0);
    settle(&mut rt);
    let more = border(&rt);
    assert!(more.a > plain.a + 0.05, "increased contrast strengthens the outline ({} → {})", plain.a, more.a);
    let fill = |rt: &Runtime<App>| get(rt, "tb-filter").fill;
    assert!((fill(&rt).a - 1.0).abs() < 1e-3, "and makes the field solid");

    rt.set_system_increased_contrast(false);
    rt.update(|s, _| s.ui.access = Access { reduce_transparency: true, ..Access::default() });
    settle(&mut rt);
    assert!((fill(&rt).a - 1.0).abs() < 1e-3, "reduce transparency makes the field solid");
    assert!((border(&rt).a - plain.a).abs() < 1e-3, "and keeps its outline");

    rt.update(|s, _| s.ui.access = Access::default());
    settle(&mut rt);
    assert!(fill(&rt).a < 0.5, "back to the translucent field");
    // The banner while reconnecting.
    rt.update(|s, _| {
        s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
        s.ui.access.reduce_transparency = true;
    });
    settle(&mut rt);
    assert!((get(&rt, "banner").fill.a - 1.0).abs() < 1e-3, "the banner is opaque");
}
