//! Dromon's binary: the askpass helper when ssh runs us with `DROMON_ASKPASS_ENDPOINT` set,
//! else the app (`dromon_app::run`).

fn main() {
    // ssh runs us as its askpass helper: answer and exit before any UI or runtime starts.
    if dromon_ssh::is_helper_invocation() {
        std::process::exit(dromon_ssh::run_helper());
    }
    dromon_app::run();
}
