//! Remote-to-remote copies of one [`FilePlan::Copy`]: into the part file `.<name>.dromon-part`
//! next to the destination, then renamed into place (as uploads, [`super::upload`]), with the
//! source's mtime.
//!
//! - Same host ([`copy_remote`]): the server copies with `copy-data` when it offers it
//!   ([`CopyOptions::server_copy`]); such a copy always starts over (it is fast). Otherwise, and
//!   between hosts ([`copy_between`]), the file is streamed through the app: reads on the
//!   source's session and writes on the destination's are both pipelined
//!   ([`CopyOptions::in_flight`] each), replies written strictly in offset order, so the part
//!   is always a prefix of the source and a later call resumes from it (when the source still
//!   has the planned size and mtime).
//! - Moving between hosts ([`move_between`]): the copy, then [`delete_verified`]: the source is
//!   deleted only when the destination has the copied size and the source is unchanged.

use std::collections::VecDeque;

use dromon_core::{EntryKind, Error, RemotePath, Result};
use dromon_sftp::{COPY_DATA_CHUNK, PendingRead, RemoteFile, Sftp, WriteAck, WriteMode};

use super::upload::{finish, part_path};
use super::{CopyOptions, write_denied};
use crate::FilePlan;

/// What a finished copy put where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copied {
    /// Where the file landed: the plan's `dst`, or (without `overwrite`) a free name beside it
    /// when something appeared there after the scan.
    pub dst: RemotePath,
    /// Bytes copied.
    pub size: u64,
    /// The source's mtime when the copy opened it (also set on `dst`).
    pub mtime: Option<i64>,
}

/// The fields of a [`FilePlan::Copy`].
struct Plan<'a> {
    src: &'a RemotePath,
    dst: &'a RemotePath,
    part: RemotePath,
    size: u64,
    mtime: Option<i64>,
    overwrite: bool,
    name: String,
}

impl<'a> Plan<'a> {
    fn of(plan: &'a FilePlan, caller: &str) -> Result<Plan<'a>> {
        let FilePlan::Copy { src, dst, size, mtime, overwrite } = plan else {
            return Err(Error::Other(format!("{caller}() was given another kind of plan for {}", plan.name())));
        };
        let part = part_path(dst).ok_or_else(|| Error::Other(format!("Can't copy to {dst}")))?;
        Ok(Plan { src, dst, part, size: *size, mtime: *mtime, overwrite: *overwrite, name: plan.name() })
    }
}

/// The error for a source that shrank under a running copy.
fn changed_while_copying(name: &str) -> Error {
    Error::Io(format!("{name} changed on the server while copying"))
}

/// Copies one [`FilePlan::Copy`] within `sftp`'s host: with `copy-data` when the server offers
/// it and `opts.server_copy` allows, else streamed (and resumed) as [`copy_between`]. `progress`
/// gets the absolute bytes of this file done. Cancel-safe: see the module docs of [`crate::io`].
pub async fn copy_remote(
    sftp: &Sftp,
    plan: &FilePlan,
    opts: CopyOptions,
    mut progress: impl FnMut(u64) + Send,
) -> Result<Copied> {
    let p = Plan::of(plan, "copy_remote")?;
    if opts.server_copy && sftp.capabilities().copy_data {
        match server_side(sftp, &p, &mut progress).await {
            Err(Error::Unsupported(_)) => {}
            res => return res,
        }
    }
    stream(sftp, sftp, &p, opts, &mut progress).await
}

/// Copies one [`FilePlan::Copy`] from `from`'s host to `to`'s, streamed through the app;
/// resumes from the part file on `to` when the source still has the planned size and mtime.
/// `progress` gets the absolute bytes of this file the destination acknowledged (a resumed
/// file starts at its offset). Cancel-safe: see the module docs of [`crate::io`].
pub async fn copy_between(
    from: &Sftp,
    to: &Sftp,
    plan: &FilePlan,
    opts: CopyOptions,
    mut progress: impl FnMut(u64) + Send,
) -> Result<Copied> {
    let p = Plan::of(plan, "copy_between")?;
    stream(from, to, &p, opts, &mut progress).await
}

/// [`copy_between`], then [`delete_verified`]: the source goes only once its copy is in place
/// and checked. Dropped before that, the source is still there.
pub async fn move_between(
    from: &Sftp,
    to: &Sftp,
    plan: &FilePlan,
    opts: CopyOptions,
    progress: impl FnMut(u64) + Send,
) -> Result<Copied> {
    let p = Plan::of(plan, "move_between")?;
    if let Some(done) = landed_before(from, to, &p).await? {
        return Ok(done);
    }
    let copied = copy_between(from, to, plan, opts, progress).await?;
    delete_verified(from, to, p.src, &copied).await?;
    Ok(copied)
}

/// What an earlier run of this move already did, when a drop lost its result (the engine
/// aborts a running file on `HostDown`, even one that just finished): the copy is at `dst`
/// and the source is gone (done), or still there (deleted now, verified as usual). Only for a
/// destination the job named itself (no `overwrite`: nothing was there when it was named), and
/// only when the file there has exactly the planned size and mtime (copies keep the source's
/// mtime). Anything else: `None`, and the move runs as usual.
async fn landed_before(from: &Sftp, to: &Sftp, p: &Plan<'_>) -> Result<Option<Copied>> {
    if p.overwrite || p.mtime.is_none() {
        return Ok(None);
    }
    let there = match to.stat(p.dst).await {
        Ok(e) => e,
        Err(Error::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    if there.kind != EntryKind::File || there.size != p.size || there.mtime != p.mtime {
        return Ok(None);
    }
    let copied = Copied { dst: p.dst.clone(), size: p.size, mtime: p.mtime };
    match from.stat(p.src).await {
        Err(Error::NotFound(_)) => Ok(Some(copied)),
        Ok(_) => {
            delete_verified(from, to, p.src, &copied).await?;
            Ok(Some(copied))
        }
        Err(e) => Err(e),
    }
}

/// Deletes `src` on `from` if `copied.dst` on `to` has the size that was copied and `src` still
/// has that size and mtime (it didn't change after the copy read it). Otherwise the source
/// stays and the error says why.
pub async fn delete_verified(from: &Sftp, to: &Sftp, src: &RemotePath, copied: &Copied) -> Result<()> {
    let name = src.file_name().unwrap_or("/");
    let landed = to.stat(&copied.dst).await?;
    if landed.kind != EntryKind::File || landed.size != copied.size {
        return Err(Error::Io(format!(
            "The copy of {name} has {} bytes instead of {}; the original was kept",
            landed.size, copied.size
        )));
    }
    let now = from.stat(src).await?;
    if now.size != copied.size || now.mtime != copied.mtime {
        return Err(Error::Io(format!("{name} changed on the server while moving; the original was kept")));
    }
    from.remove_file(src).await
}

/// The server copies `src` into the part with `copy-data`, in [`COPY_DATA_CHUNK`]s.
async fn server_side(sftp: &Sftp, p: &Plan<'_>, progress: &mut (impl FnMut(u64) + Send)) -> Result<Copied> {
    let reader = sftp.open_read(p.src).await?;
    let now = reader.stat().await?;
    let writer = sftp.open_write(&p.part, WriteMode::Truncate).await.map_err(write_denied(p.dst))?;
    let mut done = 0;
    while done < now.size {
        let len = COPY_DATA_CHUNK.min(now.size - done);
        sftp.copy_range(&reader, done, len, &writer, done).await?;
        done += len;
        progress(done);
    }
    let written = writer.stat().await?.size;
    writer.close().await?;
    drop(reader); // closes the handle in the background: nothing to learn from its reply
    if written != now.size {
        return Err(changed_while_copying(&p.name));
    }
    if let Some(t) = now.mtime {
        sftp.set_mtime(&p.part, t).await.map_err(write_denied(p.dst))?;
    }
    let dst = finish(sftp, &p.part, p.dst, p.overwrite).await?;
    Ok(Copied { dst, size: now.size, mtime: now.mtime })
}

/// Streams `src` on `from` into the part on `to` (the same session for a same-host fallback).
async fn stream(
    from: &Sftp,
    to: &Sftp,
    p: &Plan<'_>,
    opts: CopyOptions,
    progress: &mut (impl FnMut(u64) + Send),
) -> Result<Copied> {
    let reader = from.open_read(p.src).await?;
    // The file as it is now: a source that changed since the scan is copied afresh; only a
    // change while this copy runs fails it.
    let now = reader.stat().await?;
    let offset = if now.size == p.size && now.mtime == p.mtime {
        match to.lstat(&p.part).await {
            Ok(e) if e.kind == EntryKind::File && e.size <= now.size => e.size,
            Ok(_) | Err(Error::NotFound(_)) => 0,
            Err(e) => return Err(e),
        }
    } else {
        0
    };
    let mode = if offset > 0 { WriteMode::Resume } else { WriteMode::Truncate };
    let writer = to.open_write(&p.part, mode).await.map_err(write_denied(p.dst))?;
    if offset > 0 {
        progress(offset);
    }
    let pump = Pump {
        reader: &reader,
        writer: &writer,
        chunk: u64::from(from.chunk_size().min(to.chunk_size())),
        in_flight: opts.in_flight.max(1),
        size: now.size,
        name: &p.name,
    };
    let copied = pump.run(offset, progress).await?;
    drop(reader);
    writer.close().await?;
    if let Some(t) = now.mtime {
        to.set_mtime(&p.part, t).await.map_err(write_denied(p.dst))?;
    }
    let dst = finish(to, &p.part, p.dst, p.overwrite).await?;
    Ok(Copied { dst, size: copied, mtime: now.mtime })
}

/// The read and write pipelines of one streamed copy.
struct Pump<'a> {
    reader: &'a RemoteFile,
    writer: &'a RemoteFile,
    chunk: u64,
    in_flight: usize,
    /// The source's size when opened: end of file before it means the file shrank.
    size: u64,
    name: &'a str,
}

/// A read on the wire.
struct Request {
    offset: u64,
    len: u64,
    reply: PendingRead,
}

impl Pump<'_> {
    /// Copies the source from `start` to end of file at the same offsets; returns the bytes in
    /// the part once every write is acknowledged.
    async fn run(&self, start: u64, progress: &mut (impl FnMut(u64) + Send)) -> Result<u64> {
        let mut reads: VecDeque<Request> = VecDeque::with_capacity(self.in_flight + 1);
        let mut writes: VecDeque<(u64, WriteAck)> = VecDeque::with_capacity(self.in_flight);
        // Next offset to request; where end of file is expected (u64::MAX once the file grew);
        // whether the request past that end is out.
        let (mut next, mut end, mut probing) = (start, self.size, false);
        let mut pos = start;
        'read: loop {
            while reads.len() < self.in_flight {
                let len = if next < end {
                    (end - next).min(self.chunk)
                } else if !probing {
                    probing = true;
                    self.chunk
                } else {
                    break;
                };
                reads.push_back(Request { offset: next, len, reply: self.reader.read_at(next, len as u32) });
                next += len;
            }
            let Some(req) = reads.pop_front() else { break };
            debug_assert_eq!(req.offset, pos, "replies are written in order");
            let probe = req.offset >= end;
            let (mut want, mut reply) = (req.len, req.reply);
            loop {
                let data = reply.await?;
                if data.is_empty() {
                    // End of file: anything still queued lies past it.
                    if pos < self.size {
                        return Err(changed_while_copying(self.name));
                    }
                    break 'read;
                }
                let n = data.len().min(want as usize);
                while writes.len() >= self.in_flight {
                    let Some((upto, ack)) = writes.pop_front() else { break };
                    ack.await?;
                    progress(upto);
                }
                writes.push_back((pos + n as u64, self.writer.write_at(pos, &data[..n])));
                pos += n as u64;
                want -= n as u64;
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
        while let Some((upto, ack)) = writes.pop_front() {
            ack.await?;
            progress(upto);
        }
        Ok(pos)
    }
}
