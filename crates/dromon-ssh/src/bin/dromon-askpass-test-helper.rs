//! The askpass helper on its own, exactly as the app's `main()` runs it in helper mode. A bin
//! target only so integration tests can point `SSH_ASKPASS` at it through
//! `env!("CARGO_BIN_EXE_dromon-askpass-test-helper")`; the app itself is its own helper.

fn main() {
    std::process::exit(dromon_ssh::run_helper());
}
