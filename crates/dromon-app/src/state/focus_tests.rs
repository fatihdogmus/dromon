use dromon_core::{ConnStatus, HostId};
use dromon_transfer::{JobId, JobKind, JobView, Progress, TaskState};

use super::Announcer;
use crate::scenes;
use crate::state::{Action, App, ServiceEvent};

fn job(id: u64, kind: JobKind, title: &str, state: TaskState) -> JobView {
    JobView {
        id: JobId(id),
        kind,
        host: HostId("prod".into()),
        title: title.into(),
        state,
        progress: Progress { done: 0, total: 10 },
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

fn listing() -> App {
    let mut s = scenes::base();
    scenes::listing(&mut s);
    s
}

fn status(s: &mut App, status: ConnStatus) {
    s.apply(ServiceEvent::Status { host: HostId("prod".into()), status });
}

#[test]
fn what_was_there_at_the_first_look_is_not_news() {
    let mut s = listing();
    s.apply(ServiceEvent::Transfers(vec![job(1, JobKind::Download, "site", TaskState::Done)]));
    let mut a = Announcer::default();
    assert_eq!(a.news(&s), None);
    assert_eq!(a.news(&s), None, "nor the second time");
}

#[test]
fn a_job_is_said_once_when_it_ends() {
    let mut s = listing();
    let mut a = Announcer::default();
    s.apply(ServiceEvent::Transfers(vec![job(1, JobKind::Upload, "logs", TaskState::Running)]));
    a.news(&s);
    s.apply(ServiceEvent::Transfers(vec![job(1, JobKind::Upload, "logs", TaskState::Done)]));
    assert_eq!(a.news(&s).as_deref(), Some("Upload of logs finished"));
    assert_eq!(a.news(&s), None);
    // Retried after it failed, it says so again when it fails again.
    s.apply(ServiceEvent::Transfers(vec![job(2, JobKind::Move, "a", TaskState::Failed { error: "Gone".into() })]));
    assert_eq!(a.news(&s).as_deref(), Some("Move of a failed: Gone"), "one that ends before it was seen");
    s.apply(ServiceEvent::Transfers(vec![job(2, JobKind::Move, "a", TaskState::Running)]));
    assert_eq!(a.news(&s), None);
    s.apply(ServiceEvent::Transfers(vec![job(2, JobKind::Move, "a", TaskState::Failed { error: "Gone".into() })]));
    assert_eq!(a.news(&s).as_deref(), Some("Move of a failed: Gone"));
    // Cancelled is the user's doing: nothing to tell.
    s.apply(ServiceEvent::Transfers(vec![job(3, JobKind::Download, "b", TaskState::Cancelled)]));
    assert_eq!(a.news(&s), None);
}

#[test]
fn a_connection_lost_and_back_is_said() {
    let mut s = listing();
    let mut a = Announcer::default();
    a.news(&s);
    status(&mut s, ConnStatus::Reconnecting { attempt: 1 });
    assert_eq!(a.news(&s).as_deref(), Some("Connection to prod lost. Reconnecting…"));
    status(&mut s, ConnStatus::Reconnecting { attempt: 2 });
    assert_eq!(a.news(&s), None, "a second attempt isn't news");
    status(&mut s, ConnStatus::Ready);
    assert_eq!(a.news(&s).as_deref(), Some("Reconnected to prod"));
    status(&mut s, ConnStatus::Failed("Connection refused".into()));
    assert_eq!(a.news(&s).as_deref(), Some("Couldn't connect to prod"));
}

#[test]
fn a_delete_done_and_a_note_are_said() {
    let mut s = listing();
    let mut a = Announcer::default();
    a.news(&s);
    s.act(Action::SelectAll);
    s.act(Action::File(crate::state::FileAction::Delete));
    s.act(Action::File(crate::state::FileAction::ConfirmDelete));
    assert_eq!(a.news(&s).as_deref(), Some("Deleted 17 items"));
    assert_eq!(a.news(&s), None);
    s.files.toasts.push("/var/www/old was removed");
    assert_eq!(a.news(&s).as_deref(), Some("/var/www/old was removed"));
    assert_eq!(a.news(&s), None);
}

#[test]
fn several_changes_at_once_are_said_together() {
    let mut s = listing();
    let mut a = Announcer::default();
    s.apply(ServiceEvent::Transfers(vec![job(1, JobKind::Download, "x", TaskState::Running)]));
    a.news(&s);
    s.apply(ServiceEvent::Transfers(vec![job(1, JobKind::Download, "x", TaskState::Done)]));
    status(&mut s, ConnStatus::Reconnecting { attempt: 1 });
    assert_eq!(a.news(&s).as_deref(), Some("Download of x finished. Connection to prod lost. Reconnecting…"));
}
