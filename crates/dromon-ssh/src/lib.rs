//! The SSH layer: discovering hosts from ssh config, launching the system `ssh` with the SFTP
//! subsystem, answering its prompts through the askpass helper, and remembering passwords in
//! memory only. See `.plans/dromon-plan.md` §4.3.

pub mod askpass;
mod auth;
mod backoff;
mod command;
mod config;
mod passwords;
mod process;
mod prompt;
mod resolve;

pub use askpass::helper::ask;
pub use askpass::helper::is_helper_invocation;
pub use askpass::helper::run_helper;
pub use askpass::helper::run_helper_with;
pub use askpass::server::AskpassRequest;
pub use askpass::server::AskpassServer;
pub use auth::AuthAttempt;
pub use auth::AuthDecision;
pub use auth::WRONG_PASSPHRASE;
pub use auth::WRONG_PASSWORD;
pub use backoff::Backoff;
pub use command::AskpassEnv;
pub use command::BASE_ARGS;
pub use command::CommandError;
pub use command::FALLBACK_DISPLAY;
pub use command::SshCommand;
pub use command::ssh_program;
pub use config::ConfigFs;
pub use config::ConfigSources;
pub use config::Discovery;
pub use config::MAX_INCLUDE_DEPTH;
pub use config::ParseContext;
pub use config::RealFs;
pub use config::discover_aliases;
pub use config::discover_aliases_with;
pub use config::parse_host_aliases;
pub use passwords::PasswordStore;
pub use process::STDERR_BUFFER_LINES;
pub use process::SshChild;
pub use process::SshStream;
pub use process::spawn;
pub use prompt::classify_prompt;
pub use prompt::password_identity;
pub use resolve::RESOLVE_TIMEOUT;
pub use resolve::Resolved;
pub use resolve::parse_ssh_g;
pub use resolve::resolve;
pub use resolve::resolve_with_timeout;
