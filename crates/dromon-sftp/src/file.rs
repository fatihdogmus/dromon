//! Open remote files: positional reads and writes that pipeline.
//!
//! The transfer engine keeps several requests in flight per file. Both [`RemoteFile::read_at`]
//! and [`RemoteFile::write_at`] put their request on the wire *before returning* and hand back
//! a future for the reply, so a caller pipelines by issuing N calls and then awaiting them in
//! order; no task per chunk is needed. Writes use russh-sftp's `write_nowait`; reads, which
//! russh-sftp only offers as an `async fn`, are polled once on the spot to queue the request.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use dromon_core::{Entry, Error, RemotePath, Result};
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::rawsession::PendingWrite;
use russh_sftp::protocol::SftpHandle;

use crate::error::{Op, is_eof, map_err};
use crate::session::{display_name, entry_from_attrs, spawn_close};

/// How [`crate::Sftp::open_write`] treats an existing file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    /// Create, or truncate an existing file to zero length.
    Truncate,
    /// Create; fail with [`Error::AlreadyExists`] if anything exists at the path.
    CreateNew,
    /// Create, keeping existing contents: write at the offset to resume from (the verified
    /// length of a part file). Not `O_APPEND`, so explicit offsets are honoured.
    Resume,
}

/// An open file handle on the server. Closed by [`RemoteFile::close`]; dropping it closes the
/// handle in the background instead (errors are then lost).
pub struct RemoteFile {
    raw: Arc<RawSftpSession>,
    handle: Option<SftpHandle>,
    path: RemotePath,
}

impl RemoteFile {
    pub(crate) fn new(raw: Arc<RawSftpSession>, handle: SftpHandle, path: RemotePath) -> Self {
        RemoteFile { raw, handle: Some(handle), path }
    }

    pub fn path(&self) -> &RemotePath {
        &self.path
    }

    pub(crate) fn handle(&self) -> &SftpHandle {
        self.handle.as_ref().expect("handle is only taken by close and drop")
    }

    /// Requests up to `len` bytes at `offset` (keep `len` ≤ [`crate::Sftp::chunk_size`]).
    /// The request is sent before this returns. The reply is the data, which may be shorter
    /// than `len`; an empty buffer means end of file.
    pub fn read_at(&self, offset: u64, len: u32) -> PendingRead {
        let raw = self.raw.clone();
        let handle = self.handle().clone();
        let path = self.path.clone();
        let fut = Box::pin(async move {
            match raw.read(handle, offset, len).await {
                Ok(data) => Ok(data.data),
                Err(e) if is_eof(&e) => Ok(Vec::new()),
                Err(e) => Err(map_err(e, Op::Read, &path)),
            }
        });
        PendingRead::start(fut)
    }

    /// Writes `data` at `offset` (keep it ≤ [`crate::Sftp::chunk_size`]). The request is sent
    /// before this returns; the returned future resolves when the server acknowledges it.
    /// Dropping the ack discards the reply, not the write.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> WriteAck {
        let state = match self.raw.write_nowait(self.handle(), offset, data) {
            Ok(pending) => AckState::Pending(pending),
            Err(e) => AckState::Failed(Some(map_err(e, Op::Write, &self.path))),
        };
        WriteAck { state, path: self.path.clone() }
    }

    /// `fstat` on the open handle.
    pub async fn stat(&self) -> Result<Entry> {
        let attrs = self.raw.fstat(self.handle().clone()).await;
        let attrs = attrs.map_err(|e| map_err(e, Op::Stat, &self.path))?;
        Ok(entry_from_attrs(display_name(&self.path), &attrs.attrs, ""))
    }

    /// Closes the handle and reports the server's answer (for writes, the last chance to see
    /// an error such as a full disk).
    pub async fn close(mut self) -> Result<()> {
        let Some(h) = self.handle.take() else { return Ok(()) };
        self.raw.close(h).await.map(drop).map_err(|e| map_err(e, Op::Close, &self.path))
    }
}

impl Drop for RemoteFile {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            spawn_close(&self.raw, h);
        }
    }
}

type ReadFuture = Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send>>;

/// The reply to [`RemoteFile::read_at`]. Dropping it discards the reply.
pub struct PendingRead {
    state: ReadState,
}

enum ReadState {
    Pending(ReadFuture),
    Ready(Option<Result<Vec<u8>>>),
}

impl PendingRead {
    /// Polls `fut` once with a no-op waker: russh-sftp's `read` queues its packet on the first
    /// poll, so this sends the request now. Later polls register the caller's waker, which
    /// every future inside (tokio oneshot and timers) honours.
    fn start(mut fut: ReadFuture) -> PendingRead {
        let mut cx = Context::from_waker(Waker::noop());
        let state = match fut.as_mut().poll(&mut cx) {
            Poll::Ready(res) => ReadState::Ready(Some(res)),
            Poll::Pending => ReadState::Pending(fut),
        };
        PendingRead { state }
    }
}

impl Future for PendingRead {
    type Output = Result<Vec<u8>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.get_mut().state {
            ReadState::Pending(fut) => fut.as_mut().poll(cx),
            ReadState::Ready(res) => {
                Poll::Ready(res.take().unwrap_or_else(|| Err(Error::Other("read polled twice".into()))))
            }
        }
    }
}

/// The acknowledgement of [`RemoteFile::write_at`].
pub struct WriteAck {
    state: AckState,
    path: RemotePath,
}

enum AckState {
    Pending(PendingWrite),
    Failed(Option<Error>),
}

impl Future for WriteAck {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match &mut this.state {
            AckState::Pending(pending) => {
                Pin::new(pending).poll(cx).map(|res| res.map(drop).map_err(|e| map_err(e, Op::Write, &this.path)))
            }
            AckState::Failed(err) => Poll::Ready(Err(err.take().unwrap_or(Error::ConnectionLost))),
        }
    }
}
