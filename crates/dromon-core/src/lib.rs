//! Shared types for Dromon. No I/O, no async, no UI: everything here is plain data and pure logic
//! that the SSH, SFTP, transfer and app crates agree on.

mod entry;
mod error;
mod event;
mod host;
mod natural;
mod path;

pub use entry::{Entry, EntryKind, LinkTarget, SortDir, SortKey, merge_sorted, sort_entries};
pub use error::{Error, Result};
pub use event::{ConnStatus, Prompt, PromptAnswer, PromptId, PromptKind};
pub use host::{HostId, HostSource, HostSpec};
pub use natural::natural_cmp;
pub use path::RemotePath;
