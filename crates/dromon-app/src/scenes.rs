//! Named states for headless snapshots (`--scene NAME`), each reached by the same actions and
//! events a user and the service would produce. They start from the demo fixture (on its clock),
//! or from `--fixture`'s when one is given; the goldens are the demo's.

use std::time::Duration;

use dromon_core::{ConnStatus, HostId, Prompt, PromptId, PromptKind, RemotePath};
use dromon_ssh::QuickField;
use dromon_transfer::{ConflictView, JobId, JobKind, JobView, Progress, Reveal, Side, TaskState};
use rune::TextEdit;

use crate::fixture;
use crate::state::log::LogBuffer;
use crate::state::{
    Action, App, DropTarget, Effect, FileAction, FileOpEvent, HostAction, Live, MenuKind, PaletteAction, RowTarget,
    SearchAction, ServiceEvent, Silent, Tab,
};
use crate::theme::Appearance;

#[path = "scenes_preview.rs"]
mod preview;
#[path = "scenes_remote_edit.rs"]
mod remote_edit;
#[path = "scenes_report.rs"]
mod report;
#[path = "scenes_settings.rs"]
mod settings;
#[path = "scenes_update.rs"]
mod update;

/// The demo's big folder: 10,000 photos.
pub const BIG_FOLDER: &str = "/srv/media/photos";

/// The app as it launches on the active fixture: hosts listed, nothing connected.
pub fn base() -> App {
    fixture::app(fixture::active())
}

fn prod() -> HostId {
    HostId("prod".into())
}

fn act(s: &mut App, a: Action) -> Vec<Effect> {
    s.act(a)
}

pub fn welcome(s: &mut App) {
    *s = base();
}

pub fn connecting(s: &mut App) {
    *s = base();
    s.backend = Box::new(Silent::default());
    act(s, Action::SelectHost(prod()));
}

pub fn listing(s: &mut App) {
    *s = base();
    // A second live connection, so Connected holds two.
    act(s, Action::SelectHost(HostId("nas".into())));
    act(s, Action::SelectHost(prod()));
    act(s, Action::Click { name: "index.html".into(), extend: false, toggle: false });
}

/// The listing watched live: "Live" before the connection in the status bar.
pub fn live(s: &mut App) {
    listing(s);
    let live = Live::On(RemotePath::new("/var/www/site"));
    s.apply(ServiceEvent::Live { host: prod(), live });
}

pub fn listing_sorted_size(s: &mut App) {
    listing(s);
    act(s, Action::Sort(dromon_core::SortKey::Size));
}

pub fn big_folder(s: &mut App) {
    *s = base();
    act(s, Action::SelectHost(prod()));
    act(s, Action::GoTo(RemotePath::new(BIG_FOLDER)));
    let name = "IMG_05000.jpg".to_owned();
    act(s, Action::Click { name: name.clone(), extend: false, toggle: false });
    s.ui.center_on_mount = Some(name);
}

pub fn empty_folder(s: &mut App) {
    *s = base();
    act(s, Action::SelectHost(prod()));
    act(s, Action::Open("uploads".into()));
}

pub fn permission_denied(s: &mut App) {
    *s = base();
    act(s, Action::SelectHost(prod()));
    act(s, Action::GoTo(RemotePath::new("/root")));
}

pub fn reconnecting(s: &mut App) {
    listing(s);
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::Reconnecting { attempt: 2 } });
}

pub fn filter(s: &mut App) {
    listing(s);
    act(s, Action::OpenFilter);
    s.ui.filter_edit = TextEdit::new("se");
    s.ui.filter_edit.focused = true;
    act(s, Action::FilterChanged("se".into()));
}

pub fn light(s: &mut App) {
    listing(s);
    s.ui.appearance = Appearance::Light;
}

/// ⌘⇧F and `*.js*` typed on prod's site: the results with their folders (the folder searched,
/// a release below it), one selected, and the line saying what was found where.
pub fn search_results(s: &mut App) {
    listing(s);
    act(s, Action::Search(SearchAction::Open));
    let query = "*.js*";
    s.ui.filter_edit = TextEdit::new(query);
    s.ui.filter_edit.focused = true;
    act(s, Action::Search(SearchAction::Typed { text: query.into(), now: 0.0 }));
    act(s, Action::Search(SearchAction::Tick(crate::state::search::DEBOUNCE)));
    if let Some(i) = s.ui.search.found.iter().position(|f| f.folder != "." && f.entry.name == "package.json") {
        act(s, Action::Click { name: crate::state::search::hit_key(i), extend: false, toggle: false });
    }
}

/// [`search_results`] in the light theme.
pub fn search_results_light(s: &mut App) {
    lit(s, search_results);
}

/// Connecting to prod, and ssh asks `kind` with `text`.
fn asked(s: &mut App, kind: PromptKind, text: &str, error: Option<&str>) {
    connecting(s);
    s.apply(ServiceEvent::Status { host: prod(), status: ConnStatus::NeedsInput });
    let prompt = Prompt { id: PromptId(1), host: prod(), kind, text: text.into(), error: error.map(str::to_owned) };
    s.apply(ServiceEvent::Prompt(prompt));
}

const PASSWORD: &str = "deploy@prod.example.com's password: ";
const FINGERPRINT: &str = "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU";

pub fn auth_password(s: &mut App) {
    asked(s, PromptKind::Password, PASSWORD, None);
}

pub fn auth_password_error(s: &mut App) {
    asked(s, PromptKind::Password, PASSWORD, Some(dromon_ssh::WRONG_PASSWORD));
}

pub fn host_key(s: &mut App) {
    let text = format!(
        "The authenticity of host 'prod.example.com (203.0.113.10)' can't be established.\n\
         ED25519 key fingerprint is {FINGERPRINT}.\n\
         This key is not known by any other names.\n\
         Are you sure you want to continue connecting (yes/no/[fingerprint])? "
    );
    let kind = PromptKind::HostKey { key_type: Some("ED25519".into()), fingerprint: Some(FINGERPRINT.into()) };
    asked(s, kind, &text, None);
}

pub fn confirm(s: &mut App) {
    let text = format!("Allow use of key /home/fatih/.ssh/id_ed25519?\nKey fingerprint {FINGERPRINT}.");
    asked(s, PromptKind::Confirm, &text, None);
}

pub fn connection_log(s: &mut App) {
    listing(s);
    let ssh = "ssh -T -x -e none -o BatchMode=no -o ServerAliveInterval=15 -o ServerAliveCountMax=3 \
               -o ConnectTimeout=15 -o ControlMaster=no -s prod sftp";
    let lines: [(i64, String); 14] = [
        (0, format!("Running {ssh}")),
        (412, "Warning: Permanently added 'prod.example.com' (ED25519) to the list of known hosts.".into()),
        (436, "ssh asks “deploy@prod.example.com's password:”".into()),
        (5_120, "Answered ssh's prompt.".into()),
        (5_388, "SFTP session started.".into()),
        (5_388, "Connected.".into()),
        (1_843_002, "Connection lost: Connection to prod.example.com closed by remote host.".into()),
        (1_843_002, "Reconnecting in 1 s.".into()),
        (1_844_004, "Reconnecting (attempt 1)…".into()),
        (1_844_004, format!("Running {ssh}")),
        (1_844_391, "ssh asked “deploy@prod.example.com's password:”: answered from memory.".into()),
        (1_844_655, "SFTP session started.".into()),
        (1_844_655, "Reconnected.".into()),
        (1_851_230, "debug1: client_input_global_request: rtype keepalive@openssh.com want_reply 1".into()),
    ];
    let start = (s.now - 3 * 3600) * 1000;
    if let Some(c) = s.conns.get_mut(&prod()) {
        // Only this script of a session three hours old, not what connecting just logged.
        c.log = LogBuffer::default();
        for (at, line) in lines {
            c.log.push(start + at, line);
        }
    }
    act(s, Action::OpenLog);
}

/// The log after a wake found the connection dead, then verbose logging turned on: the
/// reconnect with `-v` and ssh's `debug1:` lines.
pub fn connection_log_verbose(s: &mut App) {
    listing(s);
    let ssh = "ssh -T -x -e none -o BatchMode=no -o ServerAliveInterval=15 -o ServerAliveCountMax=3 \
               -o ConnectTimeout=15 -o ControlMaster=no -v -s prod sftp";
    let lines: [(i64, String); 16] = [
        (0, "The computer was asleep: checking the connection.".into()),
        (5_002, "The connection didn't answer a check within 5 s.".into()),
        (5_002, "Connection lost: the SFTP channel stopped answering.".into()),
        (5_002, "Reconnecting in 1 s.".into()),
        (6_004, "Reconnecting (attempt 1)…".into()),
        (6_731, "Reconnected.".into()),
        (61_200, "Verbose logging on: ssh runs with -v.".into()),
        (61_200, "Reconnecting (attempt 1)…".into()),
        (61_201, format!("Running {ssh}")),
        (61_214, "debug1: OpenSSH_10.0p2, OpenSSL 3.5.1 1 Jul 2025".into()),
        (61_215, "debug1: Reading configuration data /home/fatih/.ssh/config".into()),
        (61_240, "debug1: Connecting to prod.example.com [203.0.113.10] port 22.".into()),
        (61_301, "debug1: Connection established.".into()),
        (61_655, "debug1: Authenticated to prod.example.com ([203.0.113.10]:22) using \"publickey\".".into()),
        (61_702, "SFTP session started.".into()),
        (61_702, "Reconnected.".into()),
    ];
    let start = (s.now - 600) * 1000;
    if let Some(c) = s.conns.get_mut(&prod()) {
        c.log = LogBuffer::default();
        for (at, line) in lines {
            c.log.push(start + at, line);
        }
        c.log.verbose = true;
    }
    act(s, Action::OpenLog);
}

/// A transfer job as the engine would report it, at `done` of `total` bytes.
fn job(id: u64, kind: JobKind, host: &str, title: &str, state: TaskState, done: u64, total: u64) -> JobView {
    JobView {
        id: JobId(id),
        kind,
        host: HostId(host.into()),
        title: title.into(),
        state,
        progress: Progress { done, total },
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

/// The session's transfers: every state a download can be in, and three uploads (one waits on a
/// name clash in /var/www/site when `clash`).
fn transfer_jobs(clash: bool) -> Vec<JobView> {
    use JobKind::{Download, Upload};
    let mut releases = job(9, Download, "prod", "releases", TaskState::Running, 12_400_000, 80_000_000);
    (releases.speed, releases.eta, releases.files_done, releases.files_total) =
        (Some(8_100_000.0), Some(Duration::from_secs(9)), 14, 128);
    let mut hero = job(8, Upload, "prod", "hero.png", TaskState::Running, 3_100_000, 4_600_000);
    (hero.speed, hero.eta) = (Some(640_000.0), Some(Duration::from_secs(3)));
    let mut site = job(7, Upload, "prod", "2 items", TaskState::Preparing { files_found: 1 }, 0, 0);
    if clash {
        let (existing, incoming) = (Side::default(), Side::default());
        site.conflict = Some(ConflictView {
            name: "index.html".into(),
            folder: RemotePath::new("/var/www/site"),
            existing_is_dir: false,
            existing,
            incoming,
            more: 0,
        });
    }
    let node = job(6, Download, "prod", "node_modules", TaskState::Preparing { files_found: 1204 }, 0, 0);
    let backup = job(5, Download, "prod", "backup-2026-11-01.tar.gz", TaskState::Paused, 7_300_000, 18_204_116);
    let mut photos = job(4, Download, "nas", "photos", TaskState::Running, 410_000_000, 1_020_000_000);
    photos.waiting_for_connection = true;
    let mut syslog =
        job(3, Download, "prod", "syslog", TaskState::Failed { error: "1 file failed".into() }, 0, 2_210_000);
    syslog.errors = vec!["syslog: You don't have access to /var/log/syslog".into()];
    let mut robots = job(2, Upload, "prod", "robots.txt", TaskState::Done, 68, 68);
    robots.reveal = Some(Reveal::Remote(HostId("prod".into()), RemotePath::new("/var/www/site/robots.txt")));
    let mut index = job(1, Download, "prod", "index.html", TaskState::Done, 14_812, 14_812);
    index.reveal = Some(Reveal::Local("/Users/me/Downloads/index.html".into()));
    vec![releases, hero, site, node, backup, photos, syslog, robots, index]
}

/// The transfers popover open on Downloads: running with speed and time left, preparing,
/// paused, waiting for the connection, failed and done.
pub fn transfers(s: &mut App) {
    listing(s);
    s.apply(ServiceEvent::Transfers(transfer_jobs(false)));
    act(s, Action::ToggleTransfers);
    act(s, Action::TransfersTab(Tab::Downloads));
}

/// The Uploads tab: running, waiting on a name clash (its sheet waits until the popover
/// closes), and done.
pub fn transfers_uploads(s: &mut App) {
    listing(s);
    s.apply(ServiceEvent::Transfers(transfer_jobs(true)));
    act(s, Action::ToggleTransfers);
    act(s, Action::TransfersTab(Tab::Uploads));
}

/// An upload of four items clashes with index.html, and three more after it.
pub fn transfer_conflict(s: &mut App) {
    listing(s);
    let mut j = job(1, JobKind::Upload, "prod", "4 items", TaskState::Preparing { files_found: 0 }, 0, 0);
    j.conflict = Some(ConflictView {
        name: "index.html".into(),
        folder: RemotePath::new("/var/www/site"),
        existing_is_dir: false,
        // The server's copy is from Nov 20 (09:12); the upload was saved today at 13:48.
        existing: Side { size: Some(14_812), mtime: Some(1_795_165_920) },
        incoming: Side { size: Some(15_240), mtime: Some(1_795_614_480) },
        more: 2,
    });
    s.apply(ServiceEvent::Transfers(vec![j]));
}

/// Three files dragged from Finder over the list.
pub fn drop_list(s: &mut App) {
    listing(s);
    act(s, Action::DropHover { target: DropTarget::List, over: true, count: 3 });
}

/// Two files dragged over the uploads folder's row.
pub fn drop_folder(s: &mut App) {
    listing(s);
    act(s, Action::DropHover { target: DropTarget::Folder("uploads".into()), over: true, count: 2 });
}

/// Two jobs running, 40% of their bytes done, the popover closed: the ring and its badge.
pub fn toolbar_ring(s: &mut App) {
    listing(s);
    let mut site = job(3, JobKind::Download, "prod", "site", TaskState::Running, 24_000_000, 50_000_000);
    site.speed = Some(4_000_000.0);
    let logs = job(2, JobKind::Upload, "prod", "logs", TaskState::Preparing { files_found: 12 }, 0, 10_000_000);
    let done = job(1, JobKind::Download, "prod", "index.html", TaskState::Done, 14_812, 14_812);
    s.apply(ServiceEvent::Transfers(vec![site, logs, done]));
}

/// The window's close button with two transfers running: "Quit anyway?".
pub fn quit_confirm(s: &mut App) {
    toolbar_ring(s);
    s.close_requested();
}

fn file(s: &mut App, a: FileAction) {
    act(s, Action::File(a));
}

fn click(s: &mut App, name: &str, toggle: bool) {
    act(s, Action::Click { name: name.into(), extend: false, toggle });
}

/// ⌘⇧N: the new folder's field at the top of the list, its free name selected.
pub fn new_folder(s: &mut App) {
    listing(s);
    file(s, FileAction::NewFolder);
}

/// F2 on package-lock.json, renamed to a name that's taken: the field stays, with the error.
pub fn rename_error(s: &mut App) {
    listing(s);
    click(s, "package-lock.json", false);
    file(s, FileAction::Rename);
    s.files.field_edit().replace_text("package.json");
    s.files.field_edit().mark_edited();
    file(s, FileAction::EditSubmit);
}

/// Every row selected, ⌘⌫: "Delete 17 items permanently?", five names, "and 12 more".
pub fn delete_confirm(s: &mut App) {
    listing(s);
    act(s, Action::SelectAll);
    file(s, FileAction::Delete);
}

/// Deletes `names` on prod (the service doesn't answer: the scene sets their progress).
fn deleting(s: &mut App, names: &[&str]) -> crate::state::OpId {
    for (i, n) in names.iter().enumerate() {
        click(s, n, i > 0);
    }
    file(s, FileAction::Delete);
    file(s, FileAction::ConfirmDelete);
    s.ops.ops.last().map(|o| o.id).expect("the delete")
}

/// Two deletes running: one still counting what's there, one removing it.
pub fn delete_progress(s: &mut App) {
    listing(s);
    s.backend = Box::new(Silent::default());
    deleting(s, &["releases", "node_modules", "backup-2026-11-01.tar.gz"]);
    let id = deleting(s, &["assets"]);
    s.apply(ServiceEvent::FileOp(FileOpEvent::Progress { id, host: prod(), done: 1204, total: Some(5005) }));
}

/// A delete that stopped: its toast stays with the reason and a dismiss button.
pub fn delete_failed(s: &mut App) {
    listing(s);
    s.backend = Box::new(Silent::default());
    let id = deleting(s, &["uploads"]);
    let error = dromon_core::Error::PermissionDenied(RemotePath::new("/var/www/site/uploads/2026"));
    s.apply(ServiceEvent::FileOp(FileOpEvent::Failed { id, host: prod(), error }));
}

/// index.html and robots.txt cut (⌘X): dimmed until they're pasted.
pub fn cut_rows(s: &mut App) {
    listing(s);
    click(s, "robots.txt", true);
    file(s, FileAction::Cut);
}

/// index.html dragged over the uploads row: the row lifts, "Move 1 item to uploads".
pub fn drag_folder(s: &mut App) {
    listing(s);
    file(s, FileAction::DragStart("index.html".into()));
    file(s, FileAction::DragOver { target: Some(RowTarget::Folder("uploads".into())), copy: false });
}

/// The drawn menu of a right-clicked row (Linux; macOS and Windows show a native one).
pub fn menu_row(s: &mut App) {
    listing(s);
    file(s, FileAction::Copy);
    file(s, FileAction::OpenMenu { kind: MenuKind::Row("index.html".into()), at: Some((560.0, 330.0)) });
}

/// The drawn menu of the list's empty area.
pub fn menu_empty(s: &mut App) {
    listing(s);
    file(s, FileAction::OpenMenu { kind: MenuKind::Empty, at: Some((640.0, 560.0)) });
}

/// The toolbar's add button pressed: its drawn menu under it (Upload files…, Upload folder…,
/// New folder), at the button's bottom-left corner as the view opens it.
pub fn add_menu(s: &mut App) {
    listing(s);
    file(s, FileAction::OpenMenu { kind: MenuKind::Add, at: Some((316.0, 36.0)) });
}

/// The light theme: a cut row (dimmed) with its drawn menu open.
pub fn file_ops_light(s: &mut App) {
    menu_row(s);
    file(s, FileAction::Cut);
    s.ui.appearance = Appearance::Light;
}

fn lit(s: &mut App, scene: fn(&mut App)) {
    scene(s);
    s.ui.appearance = Appearance::Light;
}

/// The sign-in sheet in the light theme.
pub fn sheet_light(s: &mut App) {
    lit(s, auth_password);
}

/// The conflict sheet in the light theme.
pub fn conflict_light(s: &mut App) {
    lit(s, transfer_conflict);
}

/// The delete sheet in the light theme.
pub fn delete_light(s: &mut App) {
    lit(s, delete_confirm);
}

/// The transfers popover in the light theme.
pub fn transfers_light(s: &mut App) {
    lit(s, transfers);
}

/// A failed delete's toast in the light theme.
pub fn toast_light(s: &mut App) {
    lit(s, delete_failed);
}

/// Increased contrast (§6.9): stronger borders, solid fields; the transfers popover open over
/// the listing with the filter field.
pub fn contrast(s: &mut App) {
    transfers(s);
    s.ui.access.increase_contrast = true;
}

/// [`contrast`] in the light theme.
pub fn contrast_light(s: &mut App) {
    lit(s, contrast);
}

/// Reduce transparency (§6.5): the reconnecting banner and the fields are opaque.
pub fn solid(s: &mut App) {
    reconnecting(s);
    s.ui.access.reduce_transparency = true;
}

/// Every file icon in the list, one row per category (folder, plain, code, text, log, image,
/// audio, video, PDF, archive, binary), to look at the set as the views draw it.
pub fn icons(s: &mut App) {
    listing(s);
    let Some(c) = s.conn() else { return };
    let (cwd, host) = (c.cwd.clone(), prod());
    let entry = |name: &str, kind: dromon_core::EntryKind, size: u64| dromon_core::Entry {
        name: name.into(),
        kind,
        size,
        mtime: Some(1_795_000_000),
        mode: Some(0o644),
        owner: None,
        group: None,
    };
    use dromon_core::EntryKind::{Dir, File};
    let entries = vec![
        entry("folder", Dir, 0),
        entry("plain", File, 1200),
        entry("code.rs", File, 3400),
        entry("notes.txt", File, 900),
        entry("server.log", File, 48000),
        entry("photo.png", File, 250_000),
        entry("song.mp3", File, 4_100_000),
        entry("clip.mp4", File, 52_000_000),
        entry("paper.pdf", File, 810_000),
        entry("backup.zip", File, 9_000_000),
        entry("tool.bin", File, 120_000),
    ];
    s.apply(ServiceEvent::Listing { host, path: cwd, entries });
}

/// The site's entries now, changed by `f`, as a poll would bring them.
fn polled(s: &mut App, f: impl FnOnce(&mut Vec<dromon_core::Entry>)) {
    let Some(c) = s.conn() else { return };
    let (path, mut entries) = (c.cwd.clone(), c.entries().to_vec());
    f(&mut entries);
    s.apply(ServiceEvent::Refreshed { host: prod(), path, entries });
}

/// A poll just brought two files in (mint, at full strength before they fade), took
/// favicon.ico away and grew server.log in place; index.html stays selected.
pub fn poll_added(s: &mut App) {
    listing(s);
    let at = s.now - 20;
    polled(s, |entries| {
        let file = |name: &str, size| dromon_core::Entry {
            name: name.into(),
            kind: dromon_core::EntryKind::File,
            size,
            mtime: Some(at),
            mode: Some(0o100_644),
            owner: None,
            group: None,
        };
        entries.retain(|e| e.name != "favicon.ico");
        entries.extend([file("deploy.log", 2_412), file("notes.md", 860)]);
        if let Some(log) = entries.iter_mut().find(|e| e.name == "server.log") {
            log.size += 48_000;
            log.mtime = Some(at);
        }
    });
    // The picture is of the wash as it comes in, before it fades.
    if let Some(c) = s.conn_mut() {
        c.fresh.hold = true;
    }
}

/// releases was on screen when it was deleted elsewhere: the view is in its parent, which no
/// longer lists it, and the toast says what went (§6.8).
pub fn folder_vanished(s: &mut App) {
    listing(s);
    act(s, Action::Open("releases".into()));
    s.backend = Box::new(Silent::default());
    let gone = RemotePath::new("/var/www/site/releases");
    s.apply(ServiceEvent::ListFailed { host: prod(), path: gone.clone(), error: dromon_core::Error::NotFound(gone) });
    let Some(c) = s.conn() else { return };
    let (path, mut entries) =
        (c.cwd.clone(), fixture::active().server("prod").and_then(|srv| srv.list(&c.cwd).ok()).unwrap_or_default());
    entries.retain(|e| e.name != "releases");
    s.apply(ServiceEvent::Listing { host: prod(), path, entries });
}

/// The quick-connect popover, empty, from the sidebar's +.
pub fn quick_connect(s: &mut App) {
    *s = base();
    act(s, Action::QuickConnect);
}

/// Every field filled, More open.
pub fn quick_connect_more(s: &mut App) {
    quick_connect(s);
    act(s, Action::HostBook(HostAction::ToggleMore));
    for (f, text) in [
        (QuickField::Host, "10.0.0.7"),
        (QuickField::User, "deploy"),
        (QuickField::Port, "2222"),
        (QuickField::Alias, "lab"),
        (QuickField::IdentityFile, "~/.ssh/id_lab"),
        (QuickField::ProxyJump, "jump@bastion.example.com"),
    ] {
        s.ui.quick.set_text(f, text);
    }
}

/// Connect with a host that has a space and a port out of range: both say so.
pub fn quick_connect_error(s: &mut App) {
    quick_connect(s);
    s.ui.quick.set_text(QuickField::Host, "my server");
    s.ui.quick.set_text(QuickField::Port, "70000");
    act(s, Action::HostBook(HostAction::Connect));
}

/// The saved host `box` on its way into the ssh config: its block, the file and the backup.
pub fn add_to_config(s: &mut App) {
    *s = base();
    act(s, Action::HostBook(HostAction::AddToConfig(HostId("box".into()))));
}

/// The drawn menu of a saved host in the sidebar.
pub fn host_menu(s: &mut App) {
    *s = base();
    file(s, FileAction::OpenMenu { kind: MenuKind::Host(HostId("box".into())), at: Some((96.0, 258.0)) });
}

/// ⌘K on prod's site: Hosts (prod and nas Connected first, each with its ⌘ number), then the
/// actions that work now.
pub fn palette(s: &mut App) {
    listing(s);
    act(s, Action::Palette(PaletteAction::Toggle));
}

/// ⌘K, "st" typed: staging among the hosts, Show transfers and others among the actions, the
/// matched letters bold.
pub fn palette_filtered(s: &mut App) {
    palette(s);
    s.ui.palette.edit = TextEdit::new("st");
    s.ui.palette.edit.focused = true;
    act(s, Action::Palette(PaletteAction::Edited));
}

/// ⌘L after a look into releases and uploads: the folder on screen's path, all selected, and
/// the recent folders under it.
pub fn palette_path(s: &mut App) {
    listing(s);
    act(s, Action::Open("releases".into()));
    act(s, Action::GoUp);
    act(s, Action::Open("uploads".into()));
    act(s, Action::GoUp);
    act(s, Action::Palette(PaletteAction::OpenPath));
}

/// prod's site as its server's `ls -l` would name the owners (the demo's entries have none), with
/// `name` selected and its Properties sheet open.
fn properties_of(s: &mut App, name: &str) {
    listing(s);
    // As if the first listing had brought them (a later one with the same rows changes nothing
    // on screen: owners aren't part of what a refresh compares).
    if let Some(c) = s.conn_mut()
        && let crate::state::Listing::Ready(entries) = &mut c.listing
    {
        for e in entries {
            (e.owner, e.group) = (Some("deploy".into()), Some("www-data".into()));
        }
    }
    click(s, name, false);
    file(s, FileAction::Properties);
}

/// ⌥↩ (⌘I) on index.html: its kind, folder, exact size, date and time, mode and owner.
pub fn properties_file(s: &mut App) {
    properties_of(s, "index.html");
}

/// The releases folder's Properties after Calculate: its total, what it holds, Recalculate.
pub fn properties_folder(s: &mut App) {
    properties_of(s, "releases");
    file(s, FileAction::Calculate);
}

/// [`properties_file`] in the light theme.
pub fn properties_file_light(s: &mut App) {
    lit(s, properties_file);
}

/// [`properties_folder`] in the light theme.
pub fn properties_folder_light(s: &mut App) {
    lit(s, properties_folder);
}

/// A named state: what `AppSpec::scenes` takes.
pub type Scene = (&'static str, fn(&mut App));

/// Every scene, for `AppSpec::scenes`; names are fixed (goldens are named after them).
pub const SCENES: &[Scene] = &[
    ("welcome", welcome),
    ("connecting", connecting),
    ("listing", listing),
    ("live", live),
    ("listing-sorted-size", listing_sorted_size),
    ("big-folder", big_folder),
    ("empty-folder", empty_folder),
    ("permission-denied", permission_denied),
    ("reconnecting", reconnecting),
    ("filter", filter),
    ("search-results", search_results),
    ("search-results-light", search_results_light),
    ("light", light),
    ("auth-password", auth_password),
    ("auth-password-error", auth_password_error),
    ("host-key", host_key),
    ("confirm", confirm),
    ("connection-log", connection_log),
    ("connection-log-verbose", connection_log_verbose),
    ("transfers", transfers),
    ("transfers-uploads", transfers_uploads),
    ("transfer-conflict", transfer_conflict),
    ("editing", remote_edit::editing),
    ("edit-conflict", remote_edit::edit_conflict),
    ("edit-conflict-light", remote_edit::edit_conflict_light),
    ("drop-list", drop_list),
    ("drop-folder", drop_folder),
    ("toolbar-ring", toolbar_ring),
    ("quit-confirm", quit_confirm),
    ("new-folder", new_folder),
    ("rename-error", rename_error),
    ("delete-confirm", delete_confirm),
    ("delete-progress", delete_progress),
    ("delete-failed", delete_failed),
    ("cut-rows", cut_rows),
    ("drag-folder", drag_folder),
    ("menu-row", menu_row),
    ("menu-empty", menu_empty),
    ("add-menu", add_menu),
    ("file-ops-light", file_ops_light),
    ("icons", icons),
    ("poll-added", poll_added),
    ("folder-vanished", folder_vanished),
    ("quick-connect", quick_connect),
    ("quick-connect-more", quick_connect_more),
    ("quick-connect-error", quick_connect_error),
    ("add-to-config", add_to_config),
    ("host-menu", host_menu),
    ("sheet-light", sheet_light),
    ("conflict-light", conflict_light),
    ("delete-light", delete_light),
    ("transfers-light", transfers_light),
    ("toast-light", toast_light),
    ("contrast", contrast),
    ("contrast-light", contrast_light),
    ("solid", solid),
    ("palette", palette),
    ("palette-filtered", palette_filtered),
    ("palette-path", palette_path),
    ("properties-file", properties_file),
    ("properties-folder", properties_folder),
    ("properties-file-light", properties_file_light),
    ("properties-folder-light", properties_folder_light),
    ("preview-image", preview::preview_image),
    ("preview-text", preview::preview_text),
    ("preview-unsupported", preview::preview_unsupported),
    ("preview-text-light", preview::preview_text_light),
    ("listing-compact", settings::listing_compact),
    ("settings", settings::settings),
    ("settings-light", settings::settings_light),
    ("settings-transfers", settings::settings_transfers),
    ("settings-general", update::settings_general),
    ("update-toast", update::update_toast),
    ("update-confirm", update::update_confirm),
    ("report", report::report),
    ("report-light", report::report_light),
    ("recording", report::recording),
    ("report-recorded", report::report_recorded),
];
