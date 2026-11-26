//! Moves on one host: a picked item is renamed as a whole ([`move_remote`]); nothing is copied,
//! so mtimes and contents stay as they are. And the tidying after a move between hosts
//! ([`remove_empty_dirs`]).
//!
//! Replace on a folder merges, as uploads do: each child is moved into the existing folder
//! (a file replacing its namesake, a folder merging again), and the source folders, emptied,
//! are removed deepest first. Every step is one atomic rename, so a move dropped mid-way leaves
//! each item in one place or the other, never half of a file; running it again carries on.

use dromon_core::{EntryKind, Error, RemotePath, Result};
use dromon_sftp::Sftp;

use super::free_remote_name;
use crate::FilePlan;

/// Moves one [`FilePlan::Move`] and returns where the item landed: `plan.dst`, or, without
/// `overwrite`, a free `name (1)` beside it when something appeared at `dst` since the clash
/// check. With `overwrite`, a file replaces a file (`posix-rename`; without it the target is
/// removed first) and a folder is merged into a folder; a file and a folder never replace each
/// other ([`Error::AlreadyExists`]).
pub async fn move_remote(sftp: &Sftp, plan: &FilePlan) -> Result<RemotePath> {
    let FilePlan::Move { src, dst, overwrite, .. } = plan else {
        return Err(Error::Other(format!("move_remote() was given another kind of plan for {}", plan.name())));
    };
    let (Some(folder), Some(name)) = (dst.parent(), dst.file_name()) else {
        return Err(Error::Other(format!("Can't move to {dst}")));
    };
    // An earlier run renamed it and a drop lost the result: the source is gone and the item is
    // at the name the job chose (no `overwrite`: nothing was there when it was named).
    if !*overwrite && moved_before(sftp, plan, src, dst).await? {
        return Ok(dst.clone());
    }
    if *overwrite {
        replace(sftp, src, dst).await?;
        return Ok(dst.clone());
    }
    // Plain rename never replaces anything (OpenSSH refuses an existing target).
    let target =
        if sftp.exists(dst).await? { folder.join(&free_remote_name(sftp, &folder, name).await?) } else { dst.clone() };
    match sftp.rename(src, &target).await {
        Ok(()) => Ok(target),
        Err(Error::AlreadyExists(_)) => {
            let again = folder.join(&free_remote_name(sftp, &folder, name).await?);
            sftp.rename(src, &again).await?;
            Ok(again)
        }
        Err(e) => Err(e),
    }
}

/// Whether `src` is gone and `dst` holds the item (a file of the planned size, or a folder).
async fn moved_before(sftp: &Sftp, plan: &FilePlan, src: &RemotePath, dst: &RemotePath) -> Result<bool> {
    match sftp.lstat(src).await {
        Err(Error::NotFound(_)) => {}
        Ok(_) => return Ok(false),
        Err(e) => return Err(e),
    }
    match sftp.lstat(dst).await {
        Ok(e) if e.kind == EntryKind::Dir => Ok(true),
        Ok(e) => Ok(e.size == plan.size()),
        Err(Error::NotFound(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `src` takes `dst`'s place: renamed over a file or a free name, merged into a folder.
async fn replace(sftp: &Sftp, src: &RemotePath, dst: &RemotePath) -> Result<()> {
    let is_dir = sftp.lstat(src).await?.kind == EntryKind::Dir;
    // Folder pairs still to merge, and the source folders merged so far (parents first).
    let mut merges = Vec::new();
    let mut emptied = Vec::new();
    place(sftp, src.clone(), dst.clone(), is_dir, &mut merges).await?;
    while let Some((from, to)) = merges.pop() {
        for e in sftp.list(&from).await? {
            let is_dir = e.kind == EntryKind::Dir;
            place(sftp, from.join(&e.name), to.join(&e.name), is_dir, &mut merges).await?;
        }
        emptied.push(from);
    }
    for dir in emptied.iter().rev() {
        sftp.remove_dir(dir).await?;
    }
    Ok(())
}

/// Moves `src` to `dst`; a folder onto a folder is left in `merges` instead. Only real folders
/// merge: a link is moved (or replaced) as itself.
async fn place(
    sftp: &Sftp,
    src: RemotePath,
    dst: RemotePath,
    src_is_dir: bool,
    merges: &mut Vec<(RemotePath, RemotePath)>,
) -> Result<()> {
    let dst_is_dir = match sftp.lstat(&dst).await {
        Ok(e) => Some(e.kind == EntryKind::Dir),
        Err(Error::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    match (src_is_dir, dst_is_dir) {
        (true, Some(true)) => merges.push((src, dst)),
        (_, None) => rename_over(sftp, &src, &dst).await?,
        (false, Some(false)) => replace_file(sftp, &src, &dst).await?,
        (true, Some(false)) | (false, Some(true)) => return Err(Error::AlreadyExists(dst)),
    }
    Ok(())
}

/// Renames `src` to `dst`, which was free a moment ago.
async fn rename_over(sftp: &Sftp, src: &RemotePath, dst: &RemotePath) -> Result<()> {
    match sftp.posix_rename(src, dst).await {
        Err(Error::Unsupported(_)) => sftp.rename(src, dst).await,
        res => res,
    }
}

/// Replaces the file (or link) `dst` with `src`: atomically with `posix-rename`; without it
/// `dst` is removed first.
async fn replace_file(sftp: &Sftp, src: &RemotePath, dst: &RemotePath) -> Result<()> {
    match sftp.posix_rename(src, dst).await {
        Err(Error::Unsupported(_)) => {
            match sftp.remove_file(dst).await {
                Ok(()) | Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
            sftp.rename(src, dst).await
        }
        res => res,
    }
}

/// Removes every empty folder in the tree at `root`, deepest first, `root` included: what is
/// left of a folder moved to another host. Folders that still hold anything (a file that
/// failed or was skipped, a link) stay, as do unreadable ones; links are never followed. Best
/// effort: only a lost connection is an error.
pub async fn remove_empty_dirs(sftp: &Sftp, root: &RemotePath) -> Result<()> {
    // Breadth first, so every folder comes after its parent; removed in reverse.
    let mut dirs = vec![root.clone()];
    let mut at = 0;
    while let Some(dir) = dirs.get(at).cloned() {
        at += 1;
        match sftp.list(&dir).await {
            Ok(entries) => {
                dirs.extend(entries.into_iter().filter(|e| e.kind == EntryKind::Dir).map(|e| dir.join(&e.name)));
            }
            Err(Error::ConnectionLost) => return Err(Error::ConnectionLost),
            Err(_) => {}
        }
    }
    for dir in dirs.iter().rev() {
        if let Err(Error::ConnectionLost) = sftp.remove_dir(dir).await {
            return Err(Error::ConnectionLost);
        }
    }
    Ok(())
}
