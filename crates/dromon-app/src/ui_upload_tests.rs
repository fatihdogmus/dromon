//! The toolbar's Upload button, ⌘U and the palette's "Upload files…" driven headless: the native
//! multi-file picker opens (resolved here through the runtime), and what it picks uploads into
//! the folder on screen. The real view and handlers on the demo fixture, with a [`Recorder`] to
//! read the commands back.

use std::path::PathBuf;

use dromon_core::{ConnStatus, HostId};
use dromon_transfer::JobRequest;
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::state::{Action, App, Command, PaletteAction, Recorder, ServiceEvent, TransferCommand};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);
const SITE: &str = "/var/www/site";

fn prod() -> HostId {
    HostId("prod".into())
}

/// The demo, connected to prod at its site, with later commands going to the returned recorder.
fn recorded() -> (Runtime<App>, Recorder) {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| {
        *s = scenes::base();
        s.act(Action::SelectHost(prod()));
    });
    settle(&mut rt);
    let rec = Recorder::default();
    let backend = rec.clone();
    rt.update(move |s, _| s.backend = Box::new(backend));
    (rt, rec)
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..4 {
        rt.frame(0.25);
    }
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// ⌘ on macOS, Ctrl elsewhere, as the platform reports the primary modifier.
fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() }
}

/// The toolbar's Upload button: whether it's a Tab stop (enabled) or dimmed and inert.
fn upload_button_enabled(rt: &Runtime<App>) -> bool {
    let b = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("tb-upload") && !e.exiting).expect("the Upload button");
    assert_eq!(b.label.as_deref(), Some("Upload"));
    b.focusable
}

/// The uploads started so far: their items and folder.
fn uploads(rec: &Recorder) -> Vec<(Vec<PathBuf>, String)> {
    rec.sent()
        .into_iter()
        .filter_map(|c| match c {
            Command::Transfer(TransferCommand::Start(JobRequest::Upload { items, into, .. })) => Some((items, into.to_string())),
            _ => None,
        })
        .collect()
}

fn two() -> Vec<PathBuf> {
    vec![PathBuf::from("/Users/me/a.csv"), PathBuf::from("/Users/me/b.csv")]
}

#[test]
fn the_button_is_enabled_while_connected_and_dimmed_otherwise() {
    let (mut rt, _) = recorded();
    assert!(upload_button_enabled(&rt));
    rt.update(|s, _| {
        s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    });
    settle(&mut rt);
    assert!(!upload_button_enabled(&rt), "reconnecting");
    click(&mut rt, "tb-upload");
    assert!(rt.resolve_file_dialog_files(None).is_err(), "a dimmed button opens nothing");

    rt.update(|s, _| s.active = None);
    settle(&mut rt);
    assert!(!upload_button_enabled(&rt), "no host on screen");
}

#[test]
fn the_button_opens_the_picker_and_the_files_upload_into_the_folder_on_screen() {
    let (mut rt, rec) = recorded();
    click(&mut rt, "tb-upload");
    rt.resolve_file_dialog_files(Some(two())).expect("the picker is open");
    settle(&mut rt);
    assert_eq!(uploads(&rec), [(two(), SITE.to_owned())]);
}

#[test]
fn a_cancelled_picker_uploads_nothing() {
    let (mut rt, rec) = recorded();
    click(&mut rt, "tb-upload");
    rt.resolve_file_dialog_files(None).expect("the picker is open");
    settle(&mut rt);
    assert!(uploads(&rec).is_empty());
}

#[test]
fn cmd_u_opens_the_picker() {
    let (mut rt, rec) = recorded();
    rt.key(KeyEvent { key: Key::Command('u'), mods: primary() });
    settle(&mut rt);
    rt.resolve_file_dialog_files(Some(two())).expect("the picker is open");
    settle(&mut rt);
    assert_eq!(uploads(&rec), [(two(), SITE.to_owned())]);
}

#[test]
fn the_palette_entry_opens_the_picker() {
    let (mut rt, rec) = recorded();
    rt.update(|s, tx| {
        crate::views::run(s, Action::Palette(PaletteAction::Toggle), tx);
        crate::views::run(s, Action::Palette(PaletteAction::Pick("pal-upload".into())), tx);
    });
    settle(&mut rt);
    rt.resolve_file_dialog_files(Some(two())).expect("the picker is open");
    settle(&mut rt);
    assert_eq!(uploads(&rec), [(two(), SITE.to_owned())]);
}
