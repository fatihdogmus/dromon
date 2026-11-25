//! Dromon's binary: the askpass helper when ssh runs us with `DROMON_ASKPASS_ENDPOINT` set,
//! `dromon --version`, else the app (`dromon_app::run`).

// Release builds are Windows GUI-subsystem programs: no console window opens with the app. The
// askpass helper still answers ssh: ssh starts it with its pipe as the child's standard output
// (STARTF_USESTDHANDLES), which a GUI-subsystem process inherits and `io::stdout()` writes to like
// any other handle; only a program started without one (from Explorer) has no stdout, and Rust
// then discards writes instead of failing. See docs/releasing.md, "Windows: no console window".
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // ssh runs us as its askpass helper: answer and exit before any UI or runtime starts.
    if dromon_ssh::is_helper_invocation() {
        std::process::exit(dromon_ssh::run_helper());
    }
    // For packaging checks and bug reports; answered before Rune parses any argument.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "--version") {
        println!("dromon {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    dromon_app::run();
}
