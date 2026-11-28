//! The askpass wire format: one JSON line each way, then the server closes.
//!
//! - Request (helper → app): `{"token":"…","prompt":"…","hint":"…"|null}`
//! - Reply (app → helper): `{"answer":"…"}` or `{"cancel":true}`
//!
//! Each line is at most [`MAX_MESSAGE_BYTES`] bytes, newline excluded. Error messages never quote
//! message content, since a reply carries a secret.

use std::fmt;
use std::io::{self, BufRead, Read};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};
use zeroize::Zeroizing;

/// The largest message either side accepts.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// What the helper sends.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub token: String,
    pub prompt: String,
    pub hint: Option<String>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("token", &"<redacted>")
            .field("prompt", &self.prompt)
            .field("hint", &self.hint)
            .finish()
    }
}

/// What the app answers.
#[derive(Clone, PartialEq, Eq)]
pub enum Reply {
    Answer(Zeroizing<String>),
    Cancel,
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reply::Answer(_) => f.write_str("Reply::Answer(<redacted>)"),
            Reply::Cancel => f.write_str("Reply::Cancel"),
        }
    }
}

#[derive(Serialize)]
struct ReplyOut<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cancel: Option<bool>,
}

#[derive(Deserialize)]
struct ReplyIn {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    cancel: bool,
}

/// `request` as one JSON line, newline included.
pub fn encode_request(request: &Request) -> io::Result<Vec<u8>> {
    let mut buf = serde_json::to_vec(request).map_err(|_| invalid("couldn't encode askpass request"))?;
    check_size(buf.len())?;
    buf.push(b'\n');
    Ok(buf)
}

/// Parses one request line (newline optional).
pub fn decode_request(line: &[u8]) -> io::Result<Request> {
    check_size(without_newline(line).len())?;
    serde_json::from_slice(line).map_err(malformed)
}

/// `reply` as one JSON line, newline included, in a buffer that is wiped on drop.
pub fn encode_reply(reply: &Reply) -> io::Result<Zeroizing<Vec<u8>>> {
    let out = match reply {
        Reply::Answer(answer) => ReplyOut { answer: Some(answer), cancel: None },
        Reply::Cancel => ReplyOut { answer: None, cancel: Some(true) },
    };
    let answer_len = match reply {
        Reply::Answer(a) => a.len(),
        Reply::Cancel => 0,
    };
    // Room for worst-case escaping (`\u00XX` per byte), so the buffer never reallocates and
    // leaves unwiped copies behind.
    let mut buf = Zeroizing::new(Vec::with_capacity(answer_len * 6 + 32));
    serde_json::to_writer(&mut *buf, &out).map_err(|_| invalid("couldn't encode askpass reply"))?;
    check_size(buf.len())?;
    buf.push(b'\n');
    Ok(buf)
}

/// Parses one reply line. `{"cancel":true}` wins over any answer, so a confused peer fails
/// closed.
pub fn decode_reply(line: &[u8]) -> io::Result<Reply> {
    check_size(without_newline(line).len())?;
    let wire: ReplyIn = serde_json::from_slice(line).map_err(malformed)?;
    match (wire.cancel, wire.answer) {
        (true, answer) => {
            drop(answer.map(Zeroizing::new));
            Ok(Reply::Cancel)
        }
        (false, Some(answer)) => Ok(Reply::Answer(Zeroizing::new(answer))),
        (false, None) => Err(invalid("askpass reply has neither an answer nor a cancel")),
    }
}

/// Reads one line of at most [`MAX_MESSAGE_BYTES`] (newline stripped). A line ended by EOF
/// instead of a newline is accepted; EOF before any byte is `UnexpectedEof`.
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut buf = Zeroizing::new(Vec::with_capacity(4096));
    reader.by_ref().take(MAX_MESSAGE_BYTES as u64 + 1).read_until(b'\n', &mut buf)?;
    finish_line(buf)
}

/// [`read_message`] for async readers.
pub async fn read_message_async<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut buf = Zeroizing::new(Vec::with_capacity(4096));
    (&mut *reader).take(MAX_MESSAGE_BYTES as u64 + 1).read_until(b'\n', &mut buf).await?;
    finish_line(buf)
}

fn finish_line(mut buf: Zeroizing<Vec<u8>>) -> io::Result<Zeroizing<Vec<u8>>> {
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "askpass peer closed the connection"));
    }
    check_size(buf.len())?;
    Ok(buf)
}

/// `line` without its one optional newline: the limit is on the message.
fn without_newline(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\n").unwrap_or(line)
}

fn check_size(len: usize) -> io::Result<()> {
    if len > MAX_MESSAGE_BYTES {
        return Err(invalid("askpass message larger than 64 KiB"));
    }
    Ok(())
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// serde_json's `Display` can quote the offending value, so report only where it went wrong.
fn malformed(e: serde_json::Error) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("malformed askpass message ({:?} error at column {})", e.classify(), e.column()),
    )
}

#[cfg(test)]
#[path = "protocol_props.rs"]
mod props;
