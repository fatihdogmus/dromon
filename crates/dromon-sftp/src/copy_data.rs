//! Same-host copies: server-side with the `copy-data` extension (spike S4), or streamed
//! through the app when the server doesn't advertise it.
//!
//! russh-sftp doesn't implement `copy-data`, but its [`RawSftpSession::extended`] sends an
//! `SSH_FXP_EXTENDED` request with any payload and returns the reply packet, so we encode the
//! payload ourselves (see [`encode_copy_data`]) on handles opened through the normal API.
//!
//! [`RawSftpSession::extended`]: russh_sftp::client::RawSftpSession::extended

use std::collections::VecDeque;

use bytes::BufMut;
use dromon_core::{Error, RemotePath, Result};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{Packet, StatusCode};

use crate::error::{Op, map_err};
use crate::file::{PendingRead, RemoteFile, WriteAck, WriteMode};
use crate::session::{COPY_DATA, Sftp};

/// Bytes per `copy-data` request. The copy runs in OpenSSH's single-threaded sftp-server, so
/// requests are bounded to keep each well inside the reply timeout (10 s by default) and to
/// let other requests on the session (polls, listings) interleave between chunks.
pub const COPY_DATA_CHUNK: u64 = 32 * 1024 * 1024;

/// Reads (and, separately, writes) kept in flight by [`Sftp::copy_streaming`].
const STREAM_WINDOW: usize = 16;

/// The `copy-data` request payload (draft-ietf-secsh-filexfer-extensions-00 §7, as OpenSSH's
/// sftp-server reads it): `string read-from-handle, uint64 read-from-offset,
/// uint64 read-data-length (0 = to end of file), string write-to-handle,
/// uint64 write-to-offset`.
pub(crate) fn encode_copy_data(
    read_handle: &[u8],
    read_offset: u64,
    len: u64,
    write_handle: &[u8],
    write_offset: u64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + read_handle.len() + write_handle.len() + 24);
    buf.put_u32(read_handle.len() as u32);
    buf.put_slice(read_handle);
    buf.put_u64(read_offset);
    buf.put_u64(len);
    buf.put_u32(write_handle.len() as u32);
    buf.put_slice(write_handle);
    buf.put_u64(write_offset);
    buf
}

impl Sftp {
    /// Copies `src` to `dst` on the server with `copy-data`, creating or truncating `dst`.
    /// [`Error::Unsupported`]`("copy-data")` when the server doesn't advertise it (then use
    /// [`Sftp::copy_streaming`]). Mtime is not preserved; a failed copy leaves a partial `dst`.
    pub async fn copy_data(&self, src: &RemotePath, dst: &RemotePath) -> Result<()> {
        self.copy_data_with_progress(src, dst, |_| {}).await
    }

    /// [`Sftp::copy_data`], calling `progress` with the bytes copied so far after every
    /// [`COPY_DATA_CHUNK`].
    pub async fn copy_data_with_progress(
        &self,
        src: &RemotePath,
        dst: &RemotePath,
        mut progress: impl FnMut(u64) + Send,
    ) -> Result<()> {
        if !self.caps().copy_data {
            return Err(Error::Unsupported("copy-data"));
        }
        let reader = self.open_read(src).await?;
        let size = reader.stat().await?.size;
        let writer = self.open_write(dst, WriteMode::Truncate).await?;
        let mut done = 0;
        while done < size {
            let len = COPY_DATA_CHUNK.min(size - done);
            self.copy_range(&reader, done, len, &writer, done).await?;
            done += len;
            progress(done);
        }
        writer.close().await?;
        reader.close().await
    }

    /// One `copy-data` request: `len` bytes (0 = to end of file) from `reader` at `read_offset`
    /// to `writer` at `write_offset`.
    pub async fn copy_range(
        &self,
        reader: &RemoteFile,
        read_offset: u64,
        len: u64,
        writer: &RemoteFile,
        write_offset: u64,
    ) -> Result<()> {
        if !self.caps().copy_data {
            return Err(Error::Unsupported("copy-data"));
        }
        let payload = encode_copy_data(
            reader.handle().as_bytes(),
            read_offset,
            len,
            writer.handle().as_bytes(),
            write_offset,
        );
        let dst = writer.path();
        match self.raw.extended(COPY_DATA, payload).await {
            Ok(Packet::Status(s)) if s.status_code == StatusCode::Ok => Ok(()),
            Ok(Packet::Status(s)) => Err(map_err(SftpError::Status(s), Op::CopyData, dst)),
            Ok(_) => Err(map_err(SftpError::UnexpectedPacket, Op::CopyData, dst)),
            Err(e) => Err(map_err(e, Op::CopyData, dst)),
        }
    }

    /// Copies `src` to `dst` through the app (download and upload over the same session),
    /// creating or truncating `dst`. The fallback when `copy-data` isn't available.
    pub async fn copy_streaming(&self, src: &RemotePath, dst: &RemotePath) -> Result<()> {
        self.copy_streaming_with_progress(src, dst, |_| {}).await
    }

    /// [`Sftp::copy_streaming`], calling `progress` with the bytes written (acknowledged)
    /// so far.
    pub async fn copy_streaming_with_progress(
        &self,
        src: &RemotePath,
        dst: &RemotePath,
        mut progress: impl FnMut(u64) + Send,
    ) -> Result<()> {
        let chunk = self.chunk_size();
        let reader = self.open_read(src).await?;
        let writer = self.open_write(dst, WriteMode::Truncate).await?;

        let mut reads: VecDeque<(u64, PendingRead)> = VecDeque::new();
        let mut writes: VecDeque<(u64, WriteAck)> = VecDeque::new();
        let mut next = 0;
        let mut eof = false;
        let mut written = 0;
        loop {
            while !eof && reads.len() < STREAM_WINDOW {
                reads.push_back((next, reader.read_at(next, chunk)));
                next += u64::from(chunk);
            }
            let Some((offset, read)) = reads.pop_front() else { break };
            let mut data = read.await?;
            if data.is_empty() {
                // End of file: everything still queued lies past it.
                eof = true;
                reads.clear();
                continue;
            }
            // A short read before the end leaves a gap; fill it before moving on.
            while data.len() < chunk as usize {
                let more = reader.read_at(offset + data.len() as u64, chunk - data.len() as u32).await?;
                if more.is_empty() {
                    eof = true;
                    reads.clear();
                    break;
                }
                data.extend_from_slice(&more);
            }
            writes.push_back((data.len() as u64, writer.write_at(offset, &data)));
            while writes.len() > STREAM_WINDOW {
                written += ack(&mut writes).await?;
                progress(written);
            }
        }
        while !writes.is_empty() {
            written += ack(&mut writes).await?;
            progress(written);
        }
        writer.close().await?;
        reader.close().await
    }
}

/// Awaits the oldest write; returns its length.
async fn ack(writes: &mut VecDeque<(u64, WriteAck)>) -> Result<u64> {
    let (len, ack) = writes.pop_front().expect("caller checks for a pending write");
    ack.await?;
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_data_payload_layout() {
        let payload = encode_copy_data(b"\x00\x01", 0x0102_0304_0506_0708, 0, b"wh", 9);
        let expected: Vec<u8> = [
            &[0, 0, 0, 2][..],
            b"\x00\x01",
            &[1, 2, 3, 4, 5, 6, 7, 8],
            &[0; 8],
            &[0, 0, 0, 2],
            b"wh",
            &[0, 0, 0, 0, 0, 0, 0, 9],
        ]
        .concat();
        assert_eq!(payload, expected);
    }

    #[test]
    fn copy_data_payload_with_empty_handles() {
        let payload = encode_copy_data(b"", 1, 2, b"", 3);
        assert_eq!(payload.len(), 4 + 8 + 8 + 4 + 8);
        assert_eq!(&payload[12..20], &2u64.to_be_bytes());
    }
}
