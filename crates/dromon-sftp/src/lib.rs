//! The SFTP layer: a thin async wrapper over `russh-sftp` running over the ssh child's stdio,
//! the extensions Dromon uses, listing diffs and the adaptive polling schedule. See
//! `.plans/dromon-plan.md` §4.4 and §4.5.

mod copy_data;
mod diff;
mod error;
mod file;
mod poll;
mod remove;
mod session;

pub use copy_data::COPY_DATA_CHUNK;
pub use diff::{ListingDiff, diff};
pub use file::{PendingRead, RemoteFile, WriteAck, WriteMode};
pub use poll::{PollSchedule, Visibility, base_interval};
pub use remove::REMOVES_IN_FLIGHT;
pub use session::{Capabilities, DEFAULT_CHUNK, Limits, MAX_CHUNK, Sftp};
