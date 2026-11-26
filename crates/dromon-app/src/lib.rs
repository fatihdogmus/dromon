//! Dromon's app: the Rune UI, its state and the service thread. The `dromon` binary
//! (`main.rs`) runs the askpass helper when ssh starts it as one, and [`run`] otherwise.
//!
//! The UI talks to the service thread (`service`), which discovers hosts from the user's ssh
//! config and connects for real. `--fixture <file.json>` runs it on a fixture instead (`fixture`,
//! `docs/fixtures.md`): hosts, folders, errors and sign-in prompts from the file, no network.
//! `DROMON_MOCK=1` is the same with the built-in demo (`assets/fixtures/demo.json`), which
//! scenes and the in-crate tests always use.
//!
//! The public surface is what the end-to-end tests (`tests/e2e.rs`) drive the app with:
//!
//! - [`app`]: the real app (view, handlers, `on_key`, window chrome), for
//!   `app().headless_runtime(..)`; its state starts on the demo fixture, which a test replaces.
//! - [`state`]: `App`, `Action`, `ServiceEvent`, the sheets, and everything a test reads back.
//! - [`service`]: the service thread (`start`, `channel`, `Config`, `Sink`).
//! - [`run`]: what the binary does after the helper check.
//!
//! The items live in `app_root.rs`, which `hot.rs` includes too: that is `dromon-hot`, the
//! live-editing binary (`scripts/dev.sh`), the same crate compiled as one executable, because
//! hot patching reaches only the executable's own crate, not a library it links.

include!("app_root.rs");

// rustfmt follows `mod` declarations but not `include!`, so `cargo fmt` would never reach
// app_root.rs and its modules. This one is never compiled (`cfg(any())` is always false); it
// only points rustfmt at them.
#[cfg(any())]
#[path = "app_root.rs"]
mod app_root_for_rustfmt;
