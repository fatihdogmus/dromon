//! The real app's [`Store`]: Rune's `prefs.json` in the app's config folder (`~/.config/dromon`,
//! `~/Library/Application Support/dromon`, `%APPDATA%\dromon`; `RUNE_PREFS_DIR` moves it), read
//! once before the UI starts and written by a `dromon-prefs` thread, so the UI thread never
//! touches the disk. Each write replaces the file whole (Rune writes a temporary file and
//! renames it), and only the newest of several queued snapshots is written.

use std::sync::mpsc;
use std::thread::JoinHandle;

use rune::prefs::Prefs;

use crate::state::{Saved, Store};

/// The app id: the config folder's name.
pub const APP_ID: &str = "dromon";
/// The key [`Saved`] is stored under.
const KEY: &str = "state";

/// The remembered settings in `prefs` (default when missing or unreadable).
pub fn load(prefs: &Prefs) -> Saved {
    prefs.get(KEY).unwrap_or_default()
}

/// Writes [`Saved`] snapshots on its own thread. Dropping it writes what is still queued and
/// waits for that (quitting).
pub struct PrefsWriter {
    tx: Option<mpsc::Sender<Saved>>,
    thread: Option<JoinHandle<()>>,
}

impl PrefsWriter {
    pub fn spawn(mut prefs: Prefs) -> PrefsWriter {
        let (tx, rx) = mpsc::channel::<Saved>();
        let thread = std::thread::Builder::new()
            .name("dromon-prefs".into())
            .spawn(move || {
                let mut warned = false;
                while let Ok(mut saved) = rx.recv() {
                    // Skip to the newest snapshot.
                    while let Ok(newer) = rx.try_recv() {
                        saved = newer;
                    }
                    if let Err(e) = prefs.set(KEY, &saved)
                        && !std::mem::replace(&mut warned, true)
                    {
                        eprintln!("dromon: settings aren't saved: {e}");
                    }
                }
            })
            .ok();
        PrefsWriter { tx: Some(tx), thread }
    }
}

impl Store for PrefsWriter {
    fn save(&mut self, saved: &Saved) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(saved.clone());
        }
    }
}

impl Drop for PrefsWriter {
    fn drop(&mut self) {
        // Closing the channel ends the thread once it has written the last snapshot.
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Sort;
    use dromon_core::{SortDir, SortKey};

    #[test]
    fn the_last_snapshot_is_on_disk_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("prefs.json");
        let mut writer = PrefsWriter::spawn(Prefs::at(&file));
        let mut saved = Saved::default();
        for w in [200.0, 230.0, 260.0] {
            saved.sidebar_width = Some(w);
            writer.save(&saved);
        }
        saved.sort = Some(Sort { key: SortKey::Size, dir: SortDir::Descending });
        saved.set_folder("prod", "/var/www");
        writer.save(&saved);
        drop(writer);
        assert_eq!(load(&Prefs::at(&file)), saved);
    }

    #[test]
    fn a_missing_or_foreign_file_loads_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&Prefs::at(dir.path().join("none.json"))), Saved::default());
        let file = dir.path().join("prefs.json");
        std::fs::write(&file, r#"{"state": {"sidebar_width": "wide", "show_hidden": false}}"#).unwrap();
        // A field of the wrong type spoils the whole value: back to defaults, not a crash.
        assert_eq!(load(&Prefs::at(&file)), Saved::default());
        std::fs::write(&file, r#"{"state": {"show_hidden": false, "from_the_future": 1}}"#).unwrap();
        assert_eq!(load(&Prefs::at(&file)).show_hidden, Some(false));
    }
}
