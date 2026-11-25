//! Transfers: jobs, per-host scheduling, atomic part files, resume, conflict naming and progress
//! maths. See `.plans/dromon-plan.md` §4.6.

mod format;
mod naming;
mod progress;
mod state;

pub use format::bytes as format_bytes;
pub use format::eta as format_eta;
pub use format::progress_line;
pub use format::speed as format_speed;
pub use naming::download_part_name;
pub use naming::is_part_file;
pub use naming::unique_name;
pub use naming::upload_part_name;
pub use progress::Progress;
pub use progress::SpeedMeter;
pub use progress::aggregate;
pub use progress::eta;
pub use state::Direction;
pub use state::JobKind;
pub use state::TaskEvent;
pub use state::TaskState;
pub use state::step;
