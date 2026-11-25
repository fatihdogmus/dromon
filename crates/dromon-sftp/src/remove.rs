//! Recursive delete: [`Sftp::remove_all`] and [`Sftp::remove_all_of`].
//!
//! Two passes:
//!
//! 1. **Counting.** Every folder to delete is listed, depth first, into a plan in post-order
//!    (a folder's contents before the folder). A delete has to list every folder anyway, so the
//!    count costs nothing extra: it is the listing, done before rather than during the
//!    removals, which also means a folder that can't be read stops the delete before anything
//!    is removed. Memory: one path per entry.
//! 2. **Removing.** The plan in order: files, symlinks and other non-folders with `remove`
//!    (up to [`REMOVES_IN_FLIGHT`] pipelined at once), each folder with `rmdir` once
//!    everything before it in the plan is gone.
//!
//! Symlinks are never followed: a link (to a folder or not) is an entry of its own, removed
//! with `remove`, and its target stays. The first error stops the delete and is returned (it
//! names the path); what was removed before it stays removed.

use dromon_core::{EntryKind, Error, RemotePath, Result};
use tokio::task::JoinSet;

use crate::error::{Op, map_err};
use crate::session::{Sftp, entry_from_attrs};

/// File removals kept in flight while deleting.
pub const REMOVES_IN_FLIGHT: usize = 64;

/// One entry to remove.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Doomed {
    /// Removed with `remove` (files, symlinks, pipes, sockets).
    File(RemotePath),
    /// Removed with `rmdir`, after its contents.
    Dir(RemotePath),
}

impl Sftp {
    /// Deletes `path`; a folder with everything inside it. See [`Sftp::remove_all_of`].
    pub async fn remove_all(&self, path: &RemotePath, progress: impl FnMut(u64, Option<u64>) + Send) -> Result<u64> {
        self.remove_all_of(std::slice::from_ref(path), progress).await
    }

    /// Deletes every path in `paths` (folders recursively, symlinks themselves, never their
    /// targets), counting everything first (see the module docs). Paths inside another one in
    /// the list are dropped (deleting the outer one deletes them).
    ///
    /// `progress(done, total)` is called as it goes: while counting with the entries found so
    /// far and `None`, then after each removal with the entries removed and `Some(total)`. It
    /// is called often (once per folder listed, once per entry removed): throttle in the caller.
    ///
    /// Returns how many entries were removed (each path itself included). The first error stops
    /// it, see [`Error`]: `NotFound`/`PermissionDenied` name the path that failed.
    pub async fn remove_all_of(
        &self,
        paths: &[RemotePath],
        mut progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<u64> {
        let mut plan = Vec::new();
        for root in outermost(paths) {
            let attrs = self.raw.lstat(root.as_str()).await.map_err(|e| map_err(e, Op::Stat, &root))?;
            if entry_from_attrs(String::new(), &attrs.attrs, "").kind == EntryKind::Dir {
                self.plan_dir(root, &mut plan, &mut progress).await?;
            } else {
                plan.push(Doomed::File(root));
                progress(plan.len() as u64, None);
            }
        }
        self.remove_planned(plan, progress).await
    }

    /// Lists `dir` and everything below it into `plan`, in post-order, without recursion.
    async fn plan_dir(
        &self,
        dir: RemotePath,
        plan: &mut Vec<Doomed>,
        progress: &mut (impl FnMut(u64, Option<u64>) + Send),
    ) -> Result<()> {
        // (folder, already listed): a listed folder is planned once its subfolders are.
        let mut stack = vec![(dir, false)];
        while let Some((dir, listed)) = stack.pop() {
            if listed {
                plan.push(Doomed::Dir(dir));
                continue;
            }
            let entries = self.list(&dir).await?;
            stack.push((dir.clone(), true));
            for e in entries {
                let path = dir.join(&e.name);
                match e.kind {
                    EntryKind::Dir => stack.push((path, false)),
                    EntryKind::File | EntryKind::Symlink { .. } | EntryKind::Other => plan.push(Doomed::File(path)),
                }
            }
            progress(plan.len() as u64 + stack.len() as u64, None);
        }
        Ok(())
    }

    async fn remove_planned(&self, plan: Vec<Doomed>, mut progress: impl FnMut(u64, Option<u64>) + Send) -> Result<u64> {
        let total = plan.len() as u64;
        let mut done = 0u64;
        let mut in_flight: JoinSet<Result<()>> = JoinSet::new();
        let mut failed: Option<Error> = None;
        let mut settle = |res: Option<std::result::Result<Result<()>, tokio::task::JoinError>>,
                          done: &mut u64,
                          failed: &mut Option<Error>| {
            match res {
                Some(Ok(Ok(()))) => {
                    *done += 1;
                    progress(*done, Some(total));
                }
                Some(Ok(Err(e))) => {
                    failed.get_or_insert(e);
                }
                Some(Err(e)) => {
                    failed.get_or_insert(Error::Other(format!("a delete stopped: {e}")));
                }
                None => {}
            }
        };
        for doomed in plan {
            match doomed {
                Doomed::File(path) => {
                    if in_flight.len() >= REMOVES_IN_FLIGHT {
                        let res = in_flight.join_next().await;
                        settle(res, &mut done, &mut failed);
                    }
                    if failed.is_some() {
                        break;
                    }
                    let sftp = self.clone();
                    in_flight.spawn(async move { sftp.remove_file(&path).await });
                }
                Doomed::Dir(path) => {
                    while !in_flight.is_empty() {
                        let res = in_flight.join_next().await;
                        settle(res, &mut done, &mut failed);
                    }
                    if failed.is_some() {
                        break;
                    }
                    let res = self.remove_dir(&path).await;
                    settle(Some(Ok(res)), &mut done, &mut failed);
                    if failed.is_some() {
                        break;
                    }
                }
            }
        }
        while !in_flight.is_empty() {
            let res = in_flight.join_next().await;
            settle(res, &mut done, &mut failed);
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(done),
        }
    }
}

/// `paths` without duplicates and without paths inside another one of them, in order.
fn outermost(paths: &[RemotePath]) -> Vec<RemotePath> {
    let mut out: Vec<RemotePath> = Vec::new();
    for p in paths {
        if paths.iter().any(|q| q != p && p.starts_with(q)) || out.contains(p) {
            continue;
        }
        out.push(p.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RemotePath {
        RemotePath::new(s)
    }

    #[test]
    fn paths_inside_others_and_repeats_are_dropped() {
        let got = outermost(&[p("/a/b"), p("/c"), p("/a"), p("/c"), p("/ab"), p("/a/b/c")]);
        assert_eq!(got, [p("/c"), p("/a"), p("/ab")]);
        assert_eq!(outermost(&[]), Vec::<RemotePath>::new());
    }
}
