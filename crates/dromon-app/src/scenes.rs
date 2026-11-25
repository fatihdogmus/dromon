//! Named states for headless snapshots (`--scene NAME`), each reached by the same actions and
//! events a user and the service would produce. They start from the demo fixture (on its clock),
//! or from `--fixture`'s when one is given; the goldens are the demo's.

use std::time::Duration;

use dromon_core::{ConnStatus, HostId, Prompt, PromptId, PromptKind, RemotePath};
use dromon_transfer::{ConflictView, JobId, JobKind, JobView, Progress, Reveal, TaskState};
use rune::TextEdit;

use crate::fixture;
use crate::state::log::LogBuffer;
use crate::state::{Action, App, DropTarget, Effect, ServiceEvent, Silent, Tab};
use crate::theme::Appearance;

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
    }
}

/// The session's transfers: every state a download can be in, and three uploads (one waits on a
/// name clash in /var/www/site when `clash`).
fn transfer_jobs(clash: bool) -> Vec<JobView> {
    use JobKind::{Download, Upload};
    let mut releases = job(9, Download, "prod", "releases", TaskState::Running, 12_400_000, 80_000_000);
    (releases.speed, releases.eta, releases.files_done, releases.files_total) = (Some(8_100_000.0), Some(Duration::from_secs(9)), 14, 128);
    let mut hero = job(8, Upload, "prod", "hero.png", TaskState::Running, 3_100_000, 4_600_000);
    (hero.speed, hero.eta) = (Some(640_000.0), Some(Duration::from_secs(3)));
    let mut site = job(7, Upload, "prod", "2 items", TaskState::Preparing { files_found: 1 }, 0, 0);
    if clash {
        site.conflict = Some(ConflictView { name: "index.html".into(), folder: RemotePath::new("/var/www/site"), existing_is_dir: false, more: 0 });
    }
    let node = job(6, Download, "prod", "node_modules", TaskState::Preparing { files_found: 1204 }, 0, 0);
    let backup = job(5, Download, "prod", "backup-2026-11-01.tar.gz", TaskState::Paused, 7_300_000, 18_204_116);
    let mut photos = job(4, Download, "nas", "photos", TaskState::Running, 410_000_000, 1_020_000_000);
    photos.waiting_for_connection = true;
    let mut syslog = job(3, Download, "prod", "syslog", TaskState::Failed { error: "1 file failed".into() }, 0, 2_210_000);
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
    j.conflict = Some(ConflictView { name: "index.html".into(), folder: RemotePath::new("/var/www/site"), existing_is_dir: false, more: 2 });
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

/// A named state: what `AppSpec::scenes` takes.
pub type Scene = (&'static str, fn(&mut App));

/// Every scene, for `AppSpec::scenes`; names are fixed (goldens are named after them).
pub const SCENES: &[Scene] = &[
    ("welcome", welcome),
    ("connecting", connecting),
    ("listing", listing),
    ("listing-sorted-size", listing_sorted_size),
    ("big-folder", big_folder),
    ("empty-folder", empty_folder),
    ("permission-denied", permission_denied),
    ("reconnecting", reconnecting),
    ("filter", filter),
    ("light", light),
    ("auth-password", auth_password),
    ("auth-password-error", auth_password_error),
    ("host-key", host_key),
    ("confirm", confirm),
    ("connection-log", connection_log),
    ("transfers", transfers),
    ("transfers-uploads", transfers_uploads),
    ("transfer-conflict", transfer_conflict),
    ("drop-list", drop_list),
    ("drop-folder", drop_folder),
    ("toolbar-ring", toolbar_ring),
    ("quit-confirm", quit_confirm),
];
