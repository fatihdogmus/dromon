//! Askpass: ssh runs our own executable as `SSH_ASKPASS`; that helper process forwards the prompt
//! to the app over a local socket and prints the answer.
//!
//! - [`protocol`]: one JSON line each way, platform-neutral.
//! - [`server`]: the app side. Unix domain socket in a 0700 per-user folder; on Windows a named
//!   pipe `\\.\pipe\dromon-askpass-<pid>-<random>` whose DACL admits only the current user.
//! - [`helper`]: the helper side, plain `std` so it starts fast.
//! - `win32`: the Win32 calls behind the pipe's DACL and client check (the crate's only `unsafe`).

pub mod helper;
pub mod protocol;
pub mod server;
#[cfg(windows)]
mod win32;

/// Set on the ssh child: the socket path (or pipe name) to connect to.
pub const ENDPOINT_ENV: &str = "DROMON_ASKPASS_ENDPOINT";
/// Set on the ssh child: the per-connection token, 32 random bytes as hex.
pub const TOKEN_ENV: &str = "DROMON_ASKPASS_TOKEN";
/// Set by ssh for the helper: `confirm`, `none`, or unset. See `classify_prompt`.
pub const HINT_ENV: &str = "SSH_ASKPASS_PROMPT";

#[cfg(test)]
mod protocol_tests;
