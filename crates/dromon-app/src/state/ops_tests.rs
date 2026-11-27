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
    s.apply(ServiceEvent::FileOp(FileOpEvent::Progress { id, host: prod(), done: 3, total: Some(9) }));
    assert_eq!(s.ops.get(id).map(|o| (o.done, o.total)), Some((3, Some(9))));
    s.apply(ServiceEvent::FileOp(FileOpEvent::Failed { id, host: prod(), error: Error::ConnectionLost }));
    assert_eq!(s.ops.get(id).and_then(|o| o.error.clone()), Some(Error::ConnectionLost));
    let next = s.ops.begin(prod(), &FileOp::Mkdir { path: RemotePath::new("/tmp/n") });
    s.apply(ServiceEvent::FileOp(FileOpEvent::Done { id: next, host: prod() }));
    assert!(s.ops.get(next).unwrap().succeeded());
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

#[test]
fn file_ops_name_what_they_change() {
    let mk = FileOp::Mkdir { path: p("/a/new") };
    assert_eq!((mk.kind(), mk.affected(), mk.removed()), (FileOpKind::Mkdir, vec![&p("/a/new")], vec![]));
    let mv = FileOp::Rename { from: p("/a/x"), to: p("/a/y") };
    assert_eq!(
        (mv.kind(), mv.affected(), mv.removed()),
        (FileOpKind::Rename, vec![&p("/a/x"), &p("/a/y")], vec![&p("/a/x")])
    );
    assert!(FileOp::Rename { from: p("/a/x"), to: p("/a/x") }.removed().is_empty());
    let rm = FileOp::Delete { paths: vec![p("/a/x"), p("/b")] };
    assert_eq!((rm.kind(), rm.removed()), (FileOpKind::Delete, vec![&p("/a/x"), &p("/b")]));
    assert_eq!(FileOpKind::Delete.name(), "Delete");
}

#[test]
fn the_folder_on_screen_is_listed_again_when_it_changed_and_still_exists() {
    let rm = FileOp::Delete { paths: vec![p("/srv/www/old"), p("/srv/logs")] };
    // The parent of a deleted item: listed again, whether or not the delete got through.
    assert!(rm.relists(&p("/srv/www"), true) && rm.relists(&p("/srv/www"), false));
    assert!(rm.relists(&p("/srv"), true));
    // Unrelated folders aren't.
    assert!(!rm.relists(&p("/srv/www/new"), true) && !rm.relists(&p("/"), true));
    // The deleted folder itself, or one inside it: gone after a success, so not listed;
    // after a failure it may still be there, and is.
    assert!(!rm.relists(&p("/srv/www/old"), true) && !rm.relists(&p("/srv/www/old/a/b"), true));
    assert!(rm.relists(&p("/srv/www/old"), false) && rm.relists(&p("/srv/logs/x"), false));

    let mk = FileOp::Mkdir { path: p("/home/u/new") };
    assert!(mk.relists(&p("/home/u"), true) && mk.relists(&p("/home/u"), false));
    assert!(!mk.relists(&p("/home"), true));

    let mv = FileOp::Rename { from: p("/home/u/a"), to: p("/home/u/b") };
    assert!(mv.relists(&p("/home/u"), true));
    // Renaming the folder on screen: its old path is gone.
    assert!(!mv.relists(&p("/home/u/a"), true) && mv.relists(&p("/home/u/a"), false));
    assert!(!mv.relists(&p("/home/u/c"), true));
}
