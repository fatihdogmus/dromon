//! What a delete's toast says as it goes, and plain notes.

use dromon_core::{Error, HostId, RemotePath};

use super::*;
use crate::state::ops::Ops;
use crate::state::service::FileOp;

fn prod() -> HostId {
    HostId("prod".into())
}

fn names(n: &[&str]) -> Vec<String> {
    n.iter().map(|s| (*s).to_owned()).collect()
}

fn delete(ops: &mut Ops, paths: &[&str]) -> OpId {
    ops.begin(prod(), &FileOp::Delete { paths: paths.iter().map(|p| RemotePath::new(p)).collect() })
}

fn text(ops: &Ops, id: OpId, n: &[&str]) -> Option<(String, Tone)> {
    delete_toast(ops.get(id).unwrap(), &names(n)).map(|t| (t.text, t.tone))
}

#[test]
fn a_delete_counts_then_counts_down_then_goes() {
    let mut ops = Ops::default();
    let id = delete(&mut ops, &["/a", "/b", "/c"]);
    let n = ["a", "b", "c"];
    assert_eq!(text(&ops, id, &n), Some(("Deleting 3 items…".into(), Tone::Busy)));
    ops.progress(id, &prod(), 17, None);
    assert_eq!(text(&ops, id, &n), Some(("Deleting 3 items…".into(), Tone::Busy)), "still counting");
    ops.progress(id, &prod(), 1204, Some(5005));
    assert_eq!(text(&ops, id, &n), Some(("Deleting… 1,204 of 5,005".into(), Tone::Busy)));
    ops.done(id, &prod());
    assert_eq!(text(&ops, id, &n), None, "done: it goes");
}

#[test]
fn one_item_is_named_and_a_failure_stays_with_its_reason() {
    let mut ops = Ops::default();
    let id = delete(&mut ops, &["/var/www/site/assets"]);
    assert_eq!(text(&ops, id, &["assets"]), Some(("Deleting ‘assets’…".into(), Tone::Busy)));
    ops.failed(id, &prod(), Error::PermissionDenied(RemotePath::new("/var/www/site/assets/img")));
    let t = delete_toast(ops.get(id).unwrap(), &names(&["assets"])).unwrap();
    assert_eq!(t.text, "Couldn't delete ‘assets’: You don't have access to /var/www/site/assets/img");
    assert!(t.dismissible() && t.tone == Tone::Error);
    assert_eq!(t.id.key(), format!("toast-op-{}", id.0));
}

#[test]
fn other_operations_have_no_toast() {
    let mut ops = Ops::default();
    let id = ops.begin(prod(), &FileOp::Mkdir { path: RemotePath::new("/x") });
    assert_eq!(delete_toast(ops.get(id).unwrap(), &[]), None);
}

#[test]
fn notes_stay_until_dismissed() {
    let mut t = Toasts::default();
    let a = t.push("/var/www/old was removed");
    t.push("second");
    assert_eq!(t.notes().map(|n| n.text).collect::<Vec<_>>(), ["/var/www/old was removed", "second"]);
    let ToastId::Note(n) = a else { panic!("a note") };
    t.dismiss(n);
    assert_eq!(t.notes().count(), 1);
}
