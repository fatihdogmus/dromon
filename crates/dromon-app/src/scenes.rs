//! Named states for headless snapshots (`--scene NAME`), each reached by the same actions and
//! events a user and the service would produce. They start from the demo fixture (on its clock),
//! or from `--fixture`'s when one is given; the goldens are the demo's.

use dromon_core::{ConnStatus, HostId, Prompt, PromptId, PromptKind, RemotePath};
use rune::TextEdit;

use crate::fixture;
use crate::state::log::LogBuffer;
use crate::state::{Action, App, Effect, ServiceEvent, Silent};
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
];
