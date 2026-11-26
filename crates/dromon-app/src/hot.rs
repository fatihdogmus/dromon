//! `dromon-hot`: the app for live editing (`scripts/dev.sh`, `--features hot`). Hot patching
//! reaches only the executable's own crate, and `dromon` (`main.rs`) links the views as a
//! library, so this binary compiles the whole app as its own crate: the same root items as
//! `lib.rs` (`app_root.rs`), whose public surface goes unused here.
#![expect(dead_code, reason = "the library's public surface (`app_root.rs`) goes unused in this binary")]

include!("app_root.rs");

fn main() {
    // ssh runs us as its askpass helper (`current_exe`): answer and exit, as `dromon` does.
    if dromon_ssh::is_helper_invocation() {
        std::process::exit(dromon_ssh::run_helper());
    }
    run();
}
