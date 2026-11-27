//! Quick preview reads (`Command::Preview`): one range of one file on the host's SFTP session,
//! in a task of its own so listings, prompts and operations keep flowing.
//!
//! A read opens the file, asks its size (`fstat`), and reads only the range asked for: the head,
//! or the tail from `size − limit` ([`PreviewRange::span`]), a few requests in flight at once
//! ([`IN_FLIGHT`]), in offset order. The bytes go into memory and to the window that asked,
//! never to disk; nothing about them is logged.
//!
//! Cancelling aborts the read's task, which drops its requests in flight and closes the file
//! in the background. A lost connection found by a read is left for the session's own listings
//! and checks to notice: the read only says so.

use std::collections::{HashMap, VecDeque};

use dromon_core::{Error, HostId, RemotePath};
use dromon_sftp::Sftp;
use tokio::task::JoinHandle;

use super::Sink;
use super::conn::{ConnCmd, ConnHandle};
use crate::state::{PreviewBytes, PreviewCommand, PreviewEvent, PreviewId, PreviewRange, ServiceEvent, WindowId};

/// Read requests a preview keeps in flight at once.
pub const IN_FLIGHT: usize = 16;

/// What the manager asks of a host's preview reads.
pub enum PreviewCmd {
    Start { id: PreviewId, path: RemotePath, range: PreviewRange },
    Cancel(PreviewId),
}

/// A preview command from window `from`: the host's task reads, or stops the read; a host
/// without one isn't connected, and the read fails at once.
pub(super) fn route<S: Sink>(from: WindowId, cmd: PreviewCommand, conns: &HashMap<HostId, ConnHandle>, sink: &S) {
    let send = |host: &HostId, cmd: PreviewCmd| {
        conns.get(host).is_some_and(|c| c.cmds.send(ConnCmd::Preview(from, cmd)).is_ok())
    };
    match cmd {
        PreviewCommand::Start { id, host, path, range } => {
            if !send(&host, PreviewCmd::Start { id, path, range }) {
                let ev = PreviewEvent::Failed { id, host, error: Error::ConnectionLost };
                sink.emit_to(from, ServiceEvent::Preview(ev));
            }
        }
        PreviewCommand::Cancel { id, host } => {
            send(&host, PreviewCmd::Cancel(id));
        }
    }
}

/// The preview reads running on one connection, by id.
#[derive(Default)]
pub struct Reads {
    running: HashMap<PreviewId, JoinHandle<()>>,
}

impl Drop for Reads {
    fn drop(&mut self) {
        for (_, read) in self.running.drain() {
            read.abort();
        }
    }
}

impl Reads {
    /// Carries out `cmd` from `window`: a read on `sftp` (none: the session is down, and it
    /// fails at once), or cancelling one.
    pub fn command<S: Sink>(&mut self, window: WindowId, cmd: PreviewCmd, sftp: Option<Sftp>, host: &HostId, sink: &S) {
        self.running.retain(|_, read| !read.is_finished());
        match cmd {
            PreviewCmd::Start { id, path, range } => {
                let Some(sftp) = sftp else {
                    let ev = PreviewEvent::Failed { id, host: host.clone(), error: Error::ConnectionLost };
                    return sink.emit_to(window, ServiceEvent::Preview(ev));
                };
                let (host, sink) = (host.clone(), sink.clone());
                let read = tokio::spawn(async move {
                    let ev = match read(&sftp, &path, range).await {
                        Ok((bytes, total)) => PreviewEvent::Done { id, host, bytes: PreviewBytes(bytes.into()), total },
                        Err(error) => PreviewEvent::Failed { id, host, error },
                    };
                    sink.emit_to(window, ServiceEvent::Preview(ev));
                });
                if let Some(old) = self.running.insert(id, read) {
                    old.abort();
                }
            }
            PreviewCmd::Cancel(id) => {
                if let Some(read) = self.running.remove(&id) {
                    read.abort();
                }
            }
        }
    }
}

/// Reads `range` of the file at `path`: its bytes, and the file's size. Only the range is
/// read; a file that shrank meanwhile gives what was left of it.
pub async fn read(sftp: &Sftp, path: &RemotePath, range: PreviewRange) -> Result<(Vec<u8>, u64), Error> {
    let file = sftp.open_read(path).await?;
    let total = file.stat().await?.size;
    let (offset, len) = range.span(total);
    let end = offset + len;
    let chunk = u64::from(sftp.chunk_size().max(1));
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    let mut pending = VecDeque::new();
    let mut next = offset;
    loop {
        while pending.len() < IN_FLIGHT && next < end {
            let n = chunk.min(end - next);
            pending.push_back((n, file.read_at(next, n as u32)));
            next += n;
        }
        let Some((asked, reply)) = pending.pop_front() else { break };
        let data = reply.await?;
        if data.is_empty() {
            // The end of the file came early: it shrank.
            break;
        }
        let got = (data.len() as u64).min(asked);
        bytes.extend_from_slice(&data[..got as usize]);
        if got < asked {
            // A short reply: the requests after it would leave a gap, so ask again from here.
            pending.clear();
            next = offset + bytes.len() as u64;
        }
    }
    // The bytes are read; an error closing a read-only handle changes nothing about them.
    let _ = file.close().await;
    Ok((bytes, total))
}
