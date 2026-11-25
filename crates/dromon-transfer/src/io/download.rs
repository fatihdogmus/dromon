//! Downloads: one remote file into the local part file `<name>.dromon-part`, then renamed into
//! place.
//!
//! Reads are pipelined: [`CopyOptions::in_flight`] requests of [`Sftp::chunk_size`] stay
//! outstanding, and replies are appended to the part file strictly in offset order (a short reply
//! re-requests the rest before moving on), so the part's length is always a verified prefix of the
//! source. One extra request just past the expected end finds end of file without another round
//! trip; if it returns data the file grew, and the download reads on to the real end.

use std::collections::VecDeque;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use dromon_core::{Error, Result};
use dromon_sftp::{PendingRead, RemoteFile, Sftp};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::{CopyOptions, changed_on_server, local_err, local_name_taken, os_message, time_of};
use crate::{FilePlan, download_part_name, unique_name};

/// The part file next to `dst`: `<name>.dromon-part`. `None` when `dst` has no file name.
pub(super) fn part_path(dst: &Path) -> Option<PathBuf> {
    let name = dst.file_name()?;
    Some(dst.with_file_name(download_part_name(&name.to_string_lossy())))
}

/// Copies one [`FilePlan::Download`] and returns the final local path: `plan.dst`, or a free
/// `name (1).ext` beside it when something appeared at `dst` after the scan.
///
/// Resumes from the part file when it is no longer than the source and the source still has
/// the planned size and mtime; otherwise starts over. `progress` gets the absolute bytes of this
/// file done (a resumed file starts at its offset) after each chunk written. Cancel-safe: see
/// the module docs of [`crate::io`].
pub async fn download(
    sftp: &Sftp,
    plan: &FilePlan,
    opts: CopyOptions,
    mut progress: impl FnMut(u64) + Send,
) -> Result<PathBuf> {
    let FilePlan::Download { src, dst, size, mtime } = plan else {
        return Err(Error::Other(format!("download() was given an upload plan for {}", plan.name())));
    };
    let part = part_path(dst).ok_or_else(|| Error::Other(format!("{} has no file name", dst.display())))?;
    let name = plan.name();

    let reader = sftp.open_read(src).await?;
    let now = reader.stat().await?;
    if now.size < *size {
        return Err(changed_on_server(&name));
    }
    let unchanged = now.size == *size && now.mtime == *mtime;
    let offset = match tokio::fs::metadata(&part).await {
        Ok(m) if unchanged && m.is_file() && m.len() <= now.size => m.len(),
        _ => 0,
    };

    let mut file = open_part(&part, offset, dst).await?;
    if offset > 0 {
        progress(offset);
    }
    let fetch = Fetch {
        reader: &reader,
        chunk: u64::from(sftp.chunk_size()),
        in_flight: opts.in_flight.max(1),
        size: now.size,
        name: &name,
        dst,
    };
    fetch.run(&mut file, offset, &mut progress).await?;
    drop(reader); // closes the handle in the background: nothing to learn from its reply
    file.flush().await.map_err(|e| local_err("write", dst, &e))?;
    let file = file.into_std().await;

    let (part, dst, mtime) = (part, dst.clone(), now.mtime.or(*mtime));
    tokio::task::spawn_blocking(move || finish(file, &part, &dst, mtime))
        .await
        .map_err(|e| Error::Other(format!("Finishing {name} stopped: {e}")))?
}

/// Opens the part file: at `offset` to resume, or truncated to start over.
async fn open_part(part: &Path, offset: u64, dst: &Path) -> Result<File> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(offset == 0)
        .open(part)
        .await
        .map_err(|e| local_err("write", dst, &e))?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset)).await.map_err(|e| local_err("write", dst, &e))?;
    }
    Ok(file)
}

/// The read pipeline of one download.
struct Fetch<'a> {
    reader: &'a RemoteFile,
    chunk: u64,
    in_flight: usize,
    /// The source's size when opened: end of file before it means the file shrank.
    size: u64,
    name: &'a str,
    dst: &'a Path,
}

/// A read on the wire.
struct Request {
    offset: u64,
    len: u64,
    reply: PendingRead,
}

impl Fetch<'_> {
    /// Appends the source from `start` to end of file to `file`; returns the bytes in the part.
    async fn run(&self, file: &mut File, start: u64, progress: &mut (impl FnMut(u64) + Send)) -> Result<u64> {
        let mut queue: VecDeque<Request> = VecDeque::with_capacity(self.in_flight + 1);
        // Next offset to request; where end of file is expected (u64::MAX once the file grew);
        // whether the request past that end is out.
        let (mut next, mut end, mut probing) = (start, self.size, false);
        let mut pos = start;
        loop {
            while queue.len() < self.in_flight {
                let len = if next < end {
                    (end - next).min(self.chunk)
                } else if !probing {
                    probing = true;
                    self.chunk
                } else {
                    break;
                };
                queue.push_back(Request { offset: next, len, reply: self.reader.read_at(next, len as u32) });
                next += len;
            }
            let Some(req) = queue.pop_front() else { return Ok(pos) };
            debug_assert_eq!(req.offset, pos, "replies are written in order");
            let probe = req.offset >= end;
            let (mut want, mut reply) = (req.len, req.reply);
            loop {
                let data = reply.await?;
                if data.is_empty() {
                    // End of file: anything still queued lies past it.
                    return if pos < self.size { Err(changed_on_server(self.name)) } else { Ok(pos) };
                }
                let n = data.len().min(want as usize);
                file.write_all(&data[..n]).await.map_err(|e| local_err("write", self.dst, &e))?;
                pos += n as u64;
                want -= n as u64;
                progress(pos);
                if want == 0 {
                    break;
                }
                // A short reply: ask for the rest of this chunk before anything later.
                reply = self.reader.read_at(pos, want as u32);
            }
            if probe {
                // The file grew since it was opened: read on until end of file.
                end = u64::MAX;
            }
        }
    }
}

/// Sets the mtime, syncs, and renames the part to `dst`, or to a free name beside it if `dst`
/// was taken meanwhile. Blocking.
fn finish(file: std::fs::File, part: &Path, dst: &Path, mtime: Option<i64>) -> Result<PathBuf> {
    if let Some(t) = mtime.and_then(time_of) {
        file.set_modified(t).map_err(|e| local_err("set the modification time of", dst, &e))?;
    }
    file.sync_all().map_err(|e| local_err("write", dst, &e))?;
    drop(file);
    let target = if dst.symlink_metadata().is_ok() {
        let dir = dst.parent().unwrap_or(Path::new(""));
        let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        dir.join(unique_name(&name, |n| local_name_taken(dir, n)))
    } else {
        dst.to_owned()
    };
    std::fs::rename(part, &target).map_err(|e| {
        Error::Io(format!("Couldn't rename {} to {}: {}", part.display(), target.display(), os_message(&e)))
    })?;
    Ok(target)
}
