//! What Dromon remembers between launches (§4.7 "Persisted state"): the sort, whether dotfiles
//! show, the sidebar's width and the last folder on each host. Never secrets, never anything a
//! server sent beyond a folder path.
//!
//! The app keeps a [`Saved`] and hands every change to a [`Store`]. The real app's store
//! (`crate::persist`) writes Rune's `prefs.json` on a background thread, since the UI thread
//! does no file I/O; scenes, fixtures and tests use [`NoStore`], so nothing on disk changes how
//! they look.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::conn::Sort;

#[cfg(test)]
#[path = "prefs_tests.rs"]
mod tests;

/// How many hosts' last folders are kept; the least recently used go first.
pub const MAX_FOLDERS: usize = 200;

/// The remembered settings. Every field is optional, so a file from another version (or a
/// value that no longer fits) falls back to the default for that field alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub sort: Option<Sort>,
    pub show_hidden: Option<bool>,
    pub sidebar_width: Option<f32>,
    /// The last folder on each host, by host id (the ssh alias).
    pub last_folder: BTreeMap<String, String>,
    /// Host ids, least recently used first: decides which folders [`MAX_FOLDERS`] drops.
    pub folder_order: Vec<String>,
}

impl Saved {
    /// Remembers `path` as the last folder on `host`.
    pub fn set_folder(&mut self, host: &str, path: &str) {
        self.folder_order.retain(|h| h != host);
        self.folder_order.push(host.to_owned());
        self.last_folder.insert(host.to_owned(), path.to_owned());
        while self.folder_order.len() > MAX_FOLDERS {
            let old = self.folder_order.remove(0);
            self.last_folder.remove(&old);
        }
    }

    pub fn folder(&self, host: &str) -> Option<&str> {
        self.last_folder.get(host).map(String::as_str)
    }
}

/// Where the app's [`Saved`] goes after each change.
pub trait Store {
    fn save(&mut self, saved: &Saved);
}

/// Keeps nothing: scenes, fixtures, tests and headless runs.
pub struct NoStore;

impl Store for NoStore {
    fn save(&mut self, _: &Saved) {}
}
