//! Changes "made elsewhere" on a fixture's server (`changes` in `docs/fixtures.md`), for
//! polling to notice (§4.5): files appearing, going, growing, and the folder on screen being
//! deleted.
//!
//! There's no clock: each poll (`Command::Poll`, which the window gaining focus sends) or
//! refresh of the folder on screen (⌘R, a relist after an operation) first applies the
//! server's next step, then lists. A script makes "a poll happened now" with
//! `{"focus": false}`, `{"focus": true}`, or presses ⌘R. Steps run out after the last one.

use dromon_core::{Error, HostId};

use super::Change;
use super::tree::Tree;

/// Applies `step` to `host`'s folders. A change whose folder doesn't exist (any more) is
/// skipped, as an edit on a server that lost the folder would fail.
pub fn apply(tree: &mut Tree, host: &HostId, step: &[Change]) {
    for change in step {
        let _ = apply_one(tree, host, change);
    }
}

fn apply_one(tree: &mut Tree, host: &HostId, change: &Change) -> Result<(), Error> {
    match change {
        Change::Add { path, entry } => tree.put_entry(host, path, entry.clone()),
        Change::Remove { path } => {
            if tree.entry_at(host, path).is_none() {
                return Err(Error::NotFound(path.clone()));
            }
            tree.remove(host, path)
        }
        Change::Modify { path, size, mtime } => {
            let mut entry = tree.entry_at(host, path).ok_or_else(|| Error::NotFound(path.clone()))?;
            if let Some(size) = size {
                entry.size = *size;
            }
            if let Some(mtime) = mtime {
                entry.mtime = Some(*mtime);
            }
            // Only the entry changes: a folder keeps what it holds.
            tree.retouch(host, path, entry)
        }
    }
}
