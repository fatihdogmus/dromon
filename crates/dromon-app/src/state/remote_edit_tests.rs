//! Editing in a local editor: the session machine's transitions, which rows offer Edit (text
//! files, while connected), the menu, palette and ⌘E, what the window that started an edit opens,
//! conflicts and their sheet, Stop, a disconnect and quitting ending or holding sessions, Retry
//! after a failed upload, the toasts, and trace lines that name nothing. On the demo fixture
//! (which answers at once) or a [`Recorder`] to drive the service's news by hand.

use std::path::PathBuf;

use dromon_core::{ConnStatus, Error, HostId, RemotePath};
use dromon_transfer::Side;

use super::*;
use crate::fixture;
use crate::state::menu::MenuKind;
use crate::state::toast::Tone;
use crate::state::{Action, Recorder, ServiceEvent, Shortcut};

fn prod() -> HostId {
    HostId("prod".into())
}

const SITE: &str = "/var/www/site";

fn path(name: &str) -> RemotePath {
    RemotePath::new(SITE).join(name)
}

fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::SelectHost(prod()));
    s
}

/// prod's site, with a recorder in the fixture's place: nothing answers.
fn recorded() -> (App, Recorder) {
    let mut s = demo();
    let rec = Recorder::default();
    s.backend = Box::new(rec.clone());
    (s, rec)
}

fn edit(s: &mut App, name: &str) -> Vec<Effect> {
    s.act(Action::RemoteEdit(RemoteEditAction::Edit(Some(name.into()))))
}

fn sent(rec: &Recorder) -> Vec<RemoteEditCommand> {
    rec.sent()
        .into_iter()
        .filter_map(|c| match c {
            Command::RemoteEdit(cmd) => Some(cmd),
            _ => None,
        })
        .collect()
}

fn news(s: &mut App, ev: RemoteEditEvent) {
    s.apply(ServiceEvent::RemoteEdit(ev));
}

fn phase(s: &App) -> Option<&Phase> {
    s.edits.sessions.first().map(|e| &e.phase)
}

fn texts(s: &App) -> Vec<String> {
    s.toasts().into_iter().map(|t| t.text).collect()
}

fn session(phase: Phase) -> Session {
    Session { id: EditId(1), host: prod(), path: path("notes.txt"), local: None, phase }
}

fn side(size: u64, mtime: i64) -> Side {
    Side { size: Some(size), mtime: Some(mtime) }
}

/// The session's phase after `ev`, and what the step asked for.
fn after(phase: Phase, ev: Event) -> (Phase, Vec<Fx>) {
    let mut s = session(phase);
    let fx = step(&mut s, ev);
    (s.phase, fx)
}

#[test]
fn a_session_opens_saves_and_closes() {
    let local = PathBuf::from("/cache/editing/notes.txt");
    let mut s = session(Phase::Downloading);
    assert_eq!(step(&mut s, Event::Opened { local: local.clone() }), [Fx::OpenEditor(local.clone())]);
    assert_eq!((&s.phase, s.local.as_ref()), (&Phase::Open, Some(&local)));
    assert!(step(&mut s, Event::Uploading).is_empty());
    assert_eq!(s.phase, Phase::Uploading);
    let to = path("notes.txt");
    assert_eq!(step(&mut s, Event::Saved { to: to.clone() }), [Fx::Note(Note::Saved { to })]);
    assert_eq!(s.phase, Phase::Open);
    assert_eq!(step(&mut s, Event::Stop), [Fx::Send(RemoteEditCommand::Stop { id: EditId(1), host: prod() })]);
    assert_eq!(s.phase, Phase::Closed);
    // Nothing moves it out of Closed.
    assert!(step(&mut s, Event::Uploading).is_empty());
    assert_eq!(s.phase, Phase::Closed);
}

#[test]
fn a_failed_download_ends_the_session_with_a_note() {
    let (p, fx) = after(Phase::Downloading, Event::Failed { stage: EditStage::Download, error: Error::ConnectionLost });
    assert_eq!(p, Phase::Closed);
    assert_eq!(fx, [Fx::Note(Note::OpenFailed(Error::ConnectionLost))]);
    // Closed by the service: no command, no note.
    assert_eq!(after(Phase::Open, Event::Closed), (Phase::Closed, Vec::new()));
}

#[test]
fn a_conflict_waits_for_its_answer_and_each_answer_moves_on() {
    let (server, local) = (side(20, 200), side(12, 300));
    let (p, fx) = after(Phase::Uploading, Event::Conflict { server, local });
    assert_eq!((p.clone(), fx), (Phase::Conflict { server, local }, Vec::new()));
    // Uploads and retries wait for the sheet.
    assert_eq!(after(p.clone(), Event::Uploading).0, p);
    assert_eq!(after(p.clone(), Event::Retry), (p.clone(), Vec::new()));
    for choice in [EditChoice::Replace, EditChoice::KeepBoth, EditChoice::Discard] {
        let (resolving, fx) = after(p.clone(), Event::Resolve(choice));
        assert_eq!(resolving, Phase::Resolving { choice, server, local });
        assert_eq!(fx, [Fx::Send(RemoteEditCommand::Resolve { id: EditId(1), host: prod(), choice })]);
    }
    let resolving = Phase::Resolving { choice: EditChoice::Discard, server, local };
    assert_eq!(after(resolving.clone(), Event::Reloaded), (Phase::Open, vec![Fx::Note(Note::Reloaded)]));
    // A reload that fails leaves the conflict standing.
    let failed = Event::Failed { stage: EditStage::Reload, error: Error::ConnectionLost };
    assert_eq!(after(resolving, failed).0, Phase::Conflict { server, local });
    // Keep both's upload lands under another name.
    let keep = Phase::Resolving { choice: EditChoice::KeepBoth, server, local };
    let to = path("notes (1).txt");
    assert_eq!(after(keep, Event::Saved { to: to.clone() }), (Phase::Open, vec![Fx::Note(Note::Saved { to })]));
}

#[test]
fn a_failed_upload_keeps_the_change_and_retry_uploads_again() {
    let error = Error::Io("No space left".into());
    let (p, fx) = after(Phase::Uploading, Event::Failed { stage: EditStage::Upload, error: error.clone() });
    assert_eq!(p, Phase::Failed { error: error.clone() });
    assert_eq!(fx, [Fx::Note(Note::SaveFailed(error))]);
    let (p, fx) = after(p, Event::Retry);
    assert_eq!(p, Phase::Uploading);
    assert_eq!(fx, [Fx::Send(RemoteEditCommand::Retry { id: EditId(1), host: prod() })]);
    // The next save tries again by itself, and may find the server changed meanwhile.
    let failed = Phase::Failed { error: Error::ConnectionLost };
    assert_eq!(after(failed.clone(), Event::Uploading).0, Phase::Uploading);
    let server = side(1, 1);
    assert_eq!(after(failed, Event::Conflict { server, local: server }).0, Phase::Conflict { server, local: server });
}

#[test]
fn edit_is_offered_for_text_files_only_and_only_while_connected() {
    let mut s = demo();
    for name in ["index.html", "robots.txt", ".env", "server.log", "package.json", "README.md"] {
        assert!(s.editable(name), "{name} is text");
    }
    // A folder, a link to one, an image, an archive, a name the folder doesn't have.
    for name in ["assets", "current", "favicon.ico", "backup-2026-11-01.tar.gz", "gone.txt"] {
        assert!(!s.editable(name), "{name} isn't");
    }
    let item = |s: &App, row: &str| {
        let menu = s.menu(&MenuKind::Row(row.into()));
        menu.into_iter().filter_map(|e| e.item().cloned()).find(|i| i.id == "edit").unwrap()
    };
    assert!(item(&s, "index.html").enabled);
    assert_eq!(item(&s, "index.html").shortcut, Some(EDIT_CHORD));
    assert!(!item(&s, "assets").enabled, "there, but disabled, as the row menu's inapplicable items are");
    assert_eq!(s.menu_action(&MenuKind::Row("assets".into()), "edit"), None);
    assert_eq!(
        s.menu_action(&MenuKind::Row("index.html".into()), "edit"),
        Some(Action::RemoteEdit(RemoteEditAction::Edit(Some("index.html".into()))))
    );
    // Reconnecting: nothing is.
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 1 } });
    assert!(!s.editable("index.html"));
    assert!(!item(&s, "index.html").enabled);
}

#[test]
fn the_palette_offers_edit_on_a_text_cursor_row_and_cmd_e_edits_it() {
    use crate::state::palette::PaletteAction;
    let (mut s, rec) = recorded();
    let palette_edit = |s: &mut App| {
        s.act(Action::Palette(PaletteAction::Toggle));
        s.ui.palette.edit = rune::TextEdit::new("edit");
        s.act(Action::Palette(PaletteAction::Edited));
        let item = s.palette_items().into_iter().find(|i| i.key == "pal-edit");
        s.act(Action::Palette(PaletteAction::Close));
        item
    };
    s.act(Action::Click { name: "assets".into(), extend: false, toggle: false });
    assert_eq!(palette_edit(&mut s), None, "not for a folder");
    assert!(s.shortcut(Shortcut::EditFile, 0.0).is_empty());
    assert!(sent(&rec).is_empty(), "⌘E on a folder does nothing");
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    let item = palette_edit(&mut s).expect("Edit in local editor");
    assert_eq!((item.title.as_str(), item.shortcut), ("Edit in local editor", Some(EDIT_CHORD)));
    assert_eq!(item.action, Action::RemoteEdit(RemoteEditAction::Edit(Some("robots.txt".into()))));
    s.shortcut(Shortcut::EditFile, 0.0);
    let [RemoteEditCommand::Start { host, path: p, .. }] = &sent(&rec)[..] else { panic!("{:?}", sent(&rec)) };
    assert_eq!((host, p), (&prod(), &path("robots.txt")));
    // A focused field keeps ⌘E.
    let (mut s, rec) = recorded();
    s.act(Action::Click { name: "robots.txt".into(), extend: false, toggle: false });
    s.act(Action::OpenFilter);
    s.ui.filter_edit.focused = true;
    s.shortcut(Shortcut::EditFile, 0.0);
    assert!(sent(&rec).is_empty());
}

#[test]
fn the_window_that_started_an_edit_opens_its_copy_and_edit_again_reopens_it() {
    let mut s = demo();
    let fx = edit(&mut s, "index.html");
    let [Effect::OpenFile(local)] = &fx[..] else { panic!("{fx:?}") };
    assert!(local.ends_with("index.html"), "the file's own name: {local:?}");
    assert_eq!(phase(&s), Some(&Phase::Open));
    assert_eq!(s.edits.sessions[0].path, path("index.html"));
    // Edit on it again opens the same copy; no second session.
    assert_eq!(edit(&mut s, "index.html"), [Effect::OpenFile(local.clone())]);
    assert_eq!(s.edits.sessions.len(), 1);
}

#[test]
fn another_window_follows_an_edit_without_opening_it() {
    let (mut s, _) = recorded();
    let local = PathBuf::from("/cache/editing/prod-1/notes.txt");
    news(
        &mut s,
        RemoteEditEvent::Opened { id: EditId(7), host: prod(), path: path("notes.txt"), local: local.clone() },
    );
    assert_eq!(s.edits.sessions.len(), 1);
    assert_eq!((phase(&s), s.edits.sessions[0].local.as_ref()), (Some(&Phase::Open), Some(&local)));
    assert!(s.take_edit_effects().is_empty(), "only the window that started it opens it");
    // News of an edit nobody here knows is stale.
    news(&mut s, RemoteEditEvent::Uploading { id: EditId(8), host: prod() });
    assert_eq!(s.edits.sessions.len(), 1);
    // A new window (⌘N) knows the open edits, not one whose download still runs.
    edit(&mut s, "robots.txt");
    assert_eq!(s.edits.sessions.len(), 2);
    let ids: Vec<EditId> = s.edits.for_window().sessions.iter().map(|e| e.id).collect();
    assert_eq!(ids, [EditId(7)]);
}

#[test]
fn the_services_news_opens_the_copy_through_the_view_after_the_event() {
    let (mut s, rec) = recorded();
    assert!(edit(&mut s, "index.html").is_empty(), "nothing yet: the download runs");
    assert_eq!(phase(&s), Some(&Phase::Downloading));
    let [RemoteEditCommand::Start { id, .. }] = sent(&rec)[..] else { panic!() };
    let local = PathBuf::from("/cache/editing/prod-1/index.html");
    news(&mut s, RemoteEditEvent::Opened { id, host: prod(), path: path("index.html"), local: local.clone() });
    assert_eq!(s.take_edit_effects(), [Effect::OpenFile(local)]);
    assert!(s.take_edit_effects().is_empty(), "once");
}

/// prod's index.html being edited (Open), on a recorder.
fn editing() -> (App, Recorder, EditId) {
    let (mut s, rec) = recorded();
    edit(&mut s, "index.html");
    let [RemoteEditCommand::Start { id, .. }] = sent(&rec)[..] else { panic!() };
    let local = PathBuf::from("/cache/editing/prod-1/index.html");
    news(&mut s, RemoteEditEvent::Opened { id, host: prod(), path: path("index.html"), local });
    s.take_edit_effects();
    (s, rec, id)
}

#[test]
fn a_save_says_so_in_a_toast_that_goes_by_itself() {
    let (mut s, _, id) = editing();
    news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
    assert_eq!(phase(&s), Some(&Phase::Uploading));
    news(&mut s, RemoteEditEvent::Saved { id, host: prod(), to: path("index.html") });
    assert_eq!(texts(&s), ["Saved index.html to prod"]);
    // The next save's toast takes its place.
    news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
    news(&mut s, RemoteEditEvent::Saved { id, host: prod(), to: path("index.html") });
    assert_eq!(texts(&s), ["Saved index.html to prod"]);
    // It goes SAVED_FOR seconds after it showed, on the window's clock.
    let first = s.remote_edit_tick(10.0);
    assert!(!first.changed);
    assert_eq!(first.wake, Some(10.0 + SAVED_FOR));
    assert!(!s.remote_edit_tick(10.0 + SAVED_FOR - 0.1).changed);
    assert!(s.remote_edit_tick(10.0 + SAVED_FOR).changed);
    assert!(texts(&s).is_empty());
    assert_eq!(s.remote_edit_tick(20.0), Stepped::default());
}

#[test]
fn a_conflict_brings_its_sheet_and_the_answer_goes_to_the_service() {
    let (mut s, rec, id) = editing();
    let (server, local) = (side(15_990, 1_795_615_000), side(14_900, 1_795_615_100));
    news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
    news(&mut s, RemoteEditEvent::Conflict { id, host: prod(), server, local });
    assert_eq!(s.edit_conflict_on_screen().map(|e| e.id), Some(id));
    assert_eq!(s.edits.unsaved(), 1, "the change isn't on the server");
    // It's modal: the list's shortcuts wait.
    let cursor = s.conn().unwrap().selection.cursor().map(str::to_owned);
    s.shortcut(Shortcut::Move { delta: 1, extend: false }, 0.0);
    assert_eq!(s.conn().unwrap().selection.cursor().map(str::to_owned), cursor);
    // The popover hides it (its row offers to answer), and the palette does.
    s.act(Action::ToggleTransfers);
    assert!(s.edit_conflict_on_screen().is_none());
    s.act(Action::ToggleTransfers);
    assert!(s.edit_conflict_on_screen().is_some());
    s.act(Action::RemoteEdit(RemoteEditAction::Resolve(EditChoice::KeepBoth)));
    let last = sent(&rec).pop();
    assert_eq!(last, Some(RemoteEditCommand::Resolve { id, host: prod(), choice: EditChoice::KeepBoth }));
    assert!(s.edit_conflict_on_screen().is_none(), "answered");
    news(&mut s, RemoteEditEvent::Saved { id, host: prod(), to: path("index (1).html") });
    assert_eq!(phase(&s), Some(&Phase::Open));
    assert_eq!(texts(&s), ["Saved index (1).html to prod"]);
}

#[test]
fn discarding_says_so_and_the_edit_goes_on() {
    let (mut s, rec, id) = editing();
    news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
    news(&mut s, RemoteEditEvent::Conflict { id, host: prod(), server: side(9, 9), local: side(1, 1) });
    s.act(Action::RemoteEdit(RemoteEditAction::Resolve(EditChoice::Discard)));
    assert_eq!(sent(&rec).pop(), Some(RemoteEditCommand::Resolve { id, host: prod(), choice: EditChoice::Discard }));
    news(&mut s, RemoteEditEvent::Reloaded { id, host: prod() });
    assert_eq!(phase(&s), Some(&Phase::Open));
    assert_eq!(texts(&s), ["Discarded your changes to index.html"]);
}

#[test]
fn a_failed_upload_offers_retry_in_its_toast() {
    let (mut s, rec, id) = editing();
    news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
    let error = Error::Io("You don't have permission to write in /var/www/site".into());
    news(&mut s, RemoteEditEvent::Failed { id, host: prod(), stage: EditStage::Upload, error });
    let toasts = s.toasts();
    let [t] = &toasts[..] else { panic!("{toasts:?}") };
    assert_eq!(t.text, "Couldn't save index.html to prod: You don't have permission to write in /var/www/site");
    assert_eq!((t.tone, t.retry), (Tone::Error, Some(id)));
    assert_eq!(s.edits.unsaved(), 1);
    s.act(Action::RemoteEdit(RemoteEditAction::Retry(id)));
    assert_eq!(sent(&rec).pop(), Some(RemoteEditCommand::Retry { id, host: prod() }));
    assert_eq!(phase(&s), Some(&Phase::Uploading));
    assert!(s.toasts().is_empty(), "the failure has had its say");
}

#[test]
fn stop_editing_ends_the_session_and_its_toast() {
    let (mut s, rec, id) = editing();
    news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
    news(&mut s, RemoteEditEvent::Failed { id, host: prod(), stage: EditStage::Upload, error: Error::ConnectionLost });
    s.act(Action::RemoteEdit(RemoteEditAction::Stop(id)));
    assert_eq!(sent(&rec).pop(), Some(RemoteEditCommand::Stop { id, host: prod() }));
    assert!(s.edits.sessions.is_empty());
    assert!(s.toasts().is_empty());
    // The service's Closed after it changes nothing.
    news(&mut s, RemoteEditEvent::Closed { id, host: prod() });
    assert!(s.edits.sessions.is_empty());
}

#[test]
fn disconnecting_ends_the_hosts_edits_on_the_fixture() {
    let mut s = demo();
    edit(&mut s, "index.html");
    edit(&mut s, "robots.txt");
    assert_eq!(s.edits.sessions.len(), 2);
    s.act(Action::Disconnect(prod()));
    assert!(s.edits.sessions.is_empty(), "{:?}", s.edits.sessions);
}

#[test]
fn quitting_asks_while_a_change_is_not_on_the_server() {
    let (mut s, _, id) = editing();
    assert!(s.close_requested(), "an edit with nothing to upload doesn't hold quitting");
    news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
    assert!(!s.close_requested(), "held for the question");
    assert!(s.ui.confirm_quit);
    s.act(Action::CancelQuit);
    news(&mut s, RemoteEditEvent::Saved { id, host: prod(), to: path("index.html") });
    assert!(s.close_requested());
    // A failed upload and a conflict hold it too; a Discard under way doesn't.
    news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
    let (server, local) = (side(1, 1), side(2, 2));
    news(&mut s, RemoteEditEvent::Conflict { id, host: prod(), server, local });
    assert_eq!(s.edits.unsaved(), 1);
    s.act(Action::RemoteEdit(RemoteEditAction::Resolve(EditChoice::Discard)));
    assert_eq!(s.edits.unsaved(), 0);
    assert!(s.close_requested());
}

#[test]
fn the_quit_count_adds_transfers_and_edits() {
    let (mut s, rec) = recorded();
    for (n, name) in ["index.html", "robots.txt", "README.md"].into_iter().enumerate() {
        edit(&mut s, name);
        let Some(RemoteEditCommand::Start { id, .. }) = sent(&rec).pop() else { panic!() };
        let local = PathBuf::from(format!("/cache/{n}/{name}"));
        news(&mut s, RemoteEditEvent::Opened { id, host: prod(), path: path(name), local });
        if n < 2 {
            news(&mut s, RemoteEditEvent::Uploading { id, host: prod() });
        }
    }
    assert_eq!(s.edits.unsaved(), 2);
    assert!(!s.close_requested());
}

#[test]
fn trace_lines_name_the_kind_host_and_id_and_never_a_path_or_error() {
    let id = EditId(4);
    let secret = RemotePath::new("/secret/notes.txt");
    let events = [
        RemoteEditEvent::Opened { id, host: prod(), path: secret.clone(), local: PathBuf::from("/home/me/secret") },
        RemoteEditEvent::Uploading { id, host: prod() },
        RemoteEditEvent::Saved { id, host: prod(), to: secret.clone() },
        RemoteEditEvent::Conflict { id, host: prod(), server: side(1, 2), local: side(3, 4) },
        RemoteEditEvent::Reloaded { id, host: prod() },
        RemoteEditEvent::Failed { id, host: prod(), stage: EditStage::Upload, error: Error::NotFound(secret) },
        RemoteEditEvent::Closed { id, host: prod() },
    ];
    let lines: Vec<String> =
        events.into_iter().map(|e| crate::state::service::trace_line(&ServiceEvent::RemoteEdit(e))).collect();
    assert_eq!(
        lines,
        [
            "RemoteEditOpened host=prod id=4",
            "RemoteEditUploading host=prod id=4",
            "RemoteEditSaved host=prod id=4",
            "RemoteEditConflict host=prod id=4",
            "RemoteEditReloaded host=prod id=4",
            "RemoteEditFailed host=prod id=4 stage=Upload",
            "RemoteEditClosed host=prod id=4",
        ]
    );
    assert!(lines.iter().all(|l| !l.contains("secret")));
}
