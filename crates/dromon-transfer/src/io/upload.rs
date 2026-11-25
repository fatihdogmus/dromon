//! Uploads: one local file into the remote part file `.<name>.dromon-part`, then renamed into
//! place (atomically replacing the target with `posix-rename` when the user chose Replace).
//!
//! Writes are pipelined: the local file is read in order in [`Sftp::chunk_size`] pieces and up
//! to [`CopyOptions::in_flight`] writes stay unacknowledged; the oldest is awaited when the window
//! is full. The server applies one session's requests in order, so the part's length is a prefix
//! of the source even when the upload is dropped mid-way.

use std::collections::VecDeque;
use std::io::SeekFrom;
use std::path::Path;

use dromon_core::{EntryKind, Error, RemotePath, Result};
use dromon_sftp::{RemoteFile, Sftp, WriteAck, WriteMode};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{CopyOptions, changed_locally, free_remote_name, local_err, secs_of};
use crate::{FilePlan, upload_part_name};

/// The part file next to `dst`: `.<name>.dromon-part`. `None` for the root.
pub(super) fn part_path(dst: &RemotePath) -> Option<RemotePath> {
    Some(dst.parent()?.join(&upload_part_name(dst.file_name()?)))
}

/// Copies one [`FilePlan::Upload`] and returns the final remote path: `plan.dst`, or, without
/// `overwrite`, a free `name (1).ext` beside it when something appeared at `dst` after the scan.
///
/// Resumes from the remote part file when it is no longer than the local file and the local
/// file still has the planned size and mtime; otherwise starts over. `progress` gets the
/// absolute bytes of this file the server acknowledged (a resumed file starts at its offset).
/// Cancel-safe: see the module docs of [`crate::io`].
pub async fn upload(
    sftp: &Sftp,
    plan: &FilePlan,
    opts: CopyOptions,
    mut progress: impl FnMut(u64) + Send,
) -> Result<RemotePath> {
    let FilePlan::Upload { src, dst, size, mtime, overwrite } = plan else {
        return Err(Error::Other(format!("upload() was given a download plan for {}", plan.name())));
    };
    let part = part_path(dst).ok_or_else(|| Error::Other(format!("Can't upload to {dst}")))?;
    let name = plan.name();

    let mut file = File::open(src).await.map_err(|e| local_err("read", src, &e))?;
    let meta = file.metadata().await.map_err(|e| local_err("read", src, &e))?;
    let (now_size, now_mtime) = (meta.len(), meta.modified().ok().map(secs_of));
    let offset = if now_size == *size && now_mtime == *mtime {
        match sftp.lstat(&part).await {
            Ok(e) if e.kind == EntryKind::File && e.size <= now_size => e.size,
            Ok(_) | Err(Error::NotFound(_)) => 0,
            Err(e) => return Err(e),
        }
    } else {
        0
    };

    let mode = if offset > 0 { WriteMode::Resume } else { WriteMode::Truncate };
    let remote = sftp.open_write(&part, mode).await?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset)).await.map_err(|e| local_err("read", src, &e))?;
        progress(offset);
    }
    let chunk = sftp.chunk_size() as usize;
    let sent = send(&remote, &mut file, src, offset, chunk, opts.in_flight.max(1), &mut progress).await?;
    drop(file);
    if sent < now_size {
        return Err(changed_locally(&name));
    }
    remote.close().await?;
    if let Some(t) = now_mtime {
        sftp.set_mtime(&part, t).await?;
    }
    finish(sftp, &part, dst, *overwrite).await
}

/// Writes `file` from `start` to its end at the same offsets in `remote`; returns the bytes in
/// the part once every write is acknowledged.
async fn send(
    remote: &RemoteFile,
    file: &mut File,
    src: &Path,
    start: u64,
    chunk: usize,
    in_flight: usize,
    progress: &mut (impl FnMut(u64) + Send),
) -> Result<u64> {
    let mut acks: VecDeque<(u64, WriteAck)> = VecDeque::with_capacity(in_flight);
    let mut buf = vec![0u8; chunk];
    let mut pos = start;
    loop {
        let n = read_full(file, &mut buf).await.map_err(|e| local_err("read", src, &e))?;
        if n == 0 {
            break;
        }
        while acks.len() >= in_flight {
            let Some((end, ack)) = acks.pop_front() else { break };
            ack.await?;
            progress(end);
        }
        acks.push_back((pos + n as u64, remote.write_at(pos, &buf[..n])));
        pos += n as u64;
        if n < chunk {
            break;
        }
    }
    while let Some((end, ack)) = acks.pop_front() {
        ack.await?;
        progress(end);
    }
    Ok(pos)
}

/// Fills `buf` unless end of file comes first; returns the bytes read.
async fn read_full(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]).await? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Renames the finished part into place and returns where it landed (uploads and remote
/// copies).
///
/// - `overwrite`: `posix-rename` replaces `dst` atomically; without it, `dst` is removed first.
/// - otherwise: a free name when `dst` is taken now; if something appears at `dst` between that
///   check and the rename, a free name is picked once more.
pub(super) async fn finish(sftp: &Sftp, part: &RemotePath, dst: &RemotePath, overwrite: bool) -> Result<RemotePath> {
    let (Some(folder), Some(name)) = (dst.parent(), dst.file_name()) else {
        return Err(Error::Other(format!("Can't upload to {dst}")));
    };
    if overwrite {
        match sftp.posix_rename(part, dst).await {
            Err(Error::Unsupported(_)) => {
                match sftp.remove_file(dst).await {
                    Ok(()) | Err(Error::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
                sftp.rename(part, dst).await?;
            }
            res => res?,
        }
        return Ok(dst.clone());
    }
    let target = if sftp.exists(dst).await? {
        folder.join(&free_remote_name(sftp, &folder, name).await?)
    } else {
        dst.clone()
    };
    match sftp.rename(part, &target).await {
        Ok(()) => Ok(target),
        Err(Error::AlreadyExists(_)) => {
            let again = folder.join(&free_remote_name(sftp, &folder, name).await?);
            sftp.rename(part, &again).await?;
            Ok(again)
        }
        Err(e) => Err(e),
    }
}
