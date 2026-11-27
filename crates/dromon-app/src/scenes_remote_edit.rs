//! Scenes of editing in a local editor: the transfers popover's Editing section with a file in
//! each state (editing, saving, changed on the server, a failed save with its toast), and the
//! edit conflict sheet (dark and light). The demo answers Edit at once; what the service would
//! say next is applied as its events.

use dromon_core::{Error, HostId, RemotePath};
use dromon_transfer::Side;

use crate::state::remote_edit::RemoteEditAction;
use crate::state::{Action, App, EditId, EditStage, RemoteEditEvent, ServiceEvent};

fn prod() -> HostId {
    HostId("prod".into())
}

/// Edits `name` of prod's site (its copy opens at once on the demo): its id.
fn edit(s: &mut App, name: &str) -> EditId {
    s.act(Action::RemoteEdit(RemoteEditAction::Edit(Some(name.into()))));
    s.edits.sessions.last().map(|e| e.id).expect("the edit")
}

fn news(s: &mut App, ev: RemoteEditEvent) {
    s.apply(ServiceEvent::RemoteEdit(ev));
}

/// index.html's save finds the server's copy changed since (today 13:52, 15,990 bytes; yours
/// 13:58).
fn conflicted(s: &mut App, id: EditId) {
    news(s, RemoteEditEvent::Uploading { id, host: prod() });
    let server = Side { size: Some(15_990), mtime: Some(1_795_614_720) };
    let local = Side { size: Some(14_960), mtime: Some(1_795_615_080) };
    news(s, RemoteEditEvent::Conflict { id, host: prod(), server, local });
}

/// Four files of prod's site being edited, the popover open: one open in the editor, one
/// saving, one changed on the server, one whose save failed (its toast offers Retry).
pub fn editing(s: &mut App) {
    super::listing(s);
    edit(s, "index.html");
    let saving = edit(s, "robots.txt");
    let changed = edit(s, "package.json");
    let failed = edit(s, "README.md");
    news(s, RemoteEditEvent::Uploading { id: saving, host: prod() });
    conflicted(s, changed);
    news(s, RemoteEditEvent::Uploading { id: failed, host: prod() });
    let error = Error::PermissionDenied(RemotePath::new("/var/www/site/README.md"));
    news(s, RemoteEditEvent::Failed { id: failed, host: prod(), stage: EditStage::Upload, error });
    s.act(Action::ToggleTransfers);
}

/// index.html changed on the server while it was being edited: Replace, Discard my changes or
/// Keep both.
pub fn edit_conflict(s: &mut App) {
    super::listing(s);
    let id = edit(s, "index.html");
    conflicted(s, id);
}

/// [`edit_conflict`] in the light theme.
pub fn edit_conflict_light(s: &mut App) {
    super::lit(s, edit_conflict);
}
