//! The editing folder (its layout, permissions, cleanup and the sweep of what runs that are
//! gone left) and the watch's debounce, on real files in a temporary folder; and what a host's
//! edits answer without a session. Uploads and downloads are `tests/remote_edit_e2e.rs`'s.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use dromon_core::{Error, HostId, RemotePath};

use super::*;
use crate::service::Sink;
use crate::state::{EditId, EditStage, RemoteEditCommand, RemoteEditEvent, ServiceEvent};

fn host(s: &str) -> HostId {
    HostId(s.into())
}

/// Writes `bytes` to `path`, dated `at` seconds after an hour ago (so each write's stamp is its
/// own, however fast the test runs).
fn write(path: &Path, bytes: &[u8], at: u64) {
    std::fs::write(path, bytes).unwrap();
    let t = SystemTime::now() - Duration::from_secs(3600) + Duration::from_secs(at);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

#[test]
fn a_save_through_a_temporary_file_renamed_over_the_copy_counts_once() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("notes.txt");
    write(&copy, b"hello\n", 1);
    let mut w = Watch::default();
    w.settle(stamp(&copy));
    assert!(!w.look(stamp(&copy)), "nothing changed");
    // The editor writes its new file beside the copy, removes the copy, renames the new file.
    let tmp = dir.path().join(".notes.txt.swp");
    write(&tmp, b"hello world\n", 2);
    assert!(!w.look(stamp(&copy)), "the copy is as it was");
    std::fs::remove_file(&copy).unwrap();
    assert!(!w.look(stamp(&copy)), "gone for a moment: not a save");
    std::fs::rename(&tmp, &copy).unwrap();
    assert!(!w.look(stamp(&copy)), "just changed: it may not be done");
    assert!(w.look(stamp(&copy)), "held still for a look: a save");
    assert!(!w.look(stamp(&copy)), "once");
    assert!(!w.look(stamp(&copy)));
}

#[test]
fn a_write_in_steps_counts_once_it_is_done() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("app.conf");
    write(&copy, b"a = 1\n", 1);
    let mut w = Watch::default();
    w.settle(stamp(&copy));
    // Truncated, then written, a look apart: one save, of the whole file.
    write(&copy, b"", 2);
    assert!(!w.look(stamp(&copy)));
    write(&copy, b"a = 2\nb = 3\n", 3);
    assert!(!w.look(stamp(&copy)), "still changing");
    assert!(w.look(stamp(&copy)));
    assert!(!w.look(stamp(&copy)));
    // A later save counts again; a download settles the watch, so its own write isn't one.
    write(&copy, b"a = 4\n", 4);
    assert!(!w.look(stamp(&copy)));
    assert!(w.look(stamp(&copy)));
    write(&copy, b"from the server\n", 5);
    w.settle(stamp(&copy));
    assert!(!w.look(stamp(&copy)));
}

#[test]
fn each_host_and_path_has_a_folder_of_its_own_named_for_its_host() {
    let run = Path::new("/run");
    let a = session_dir(run, &host("prod"), &RemotePath::new("/etc/app.conf"));
    assert_eq!(a, session_dir(run, &host("prod"), &RemotePath::new("/etc/app.conf")), "the same each time");
    let others = [
        session_dir(run, &host("nas"), &RemotePath::new("/etc/app.conf")),
        session_dir(run, &host("prod"), &RemotePath::new("/srv/app.conf")),
    ];
    assert!(others.iter().all(|o| *o != a), "{a:?} {others:?}");
    let quick = session_dir(run, &host("deploy@10.0.0.7:2222"), &RemotePath::new("/a"));
    let name = quick.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("deploy_10.0.0.7_2222-"), "{name}");
    assert_eq!(quick.parent(), Some(run));
}

#[test]
fn a_copy_keeps_the_files_own_name_but_what_no_folder_can_hold() {
    assert_eq!(local_name("notes.txt", false), "notes.txt");
    assert_eq!(local_name("a:b?.txt", false), "a:b?.txt", "fine on Unix");
    assert_eq!(local_name("a:b?.txt", true), "a_b_.txt");
    assert_eq!(local_name("back\\slash|pipe", true), "back_slash_pipe");
    assert_eq!(local_name("trailing.", true), "trailing_");
    assert_eq!(local_name("con.txt", true), "_con.txt");
    assert_eq!(local_name("COM1", true), "_COM1");
    assert_eq!(local_name("console.log", true), "console.log");
    assert_eq!(local_name("..", false), "file");
    assert_eq!(local_name("", true), "file");
    assert_eq!(local_name("a/b\0c", false), "a_b_c");
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_run_folder_is_private_locked_and_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cache").join("editing");
    let run = Run::open(&root).unwrap();
    assert!(run.dir.is_dir() && lock_of(&run.dir).is_file());
    assert_eq!(run.dir.parent(), Some(root.as_path()));
    #[cfg(unix)]
    {
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&run.dir), 0o700);
        assert_eq!(mode(&lock_of(&run.dir)), 0o600);
    }
    // An edit's folder in it, and its copy.
    let edit = session_dir(&run.dir, &host("prod"), &RemotePath::new("/etc/app.conf"));
    private_dir(&edit).unwrap();
    std::fs::write(edit.join("app.conf"), b"secret = 1\n").unwrap();
    #[cfg(unix)]
    assert_eq!(mode(&edit), 0o700);
    // Another run in the same root has its own folder; a sweep while both live takes neither.
    let other = Run::open(&root).unwrap();
    assert_ne!(other.dir, run.dir);
    sweep(&root);
    assert!(run.dir.is_dir() && other.dir.is_dir());
    run.clear();
    assert!(!run.dir.exists() && !lock_of(&run.dir).exists(), "every copy goes");
    assert!(other.dir.is_dir());
}

#[cfg(unix)]
#[test]
fn a_link_in_the_folders_place_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let root = dir.path().join("editing");
    std::os::unix::fs::symlink(&elsewhere, &root).unwrap();
    assert!(matches!(Run::open(&root), Err(Error::Io(_))));
}

#[test]
fn the_sweep_removes_what_runs_that_are_gone_left_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("editing");
    let living = Run::open(&root).unwrap();
    // A crashed run: its folder and its lock file, which nobody holds.
    let crashed = root.join("1-dead-0");
    private_dir(&crashed.join("prod-0123")).unwrap();
    std::fs::write(crashed.join("prod-0123").join(".env"), b"DB_PASSWORD=x\n").unwrap();
    std::fs::write(lock_of(&crashed), b"").unwrap();
    // A folder without a lock file, and a lock file without its folder.
    let orphan = root.join("2-dead-0");
    std::fs::create_dir(&orphan).unwrap();
    let stray = root.join("3-dead-0.lock");
    std::fs::write(&stray, b"").unwrap();
    // A lock file another run holds while it makes its folder.
    let starting = root.join("4-alive-0.lock");
    let held = std::fs::File::create(&starting).unwrap();
    held.try_lock().unwrap();
    // Something that isn't Dromon's stays too.
    let other = root.join("notes.md");
    std::fs::write(&other, b"").unwrap();
    sweep(&root);
    assert!(!crashed.exists() && !lock_of(&crashed).exists(), "the crashed run's copies go");
    assert!(!orphan.exists() && !stray.exists());
    assert!(starting.exists(), "a held lock stays");
    assert!(living.dir.is_dir() && lock_of(&living.dir).is_file(), "a living run's folder stays");
    assert!(other.exists());
    drop(held);
}

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<ServiceEvent>>>);

impl Sink for Events {
    fn emit(&self, ev: ServiceEvent) {
        self.0.lock().unwrap().push(ev);
    }
}

impl Events {
    fn edits(&self) -> Vec<RemoteEditEvent> {
        let events = self.0.lock().unwrap();
        events
            .iter()
            .filter_map(|e| match e {
                ServiceEvent::RemoteEdit(ev) => Some(ev.clone()),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn without_a_session_an_edit_fails_at_once_and_unknown_edits_are_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut edits = Edits::new(Some(Arc::new(Editing::new(dir.path().join("editing")))));
    let events = Events::default();
    let (id, prod) = (EditId(3), host("prod"));
    let path = RemotePath::new("/etc/app.conf");
    edits.command(RemoteEditCommand::Start { id, host: prod.clone(), path }, &prod, &events);
    edits.command(RemoteEditCommand::Retry { id, host: prod.clone() }, &prod, &events);
    assert_eq!(
        events.edits(),
        [
            RemoteEditEvent::Failed {
                id,
                host: prod.clone(),
                stage: EditStage::Download,
                error: Error::ConnectionLost
            },
            RemoteEditEvent::Closed { id, host: prod },
        ]
    );
    assert!(!dir.path().join("editing").exists(), "nothing was made for it");
}

/// A service without an editing folder (`Config::editing_root: None`): an edit fails at once,
/// before its host's session is even looked at, and no task runs for it (there's no folder to
/// make its copy in, the user's cache folder least of all).
#[test]
fn without_an_editing_folder_an_edit_fails_at_once_and_nothing_is_made() {
    let mut edits = Edits::new(None);
    let events = Events::default();
    let (id, prod) = (EditId(4), host("prod"));
    let path = RemotePath::new("/etc/app.conf");
    edits.command(RemoteEditCommand::Start { id, host: prod.clone(), path }, &prod, &events);
    let error = Error::Other("Editing in a local editor isn't available".into());
    assert_eq!(events.edits(), [RemoteEditEvent::Failed { id, host: prod, stage: EditStage::Download, error }]);
    assert!(edits.running.is_empty(), "no edit runs");
}
