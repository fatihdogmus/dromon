//! Mapping russh-sftp client errors onto the user-facing [`dromon_core::Error`].

use dromon_core::{Error, RemotePath};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;

/// The operation a request was part of: names the verb in generic messages and the feature in
/// [`Error::Unsupported`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Close,
    CopyData,
    List,
    Mkdir,
    Open,
    PosixRename,
    Read,
    Realpath,
    RemoveDir,
    RemoveFile,
    Rename,
    SetTimes,
    Stat,
    Write,
}

impl Op {
    fn verb(self) -> &'static str {
        match self {
            Op::Close => "close",
            Op::CopyData => "copy",
            Op::List => "list",
            Op::Mkdir => "create the folder",
            Op::Open => "open",
            Op::PosixRename | Op::Rename => "move",
            Op::Read => "read",
            Op::Realpath => "resolve",
            Op::RemoveDir => "delete the folder",
            Op::RemoveFile => "delete",
            Op::SetTimes => "set the modification time of",
            Op::Stat => "get information about",
            Op::Write => "write",
        }
    }

    /// The feature named in "The server doesn't support …".
    fn feature(self) -> &'static str {
        match self {
            Op::Close => "closing files",
            Op::CopyData => "copy-data",
            Op::List => "listing folders",
            Op::Mkdir => "creating folders",
            Op::Open => "opening files",
            Op::PosixRename => "posix-rename",
            Op::Read => "reading files",
            Op::Realpath => "resolving paths",
            Op::RemoveDir => "deleting folders",
            Op::RemoveFile => "deleting files",
            Op::Rename => "renaming",
            Op::SetTimes => "setting modification times",
            Op::Stat => "file information",
            Op::Write => "writing files",
        }
    }
}

/// Maps a client error from an operation on `path` to the user-facing error.
///
/// - `NoSuchFile` → [`Error::NotFound`], `PermissionDenied` → [`Error::PermissionDenied`],
///   `OpUnsupported` → [`Error::Unsupported`].
/// - A closed session, a lost connection and a request timeout → [`Error::ConnectionLost`]:
///   requests are only timed once written, so a reply missing for that long means the channel
///   is gone (or the server is wedged, which the reconnect logic treats the same way).
/// - Everything else (`Failure`, `BadMessage`, protocol errors) → [`Error::Io`] with the
///   operation, the path and the server's message.
///
/// SFTP v3 has no "already exists" status (OpenSSH sends `Failure` for `EEXIST`); callers that
/// can tell check the target themselves, see `Sftp::exists_or`.
pub(crate) fn map_err(err: SftpError, op: Op, path: &RemotePath) -> Error {
    match err {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => Error::NotFound(path.clone()),
            StatusCode::PermissionDenied => Error::PermissionDenied(path.clone()),
            StatusCode::OpUnsupported => Error::Unsupported(op.feature()),
            StatusCode::NoConnection | StatusCode::ConnectionLost => Error::ConnectionLost,
            code => {
                let msg = status.error_message.trim();
                let detail = if msg.is_empty() { code.to_string() } else { msg.to_owned() };
                Error::Io(format!("Couldn't {} {path}: {detail}", op.verb()))
            }
        },
        SftpError::Timeout | SftpError::IO(_) => Error::ConnectionLost,
        SftpError::UnexpectedBehavior(msg) if is_closed(&msg) => Error::ConnectionLost,
        SftpError::UnexpectedBehavior(msg) | SftpError::Limited(msg) => {
            Error::Io(format!("Couldn't {} {path}: {msg}", op.verb()))
        }
        SftpError::UnexpectedPacket => {
            Error::Io(format!("Couldn't {} {path}: unexpected reply from the server", op.verb()))
        }
    }
}

/// russh-sftp reports a stopped transport as `UnexpectedBehavior` with one of these messages
/// (`rawsession.rs`: "session closed", "sender dropped"; `error.rs`: channel send/recv errors).
fn is_closed(msg: &str) -> bool {
    msg == "session closed" || msg == "sender dropped" || msg.starts_with("SendError") || msg.starts_with("RecvError")
}

/// True when `err` is the generic `Failure` status, which OpenSSH also uses for `EEXIST`.
pub(crate) fn is_failure(err: &SftpError) -> bool {
    matches!(err, SftpError::Status(s) if s.status_code == StatusCode::Failure)
}

/// True when `err` is the `Eof` status that ends reads and directory listings.
pub(crate) fn is_eof(err: &SftpError) -> bool {
    matches!(err, SftpError::Status(s) if s.status_code == StatusCode::Eof)
}

#[cfg(test)]
mod tests {
    use russh_sftp::protocol::Status;

    use super::*;

    fn status(code: StatusCode, msg: &str) -> SftpError {
        SftpError::Status(Status { id: 7, status_code: code, error_message: msg.into(), language_tag: "en".into() })
    }

    #[test]
    fn status_codes_map_to_variants_with_the_path() {
        let p = RemotePath::new("/srv/a");
        assert_eq!(map_err(status(StatusCode::NoSuchFile, "No such file"), Op::Stat, &p), Error::NotFound(p.clone()));
        assert_eq!(
            map_err(status(StatusCode::PermissionDenied, "Permission denied"), Op::List, &p),
            Error::PermissionDenied(p.clone())
        );
        assert_eq!(map_err(status(StatusCode::OpUnsupported, ""), Op::CopyData, &p), Error::Unsupported("copy-data"));
        assert_eq!(map_err(status(StatusCode::ConnectionLost, ""), Op::Read, &p), Error::ConnectionLost);
    }

    #[test]
    fn failure_keeps_the_server_message() {
        let p = RemotePath::new("/srv/full");
        assert_eq!(
            map_err(status(StatusCode::Failure, "No space left on device"), Op::Write, &p),
            Error::Io("Couldn't write /srv/full: No space left on device".into())
        );
        assert_eq!(
            map_err(status(StatusCode::Failure, " "), Op::Mkdir, &p),
            Error::Io("Couldn't create the folder /srv/full: Failure".into())
        );
    }

    #[test]
    fn transport_errors_are_connection_lost() {
        let p = RemotePath::root();
        for err in [
            SftpError::Timeout,
            SftpError::IO("broken pipe".into()),
            SftpError::UnexpectedBehavior("session closed".into()),
            SftpError::UnexpectedBehavior("sender dropped".into()),
            SftpError::UnexpectedBehavior("SendError: channel closed".into()),
        ] {
            assert_eq!(map_err(err, Op::Stat, &p), Error::ConnectionLost);
        }
        assert!(matches!(map_err(SftpError::UnexpectedBehavior("bad data".into()), Op::Stat, &p), Error::Io(_)));
    }

    #[test]
    fn predicates() {
        assert!(is_failure(&status(StatusCode::Failure, "")));
        assert!(!is_failure(&status(StatusCode::Eof, "")));
        assert!(is_eof(&status(StatusCode::Eof, "")));
        assert!(!is_eof(&SftpError::Timeout));
    }
}
