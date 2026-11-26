//! The clip: its text, when it still holds, and the job each paste makes.

use dromon_core::{EntryKind, HostId, RemotePath};
use dromon_transfer::{JobRequest, RemoteItem};

use super::*;

fn host(s: &str) -> HostId {
    HostId(s.into())
}

fn item(path: &str, kind: EntryKind) -> RemoteItem {
    RemoteItem { path: RemotePath::new(path), kind, size: 7 }
}

fn clip(cut: bool) -> Clip {
    Clip::new(
        host("prod"),
        vec![item("/var/www/site/index.html", EntryKind::File), item("/var/www/site/assets", EntryKind::Dir)],
        cut,
    )
}

#[test]
fn its_text_is_the_full_paths_one_per_line() {
    assert_eq!(clip(false).text, "/var/www/site/index.html\n/var/www/site/assets");
}

#[test]
fn it_holds_while_the_os_clipboard_has_its_text() {
    let c = clip(false);
    assert!(c.matches("/var/www/site/index.html\n/var/www/site/assets"));
    assert!(c.matches("/var/www/site/index.html\r\n/var/www/site/assets\r\n"), "Windows line ends");
    assert!(!c.matches("/var/www/site/index.html"), "something else was copied since");
    assert!(!c.matches(""));
    assert!(!Clip::new(host("prod"), Vec::new(), false).matches(""), "an empty clip never pastes");
}

#[test]
fn cut_rows_are_the_cut_items_on_their_host() {
    let c = clip(true);
    let index = RemotePath::new("/var/www/site/index.html");
    assert!(c.is_cut(&host("prod"), &index));
    assert!(!c.is_cut(&host("nas"), &index));
    assert!(!c.is_cut(&host("prod"), &RemotePath::new("/var/www/site/robots.txt")));
    assert!(!clip(false).is_cut(&host("prod"), &index), "a copy dims nothing");
}

#[test]
fn each_paste_makes_its_job() {
    let into = RemotePath::new("/srv");
    let items = clip(false).items;
    assert_eq!(
        clip(false).paste(&host("prod"), into.clone(), false),
        JobRequest::RemoteCopy { host: host("prod"), items: items.clone(), into: into.clone() }
    );
    assert_eq!(
        clip(true).paste(&host("prod"), into.clone(), false),
        JobRequest::Move { host: host("prod"), items: items.clone(), into: into.clone() }
    );
    assert_eq!(
        clip(false).paste(&host("prod"), into.clone(), true),
        JobRequest::Move { host: host("prod"), items: items.clone(), into: into.clone() },
        "Move here"
    );
    assert_eq!(
        clip(false).paste(&host("nas"), into.clone(), false),
        JobRequest::CrossHostCopy { from: host("prod"), items: items.clone(), to: host("nas"), into: into.clone() }
    );
    assert_eq!(
        clip(true).paste(&host("nas"), into.clone(), false),
        JobRequest::CrossHostMove { from: host("prod"), items, to: host("nas"), into }
    );
}
