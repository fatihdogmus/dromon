use crate::RemotePath;

/// Errors shown to the user. Each variant's message is written to be readable in the UI as is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("You don't have access to {0}")]
    PermissionDenied(RemotePath),
    #[error("{0} doesn't exist")]
    NotFound(RemotePath),
    #[error("{0} already exists")]
    AlreadyExists(RemotePath),
    #[error("The connection was lost")]
    ConnectionLost,
    #[error("Couldn't connect: {0}")]
    Connect(String),
    #[error("Sign-in was cancelled")]
    Cancelled,
    #[error("The server doesn't support {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
