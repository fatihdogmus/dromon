//! The fixture's transfer engine (the real queue on fake, instant I/O) and the `transfers`
//! key of the fixture format.

use std::path::PathBuf;
use std::sync::Arc;

use dromon_core::{EntryKind, HostId, RemotePath};
use dromon_transfer::{ConflictChoice, ConflictView, JobId, JobKind, JobRequest, JobView, Progress, RemoteItem, Reveal, TaskState};

use super::transfers::UPLOAD_SIZE;
use super::*;
use crate::state::{Backend, Command, ServiceEvent, TransferCommand};

const SITE: &str = "/var/www/site";

fn prod() -> HostId {
    HostId("prod".into())
}

fn site() -> RemotePath {
    RemotePath::new(SITE)
}

/// The demo's backend, connected to prod and listing its site.
fn connected() -> FixtureBackend {
    let mut b = FixtureBackend::new(demo());
    b.send(Command::Connect { host: prod() });
    b.send(Command::List { host: prod(), path: site() });
    b
}

fn send(b: &mut FixtureBackend, cmd: TransferCommand) -> Vec<ServiceEvent> {
    b.send(Command::Transfer(cmd))
}

/// The jobs of the one `Transfers` snapshot in `evs`.
fn jobs(evs: &[ServiceEvent]) -> Vec<JobView> {
    let snaps: Vec<&Vec<JobView>> = evs.iter().filter_map(|e| if let ServiceEvent::Transfers(j) = e { Some(j) } else { None }).collect();
    assert_eq!(snaps.len(), 1, "one snapshot per command: {evs:#?}");
    snaps[0].clone()
}

/// The names a listing of `path` in `evs` ends with (pages joined).
fn listed(evs: &[ServiceEvent], path: &str) -> Option<Vec<String>> {
    let mut names: Option<Vec<String>> = None;
    for e in evs {
        if let ServiceEvent::ListingPage { path: p, entries, first, .. } = e
            && p.as_str() == path
        {
            if *first {
                names = Some(Vec::new());
            }
            names.get_or_insert_default().extend(entries.iter().map(|e| e.name.clone()));
        }
    }
    names
}

fn download(names: &[(&str, EntryKind, u64)]) -> TransferCommand {
    let items = names.iter().map(|(n, kind, size)| RemoteItem { path: site().join(n), kind: *kind, size: *size }).collect();
    TransferCommand::Start(JobRequest::Download { host: prod(), items, into: PathBuf::from("/dl") })
}

fn upload(names: &[&str], into: &str) -> TransferCommand {
    let items = names.iter().map(|n| PathBuf::from(format!("/Users/me/{n}"))).collect();
    TransferCommand::Start(JobRequest::Upload { host: prod(), items, into: RemotePath::new(into) })
}

#[test]
fn downloads_finish_at_once_and_a_second_copy_gets_a_chrome_style_name() {
    let mut b = connected();
    let first = jobs(&send(&mut b, download(&[("index.html", EntryKind::File, 14812)])));
    assert_eq!(first.len(), 1);
    let j = &first[0];
    assert_eq!((j.id, j.kind, j.title.as_str(), &j.state), (JobId(1), JobKind::Download, "index.html", &TaskState::Done));
    assert_eq!((j.progress, j.files_done, j.files_total), (Progress { done: 14812, total: 14812 }, 1, 1));
    assert_eq!(j.reveal, Some(Reveal::Local(PathBuf::from("/dl/index.html"))));

    let again = jobs(&send(&mut b, download(&[("index.html", EntryKind::File, 14812)])));
    assert_eq!(again.iter().map(|j| j.id.0).collect::<Vec<_>>(), [2, 1], "newest first");
    assert_eq!(again[0].reveal, Some(Reveal::Local(PathBuf::from("/dl/index (1).html"))));
}

#[test]
fn a_folder_download_walks_the_tree_and_leaves_links_to_folders_out() {
    let mut b = connected();
    let www = RemoteItem { path: RemotePath::new("/var/www"), kind: EntryKind::Dir, size: 4096 };
    let req = JobRequest::Download { host: prod(), items: vec![www], into: PathBuf::from("/dl") };
    let j = jobs(&send(&mut b, TransferCommand::Start(req))).remove(0);
    assert_eq!(j.state, TaskState::Done);
    assert_eq!(j.title, "www");
    // site's 11 files and release 20261125-1348's 3; html and site/current are links.
    assert_eq!((j.files_done, j.files_total), (14, 14));
    assert_eq!(j.skipped, ["html: a link to a folder", "site/current: a link to a folder"]);
    assert_eq!(j.reveal, Some(Reveal::Local(PathBuf::from("/dl/www"))));
}

#[test]
fn an_upload_to_a_free_name_lands_and_the_folder_on_screen_lists_it_again() {
    let mut b = connected();
    let evs = send(&mut b, upload(&["notes.txt"], SITE));
    let j = jobs(&evs).remove(0);
    assert_eq!((j.kind, &j.state, j.progress.total), (JobKind::Upload, &TaskState::Done, UPLOAD_SIZE));
    assert_eq!(j.reveal, Some(Reveal::Remote(prod(), site().join("notes.txt"))));
    let names = listed(&evs, SITE).expect("the site is on screen, so it lists again");
    assert!(names.contains(&"notes.txt".to_owned()));

    // Into a subfolder: it changes, but isn't on screen.
    let evs = send(&mut b, upload(&["a.png"], "/var/www/site/uploads"));
    assert_eq!(listed(&evs, "/var/www/site/uploads"), None);
    let later = b.send(Command::List { host: prod(), path: RemotePath::new("/var/www/site/uploads") });
    assert_eq!(listed(&later, "/var/www/site/uploads"), Some(vec!["a.png".to_owned()]));
}

#[test]
fn a_clash_waits_for_the_user_and_keep_both_uploads_under_a_free_name() {
    let mut b = connected();
    let j = jobs(&send(&mut b, upload(&["index.html", "notes.txt"], SITE))).remove(0);
    assert_eq!(j.title, "2 items");
    assert!(matches!(j.state, TaskState::Preparing { .. }), "{:?}", j.state);
    assert_eq!(j.conflict, Some(ConflictView { name: "index.html".into(), folder: site(), existing_is_dir: false, more: 0 }));

    let evs = send(&mut b, TransferCommand::Resolve { job: j.id, choice: ConflictChoice::KeepBoth, apply_all: false });
    let j = jobs(&evs).remove(0);
    assert_eq!((&j.state, j.files_total, j.conflict.as_ref()), (&TaskState::Done, 2, None));
    let names = listed(&evs, SITE).unwrap();
    assert!(names.contains(&"index (1).html".to_owned()) && names.contains(&"notes.txt".to_owned()), "{names:?}");
    assert_eq!(names.iter().filter(|n| n.as_str() == "index.html").count(), 1);
}

#[test]
fn replace_overwrites_and_apply_to_all_answers_every_clash() {
    let mut b = connected();
    let j = jobs(&send(&mut b, upload(&["index.html"], SITE))).remove(0);
    send(&mut b, TransferCommand::Resolve { job: j.id, choice: ConflictChoice::Replace, apply_all: false });
    let entries = b.send(Command::List { host: prod(), path: site() });
    let size = entries.iter().find_map(|e| match e {
        ServiceEvent::ListingPage { entries, .. } => entries.iter().find(|e| e.name == "index.html").map(|e| e.size),
        _ => None,
    });
    assert_eq!(size, Some(UPLOAD_SIZE), "replaced");

    let j = jobs(&send(&mut b, upload(&["robots.txt", "favicon.ico", "README.md", "new.txt"], SITE))).remove(0);
    assert_eq!(j.conflict.as_ref().map(|c| (c.name.as_str(), c.more)), Some(("robots.txt", 2)));
    let j = jobs(&send(&mut b, TransferCommand::Resolve { job: j.id, choice: ConflictChoice::Skip, apply_all: true })).remove(0);
    assert_eq!(j.state, TaskState::Done);
    assert_eq!(j.skipped, ["robots.txt", "favicon.ico", "README.md"]);
    assert_eq!(j.files_total, 1);
}

#[test]
fn nothing_starts_while_the_host_is_down_and_the_job_says_it_waits() {
    let mut b = FixtureBackend::new(demo());
    let j = jobs(&send(&mut b, download(&[("index.html", EntryKind::File, 14812)]))).remove(0);
    assert_eq!((&j.state, j.waiting_for_connection), (&TaskState::Queued, true));
    b.send(Command::Connect { host: prod() });
    let j = jobs(&send(&mut b, TransferCommand::ClearFinished)).remove(0);
    assert_eq!((&j.state, j.waiting_for_connection), (&TaskState::Done, false), "the next command finds the host up");
    assert!(jobs(&send(&mut b, TransferCommand::ClearFinished)).is_empty());
}

#[test]
fn an_upload_into_a_folder_that_isnt_there_fails_with_why() {
    let mut b = connected();
    let j = jobs(&send(&mut b, upload(&["a.txt"], "/nope"))).remove(0);
    assert_eq!(j.state, TaskState::Failed { error: "a.txt: /nope doesn't exist".into() });
    assert_eq!(j.errors, ["a.txt: /nope doesn't exist"]);
}

#[test]
fn the_same_commands_give_the_same_events() {
    let run = || {
        let mut b = connected();
        let mut all = send(&mut b, upload(&["index.html", "x.bin"], SITE));
        all.extend(send(&mut b, TransferCommand::Resolve { job: JobId(1), choice: ConflictChoice::KeepBoth, apply_all: false }));
        all.extend(send(&mut b, download(&[("assets", EntryKind::Dir, 4096), ("robots.txt", EntryKind::File, 68)])));
        all
    };
    assert_eq!(run(), run());
}

const WITH_JOBS: &str = r#"{"version": 1, "now": "2026-11-25T14:00:00Z",
    "hosts": [{"alias": "prod", "source": "ssh-config"}],
    "servers": {"prod": {"home": "/srv"}},
    "transfers": [
        {"kind": "download", "host": "prod", "title": "photos", "state": "running", "done": 12400000, "total": 80000000,
         "files": [3, 40], "speed": 8100000, "eta": 9, "reveal": "/Users/me/Downloads/photos"},
        {"kind": "upload", "host": "prod", "title": "site", "state": "preparing", "found": 1204, "waiting": true},
        {"kind": "upload", "host": "prod", "title": "index.html", "state": "preparing",
         "conflict": {"name": "index.html", "folder": "/var/www", "more": 2}},
        {"kind": "download", "host": "prod", "title": "db.sql", "state": "failed", "error": "Permission denied",
         "errors": ["db.sql: Permission denied"], "skipped": ["cache: a link to a folder"]},
        {"kind": "upload", "host": "prod", "title": "a.txt", "state": "done", "done": 5, "total": 5, "reveal": "/srv/a.txt"}
    ]}"#;

#[test]
fn the_transfers_key_reads_job_snapshots_newest_first() {
    let fx = parse(WITH_JOBS).unwrap();
    let t = &fx.transfers;
    assert_eq!(t.len(), 5);
    assert_eq!(t.iter().map(|j| j.id.0 - FIXTURE_JOBS).collect::<Vec<_>>(), [5, 4, 3, 2, 1]);
    assert_eq!(t[0].state, TaskState::Running);
    assert_eq!((t[0].progress, t[0].files_done, t[0].files_total), (Progress { done: 12_400_000, total: 80_000_000 }, 3, 40));
    assert_eq!((t[0].speed, t[0].eta), (Some(8_100_000.0), Some(std::time::Duration::from_secs(9))));
    assert_eq!(t[0].reveal, Some(Reveal::Local(PathBuf::from("/Users/me/Downloads/photos"))));
    assert_eq!((&t[1].state, t[1].waiting_for_connection), (&TaskState::Preparing { files_found: 1204 }, true));
    assert_eq!(t[2].conflict, Some(ConflictView { name: "index.html".into(), folder: RemotePath::new("/var/www"), existing_is_dir: false, more: 2 }));
    assert_eq!(t[3].state, TaskState::Failed { error: "Permission denied".into() });
    assert_eq!((t[3].errors.len(), t[3].skipped.len()), (1, 1));
    assert_eq!(t[4].reveal, Some(Reveal::Remote(prod(), RemotePath::new("/srv/a.txt"))));

    // The app starts with them.
    let app = app(Arc::new(fx));
    assert_eq!(app.transfers.jobs.len(), 5);
    assert_eq!(app.transfers.active(), 3);
}

#[test]
fn the_docs_transfers_example_reads() {
    let doc = include_str!("../../../../docs/fixtures.md");
    let start = doc.find("## Transfers").and_then(|at| doc[at..].find("```json\n").map(|i| at + i + 8)).unwrap();
    let end = start + doc[start..].find("\n```").unwrap();
    let src = format!(
        r#"{{"version": 1, "hosts": [{{"alias": "prod", "source": "saved"}}, {{"alias": "nas", "source": "saved"}}], {}}}"#,
        &doc[start..end]
    );
    let fx = parse(&src).unwrap_or_else(|e| panic!("docs/fixtures.md: {e}"));
    let states: Vec<&TaskState> = fx.transfers.iter().map(|j| &j.state).collect();
    assert_eq!(states.len(), 6);
    assert_eq!(states[1], &TaskState::Preparing { files_found: 1204 });
    assert_eq!(fx.transfers[3].conflict.as_ref().map(|c| c.more), Some(1));
}

#[test]
fn transfers_errors_name_where() {
    let bad = |job: &str| {
        let src = format!(r#"{{"version": 1, "hosts": [{{"alias": "prod", "source": "saved"}}], "transfers": [{job}]}}"#);
        parse(&src).unwrap_err().to_string()
    };
    let ok = r#""kind": "download", "host": "prod", "title": "t""#;
    assert_eq!(bad(&format!(r#"{{{ok}, "state": "going"}}"#)), r#"transfers: job 1: state: unknown state "going" (expected queued, preparing, running, paused, done, failed or cancelled)"#);
    assert_eq!(bad(&format!(r#"{{{ok}, "state": "failed"}}"#)), "transfers: job 1: error: missing");
    assert_eq!(bad(&format!(r#"{{{ok}, "state": "running", "error": "x"}}"#)), "transfers: job 1: error: only a failed job has error");
    assert_eq!(bad(r#"{"kind": "copy", "host": "prod", "title": "t", "state": "done"}"#), r#"transfers: job 1: kind: unknown kind "copy" (expected download or upload)"#);
    assert_eq!(bad(r#"{"kind": "upload", "host": "nas", "title": "t", "state": "done"}"#), r#"transfers: job 1: host: "nas" is not a host (add it to hosts)"#);
    assert_eq!(bad(&format!(r#"{{{ok}, "state": "done", "done": 9, "total": 5}}"#)), "transfers: job 1: done: 9 is more than total (5)");
    assert_eq!(bad(&format!(r#"{{{ok}, "state": "done", "files": [3]}}"#)), "transfers: job 1: files: files is [done, total], whole numbers");
    assert_eq!(bad(&format!(r#"{{{ok}, "state": "done", "speed": -1}}"#)), "transfers: job 1: speed: expected a number of at least 0, found -1");
    assert!(bad(&format!(r#"{{{ok}, "state": "done", "colour": 1}}"#)).starts_with(r#"transfers: job 1: unknown field "colour""#));
    assert_eq!(
        bad(r#"{"kind": "upload", "host": "prod", "title": "t", "state": "done", "reveal": "relative"}"#),
        r#"transfers: job 1: reveal: "relative" is not an absolute path"#
    );
}

#[test]
fn the_fixtures_pictures_stay_after_the_sessions_jobs_and_their_buttons_move_them() {
    let fx = Arc::new(parse(WITH_JOBS).unwrap());
    let mut b = FixtureBackend::new(fx.clone());
    let photos = fx.transfers[0].id;
    let j = jobs(&send(&mut b, TransferCommand::Pause(photos)));
    assert_eq!(j.len(), 5, "no session jobs yet");
    assert_eq!((&j[0].state, j[0].speed), (&TaskState::Paused, None));
    let j = jobs(&send(&mut b, TransferCommand::Resume(photos)));
    assert_eq!(j[0].state, TaskState::Running);

    b.send(Command::Connect { host: prod() });
    let item = RemoteItem { path: RemotePath::new("/srv/x"), kind: EntryKind::File, size: 1 };
    let j = jobs(&send(&mut b, TransferCommand::Start(JobRequest::Download { host: prod(), items: vec![item], into: PathBuf::from("/dl") })));
    assert_eq!(j.len(), 6);
    assert_eq!(j[0].id, JobId(1), "the session's job comes first");
    let j = jobs(&send(&mut b, TransferCommand::ClearFinished));
    assert_eq!(j.iter().map(|j| j.title.as_str()).collect::<Vec<_>>(), ["photos", "site", "index.html"]);
}
