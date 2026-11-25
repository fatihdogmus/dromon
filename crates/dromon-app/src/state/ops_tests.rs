//! The ops list: what `begin` records and how the service's `FileOp*` events move it along.

use dromon_core::{Error, HostId, RemotePath};

use super::*;
use crate::fixture;
use crate::state::{App, ServiceEvent};

fn prod() -> HostId {
    HostId("prod".into())
}

fn delete(paths: &[&str]) -> FileOp {
    FileOp::Delete { paths: paths.iter().map(|p| RemotePath::new(p)).collect() }
}

#[test]
fn begin_hands_out_fresh_ids_and_records_the_kind() {
    let mut ops = Ops::default();
    let a = ops.begin(prod(), &FileOp::Mkdir { path: RemotePath::new("/a") });
    let b = ops.begin(prod(), &delete(&["/x", "/y", "/z"]));
    assert_ne!(a, b);
    assert_eq!(ops.get(a).map(|o| (o.kind, o.items, o.finished)), Some((FileOpKind::Mkdir, 1, false)));
    assert_eq!(ops.get(b).map(|o| (o.kind, o.items, o.total)), Some((FileOpKind::Delete, 3, None)));
    assert_eq!(ops.running().count(), 2);
}

#[test]
fn progress_then_done_or_failed() {
    let mut ops = Ops::default();
    let id = ops.begin(prod(), &delete(&["/srv/old"]));
    ops.progress(id, &prod(), 40, None);
    assert_eq!(ops.get(id).map(|o| (o.done, o.total)), Some((40, None)));
    ops.progress(id, &prod(), 12, Some(41));
    assert_eq!(ops.get(id).map(|o| (o.done, o.total)), Some((12, Some(41))));
    ops.done(id, &prod());
    let o = ops.get(id).unwrap();
    assert!(o.succeeded());
    assert_eq!(o.done, 41, "a finished delete shows everything removed");
    // Late events for a finished op change nothing.
    ops.progress(id, &prod(), 1, Some(41));
    ops.failed(id, &prod(), Error::ConnectionLost);
    assert!(ops.get(id).unwrap().succeeded());

    let other = ops.begin(prod(), &FileOp::Rename { from: RemotePath::new("/a"), to: RemotePath::new("/b") });
    ops.failed(other, &prod(), Error::AlreadyExists(RemotePath::new("/b")));
    let o = ops.get(other).unwrap();
    assert!(o.finished && !o.succeeded());
    assert_eq!(o.error, Some(Error::AlreadyExists(RemotePath::new("/b"))));
    assert_eq!(ops.running().count(), 0);

    ops.forget(id);
    assert!(ops.get(id).is_none());
    ops.clear_finished();
    assert!(ops.ops.is_empty());
}

#[test]
fn events_for_unknown_ids_or_another_host_are_ignored() {
    let mut ops = Ops::default();
    let id = ops.begin(prod(), &delete(&["/a"]));
    ops.done(OpId(99), &prod());
    ops.done(id, &HostId("pi".into()));
    assert!(!ops.get(id).unwrap().finished);
}

#[test]
fn the_app_applies_file_op_events_to_its_ops() {
    let mut s: App = fixture::app(fixture::demo());
    let id = s.ops.begin(prod(), &delete(&["/var/www/site/old"]));
    s.apply(ServiceEvent::FileOpProgress { id, host: prod(), done: 3, total: Some(9) });
    assert_eq!(s.ops.get(id).map(|o| (o.done, o.total)), Some((3, Some(9))));
    s.apply(ServiceEvent::FileOpFailed { id, host: prod(), error: Error::ConnectionLost });
    assert_eq!(s.ops.get(id).and_then(|o| o.error.clone()), Some(Error::ConnectionLost));
    let next = s.ops.begin(prod(), &FileOp::Mkdir { path: RemotePath::new("/tmp/n") });
    s.apply(ServiceEvent::FileOpDone { id: next, host: prod() });
    assert!(s.ops.get(next).unwrap().succeeded());
}
