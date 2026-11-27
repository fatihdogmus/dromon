//! The toolbar's add button (`tb-upload`) and its menu, ⌘U and the palette's "Upload files…" and
//! "Upload folder…" driven headless: the native multi-file or folder picker opens (resolved here
//! through the runtime), and what it picks uploads into the folder on screen. The real view and
//! handlers on the demo fixture, with a [`Recorder`] to read the commands back.

use std::path::PathBuf;

use dromon_core::{ConnStatus, HostId};
use dromon_transfer::JobRequest;
use rune::{ElementInfo, Env, FileDialogKind, Key, KeyEvent, Mods, Runtime};

use crate::state::{Action, App, Command, MenuKind, PaletteAction, Recorder, ServiceEvent, TransferCommand};
use crate::{app, scenes};

const SIZE: (f32, f32) = (1080.0, 680.0);
const SITE: &str = "/var/www/site";
const ADD: &str = "tb-upload";

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

fn element(rt: &Runtime<App>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

/// The add button: whether it's a Tab stop (enabled) or faded and inert.
fn add_button_enabled(rt: &Runtime<App>) -> bool {
    element(rt, ADD).expect("the add button").focusable
}

fn menu_shown(rt: &Runtime<App>) -> bool {
    element(rt, "ctx-menu").is_some()
}

/// The drawn menu's items, by label, in order.
fn items(rt: &Runtime<App>) -> Vec<String> {
    rt.elements().into_iter().filter(|e| !e.exiting && e.role == "menuitem").filter_map(|e| e.label).collect()
}

/// The uploads started so far: their items and folder.
fn uploads(rec: &Recorder) -> Vec<(Vec<PathBuf>, String)> {
    rec.sent()
        .into_iter()
        .filter_map(|c| match c {
            Command::Transfer(TransferCommand::Start(JobRequest::Upload { items, into, .. })) => {
                Some((items, into.to_string()))
            }
            _ => None,
        })
        .collect()
}

fn two() -> Vec<PathBuf> {
    vec![PathBuf::from("/Users/me/a.csv"), PathBuf::from("/Users/me/b.csv")]
}

fn photos() -> PathBuf {
    PathBuf::from("/Users/me/photos")
}

#[test]
fn the_button_is_named_for_the_folder_and_says_it_opens_a_menu() {
    let (mut rt, _) = recorded();
    let b = element(&rt, ADD).expect("the add button");
    assert_eq!((b.role, b.label.as_deref(), b.help.as_deref()), ("button", Some("Add to site"), Some("Add to site")));
    assert_eq!(b.expanded, Some(false), "a menu button, closed");
    // At the root it's the host's name, as the first breadcrumb says.
    rt.update(|s, _| drop(s.act(Action::GoTo(dromon_core::RemotePath::new("/")))));
    settle(&mut rt);
    assert_eq!(element(&rt, ADD).unwrap().label.as_deref(), Some("Add to prod"));
    // It sits right after Back and Forward, before the path.
    let x = |k: &str| element(&rt, k).unwrap_or_else(|| panic!("{k}")).rect[0];
    assert!(x("tb-forward") < x(ADD) && x(ADD) < x("tb-path"), "{} {} {}", x("tb-forward"), x(ADD), x("tb-path"));
}

#[test]
fn the_button_is_enabled_while_connected_and_faded_otherwise() {
    let (mut rt, _) = recorded();
    assert!(add_button_enabled(&rt));
    let fill = element(&rt, ADD).unwrap().fill;
    rt.update(|s, _| {
        s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    });
    settle(&mut rt);
    assert!(!add_button_enabled(&rt), "reconnecting");
    let faded = element(&rt, ADD).unwrap().fill;
    assert!((faded.a - fill.a * 0.6).abs() < 0.01 && faded.r == fill.r, "the accent at 60%: {faded:?} from {fill:?}");
    click(&mut rt, ADD);
    assert!(!menu_shown(&rt), "a faded button opens nothing");
    assert!(rt.file_dialog().is_none());

    rt.update(|s, _| s.active = None);
    settle(&mut rt);
    assert!(!add_button_enabled(&rt), "no host on screen");
    assert_eq!(element(&rt, ADD).unwrap().label.as_deref(), Some("Add"));
}

#[test]
fn a_click_opens_the_add_menu_under_the_button() {
    let (mut rt, _) = recorded();
    click(&mut rt, ADD);
    assert!(menu_shown(&rt));
    assert_eq!(rt.state.files.menu.as_ref().map(|m| &m.kind), Some(&MenuKind::Add));
    assert_eq!(items(&rt), ["Upload files…", "Upload folder…", "New folder"]);
    assert_eq!(element(&rt, ADD).unwrap().expanded, Some(true));
    let ([bx, by, _, bh], [mx, my, ..]) = (element(&rt, ADD).unwrap().rect, element(&rt, "ctx-menu").unwrap().rect);
    assert!((mx - bx).abs() < 1.5 && (my - (by + bh + 2.0)).abs() < 1.5, "menu at ({mx}, {my}), button {bx},{by}+{bh}");
    // The add-menu scene opens it where a click does.
    let mut scene = scenes::base();
    scenes::add_menu(&mut scene);
    assert_eq!(rt.state.files.menu.as_ref().map(|m| m.at), scene.files.menu.map(|m| m.at));
}

#[test]
fn upload_files_opens_the_picker_as_cmd_u_does() {
    let (mut rt, rec) = recorded();
    click(&mut rt, ADD);
    click(&mut rt, "ctx-upload-files");
    assert!(!menu_shown(&rt), "picked: the menu closes");
    let (kind, options) = rt.file_dialog().expect("the picker is open");
    assert_eq!((kind, options.title.as_str()), (FileDialogKind::Open, "Upload to site"));
    rt.resolve_file_dialog_files(Some(two())).expect("the picker is open");
    settle(&mut rt);
    assert_eq!(uploads(&rec), [(two(), SITE.to_owned())]);
}

#[test]
fn upload_folder_opens_the_folder_picker_and_the_folder_uploads_into_the_folder_on_screen() {
    let (mut rt, rec) = recorded();
    click(&mut rt, ADD);
    click(&mut rt, "ctx-upload-folder");
    let (kind, options) = rt.file_dialog().expect("the folder picker is open");
    assert_eq!((kind, options.title.as_str()), (FileDialogKind::Folder, "Upload to site"));
    rt.resolve_file_dialog(Some(photos())).expect("the folder picker is open");
    settle(&mut rt);
    assert_eq!(uploads(&rec), [(vec![photos()], SITE.to_owned())]);
}

#[test]
fn new_folder_from_the_add_menu_opens_the_name_field() {
    let (mut rt, _) = recorded();
    click(&mut rt, ADD);
    click(&mut rt, "ctx-new-folder");
    assert!(element(&rt, "edit-input").is_some(), "the new folder's field");
}

#[test]
fn a_cancelled_picker_uploads_nothing() {
    let (mut rt, rec) = recorded();
    click(&mut rt, ADD);
    click(&mut rt, "ctx-upload-files");
    rt.resolve_file_dialog_files(None).expect("the picker is open");
    settle(&mut rt);
    click(&mut rt, ADD);
    click(&mut rt, "ctx-upload-folder");
    rt.resolve_file_dialog(None).expect("the folder picker is open");
    settle(&mut rt);
    assert!(uploads(&rec).is_empty());
}

#[test]
fn cmd_u_opens_the_picker() {
    let (mut rt, rec) = recorded();
    rt.key(KeyEvent { key: Key::Command('u'), mods: primary() });
    settle(&mut rt);
    assert!(!menu_shown(&rt), "straight to the picker, no menu");
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

#[test]
fn the_palettes_upload_folder_opens_the_folder_picker() {
    let (mut rt, rec) = recorded();
    rt.update(|s, tx| {
        crate::views::run(s, Action::Palette(PaletteAction::Toggle), tx);
        crate::views::run(s, Action::Palette(PaletteAction::Pick("pal-upload-folder".into())), tx);
    });
    settle(&mut rt);
    rt.resolve_file_dialog(Some(photos())).expect("the folder picker is open");
    settle(&mut rt);
    assert_eq!(uploads(&rec), [(vec![photos()], SITE.to_owned())]);
}
