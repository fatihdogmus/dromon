//! A row drag's pill and the job its drop makes.

use dromon_core::{EntryKind, HostId, RemotePath};
use dromon_transfer::{JobRequest, RemoteItem};

use super::*;

fn host(s: &str) -> HostId {
    HostId(s.into())
}

fn drag(names: &[&str], over: Option<Over>) -> RowDrag {
    let folder = RemotePath::new("/var/www/site");
    let items = names.iter().map(|n| RemoteItem { path: folder.join(n), kind: EntryKind::File, size: 1 }).collect();
    RowDrag { host: host("prod"), folder, names: names.iter().map(|n| (*n).to_owned()).collect(), items, over }
}

fn over(target: DragTarget, copy: bool, to: &str, into: &str, label: &str) -> Option<Over> {
    Some(Over { target, copy, to: host(to), into: RemotePath::new(into), label: label.into() })
}

#[test]
fn the_pill_says_what_and_where() {
    let folder = over(DragTarget::Folder("uploads".into()), false, "prod", "/var/www/site/uploads", "uploads");
    assert_eq!(drag(&["a", "b", "c"], folder.clone()).pill().as_deref(), Some("Move 3 items to uploads"));
    let mut copy = folder.clone();
    copy.as_mut().unwrap().copy = true;
    assert_eq!(drag(&["a"], copy).pill().as_deref(), Some("Copy 1 item to uploads"));
    let nas = over(DragTarget::Host(host("nas")), true, "nas", "/volume1", "nas");
    assert_eq!(drag(&["a", "b", "c"], nas).pill().as_deref(), Some("Copy 3 items to nas"));
    assert_eq!(drag(&["a"], None).pill(), None, "nothing under it takes the rows");
}

#[test]
fn drops_move_or_copy_within_the_host_and_copy_to_another() {
    let d = drag(&["a"], over(DragTarget::Crumb(RemotePath::new("/var/www")), false, "prod", "/var/www", "www"));
    assert_eq!(
        d.request(),
        Some(JobRequest::Move { host: host("prod"), items: d.items.clone(), into: RemotePath::new("/var/www") })
    );
    let d = drag(&["a"], over(DragTarget::Folder("uploads".into()), true, "prod", "/var/www/site/uploads", "uploads"));
    assert_eq!(
        d.request(),
        Some(JobRequest::RemoteCopy {
            host: host("prod"),
            items: d.items.clone(),
            into: RemotePath::new("/var/www/site/uploads")
        })
    );
    let d = drag(&["a"], over(DragTarget::Host(host("nas")), true, "nas", "/volume1", "nas"));
    assert_eq!(
        d.request(),
        Some(JobRequest::CrossHostCopy {
            from: host("prod"),
            items: d.items.clone(),
            to: host("nas"),
            into: RemotePath::new("/volume1")
        })
    );
    assert_eq!(drag(&["a"], None).request(), None);
}

#[test]
fn nothing_goes_into_itself() {
    let d = drag(&["assets"], None);
    assert!(d.contains(&RemotePath::new("/var/www/site/assets")));
    assert!(d.contains(&RemotePath::new("/var/www/site/assets/img")));
    assert!(!d.contains(&RemotePath::new("/var/www/site/assets2")));
    assert!(!d.contains(&RemotePath::new("/var/www/site")));
}
