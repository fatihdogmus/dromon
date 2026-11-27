# Architecture Guide

> Source: `architecture.md`
> Purpose: The crates and their dependency direction, the UI thread and the service thread, how state, views and the service divide the app, windows, fixtures, persistence, live editing and the Rune pin.

## Field contract

This guide uses typed sections with real fields, not title-only labels.

| Section type | Required fields | Use for |
| --- | --- | --- |
| Decision | Status, Scope, Context, Decision, Rules and examples, Consequences, Agent checklist | Durable choices and boundaries |
| Standard | Scope, Purpose, Rules, Agent checklist | Conventions and rules |
| Pattern | Scope, Use when, Pattern, Agent checklist | Repeatable implementation shapes |
| Recipe | Scope, Use when, Steps, Done when | Step-by-step workflows |
| Runbook | Scope, Use when, Procedure, Success signals | Troubleshooting and operations |
| Reference | Scope, Use for, Reference, Agent note | Tables, inventories, commands, and lookup material |

The product view of the same design (diagrams, the SSH launch line, polling intervals) is the
plan, `.plans/dromon-plan.md` §4. This guide states the rules the code keeps.

## Index

- [Decision: Crates and Dependency Direction](#decision-crates-and-dependency-direction)
- [Decision: The UI Thread and the Service Thread](#decision-the-ui-thread-and-the-service-thread)
- [Pattern: Action, State Step, Effect](#pattern-action-state-step-effect)
- [Pattern: Phases as Enum State Machines](#pattern-phases-as-enum-state-machines)
- [Decision: Several Main Windows, One Service](#decision-several-main-windows-one-service)
- [Reference: The Fixture Backend and the Demo](#reference-the-fixture-backend-and-the-demo)
- [Standard: Persistence](#standard-persistence)
- [Reference: Binaries and the Crate Root](#reference-binaries-and-the-crate-root)
- [Standard: The Rune Pin](#standard-the-rune-pin)
- [Reference: Where Things Live in dromon-app](#reference-where-things-live-in-dromon-app)

---

## Decision: Crates and Dependency Direction

**Status:** Accepted
**Scope:** every crate under `crates/`

### Context

Most of Dromon is plain logic: path rules, prompt classification, conflict naming, the transfer
queue, connection phases. Keeping that logic in small crates with no UI and few dependencies
makes it fast to test and keeps the Rune app thin.

### Decision

Six crates, each depending only on crates above it in this table:

| Crate | Owns | Depends on |
| --- | --- | --- |
| `dromon-core` | Plain shared types: `RemotePath`, `Entry`, `HostId`, `HostSpec`, `ConnStatus`, `Prompt`, `PromptAnswer`, `Error`, natural sort. No I/O, no async, no UI. | serde, thiserror |
| `dromon-ssh` | ssh config discovery, `ssh -G`, the `ssh` command line and its checks, spawning ssh, prompt classification, the askpass helper and its IPC, `PasswordStore`, `AuthAttempt`, `Backoff`, `hosts.conf`, quick-connect input. | core |
| `dromon-sftp` | `Sftp`: the session over ssh's stdio (russh-sftp's raw session), extensions, pipelined file I/O, recursive delete, listing diff, poll schedule. | core |
| `dromon-transfer` | Jobs, the pure `Queue` with its per-host scheduler, the task state machine, naming, progress maths, formatting, and `io` (scans, copies, moves). | core, sftp |
| `dromon-app` | The Rune app: state, views, theme, icons, the service thread, fixtures, scenes; the `dromon` and `dromon-hot` binaries. | all four |
| `dromon-testkit` | The sshd fixture for tests (dev-dependency only). | core |

`xtask` is the developer-command runner behind the `cargo` aliases; it is not part of the product.

### Rules and examples

- Lower crates never depend on `dromon-app` or on Rune. A type the UI and the service both need
  goes in `dromon-core` (or in `dromon-app/src/state/service.rs` when it is app-only).
- `dromon-ssh` and `dromon-sftp` don't know each other outside tests: the app hands the ssh
  child's stream (`SshStream`) to `Sftp`.
- Pure logic goes in the lowest crate that can own it, as a function or a type with no clock
  and no I/O (`dromon_sftp::PollSchedule`, `dromon_transfer::queue::Queue`). The caller owns
  timers and sessions.
- `unsafe` is denied workspace-wide (`[workspace.lints.rust]`); the app's one exception is
  `dromon-ssh/src/askpass/win32.rs` (the Win32 pipe DACL), which says so in its module docs.
  The perf example's counting allocator (`examples/perf.rs`, a `GlobalAlloc`) is the other,
  outside the app.

### Consequences

A change in naming, parsing or scheduling is tested without Rune or a server. A new dependency
in a lower crate is a design change: say why in the pull request.

### Agent checklist

- Does the new code sit in the lowest crate that can own it?
- Did any lower crate gain a dependency on Rune, tokio-only UI glue or `dromon-app`?
- Is new pure logic free of clocks and I/O, with the caller passing time in?

## Decision: The UI Thread and the Service Thread

**Status:** Accepted
**Scope:** `dromon-app` (`state/service.rs`, `service/`, `views/`)

### Context

Rune runs every view and handler on the UI thread. A network round trip, a file read or a lock
held across an await there freezes the window.

### Decision

The UI thread owns all app state and never blocks. Networking runs on the `dromon-service`
thread, a multi-threaded tokio runtime started from `on_ready` (`service::start`). The two talk
only through the boundary in `state/service.rs`:

- UI → service: a `Command` through the `Backend` trait. `service::ChannelBackend` puts it on an
  unbounded channel and returns at once.
- Service → UI: a `ServiceEvent` through a `Sink`. `service::RuneSink` applies it on the UI
  thread with `AppHandle::update_if`, which calls `App::apply`.

### Rules and examples

- Nothing on the UI thread does file I/O or waits on the network. Files are read or written on
  other threads: prefs by the `dromon-prefs` thread (`persist.rs`), "Show in Finder" on a
  short-lived thread (`reveal.rs`), `hosts.conf` and ssh config on the service's blocking threads
  (`service/hostfiles.rs`).
- Each connected host has one task (`service/conn.rs`) owning its ssh child, its SFTP session,
  its prompts and its lifecycle; everything about a host happens on that task, in order.
- The transfer engine is one task (`service/transfers.rs`); connection tasks hand it each
  host's session as it comes and goes. See [transfers.md](transfers.md).
- Secrets cross the boundary in one place only, `Command::AnswerPrompt`'s `PromptAnswer`; no
  event carries one. See [security.md](security.md).
- A new kind of work adds a `Command` variant and the `ServiceEvent`s that answer it, and the
  fixture backend learns to answer it too, so scenes and UI tests keep working.

### Consequences

The UI is testable with a synchronous backend, and the service is testable without a window.
Every new command has two implementations to keep in step: the service and the fixture.

### Agent checklist

- Does the change keep blocking work and I/O off the UI thread?
- Did a new `Command` get an answer in `fixture/backend.rs` as well as in `service/`?
- Is the mutex shared by connection tasks (the password store) locked only briefly, never across
  an await?

## Pattern: Action, State Step, Effect

**Scope:** `dromon-app/src/state/` and `dromon-app/src/views/`

### Use when

Adding anything the user can do, or anything the service tells the UI.

### Pattern

`state/` is plain data and functions of it; it never touches Rune's `Tx` or `Cx`. `views/` turns
state into `El` trees and input into actions.

```rust
// views: a handler names what the user did and runs it.
.on_press_tx(|s: &mut App, tx: &mut Tx| run(s, Action::OpenFilter, tx))

// views/mod.rs: the state steps; the view carries out what it asks.
pub fn run(s: &mut App, action: Action, tx: &mut Tx) {
    for fx in s.act(action) {
        effect(fx, tx);
    }
    animate_refresh(s, tx);
}
```

- `App::act(Action) -> Vec<Effect>`: the pure step. It may send `Command`s through the backend.
- `Effect` lists what only the view layer can do: scroll a row into view, blur, copy to the OS
  clipboard, read it, open a context menu, show a file in its folder, close the window.
- `App::apply(ServiceEvent)`: the service's answers, applied on the UI thread; it never blocks
  and may send follow-up commands.
- Keys nothing focused took go through `on_key` in `app_root.rs`, `state::keys::shortcut`, then
  `App::shortcut`, which returns effects the same way.

### Agent checklist

- Is the new behaviour an `Action` (or `ServiceEvent`) handled in `state/`, with a state test?
- Does `state/` stay free of `Tx`, `Cx` and file or network access?
- Is anything only Rune can do expressed as an `Effect` the view carries out?

## Pattern: Phases as Enum State Machines

**Scope:** every crate

### Use when

Behaviour moves through phases: a connection, an edit field, a transfer task, the palette, the
done flourish.

### Pattern

An explicit `enum` of phases and a pure `step` from a phase and an event to the next phase and a
list of effects, as Rune does. The owner carries the effects out; `step` has no I/O and no
clock, so every transition is a unit test.

| Machine | Phases | Step |
| --- | --- | --- |
| Connection lifecycle | `service::lifecycle::Phase` | `lifecycle::step` → `lifecycle::Effect` |
| Live updates' watch | `service::live::Phase` | `live::step` → `live::Effect` |
| Transfer task | `dromon_transfer::TaskState` | `dromon_transfer::step` |
| Inline name field | `state::edit::Edit` | `edit::step` → `EditEffect` |
| ⌘K palette | `state::palette::Phase` | `palette::step` |
| Done flourish | `state::transfers::Flourish` | stepped by `views::transfers::poll` |

### Agent checklist

- Is the new behaviour an enum with a pure step, not booleans spread over a struct?
- Are the transitions tested directly, without a runtime?
- Does the owner, not the step, do the I/O and keep the time?

## Decision: Several Main Windows, One Service

**Status:** Accepted
**Scope:** `state/windows.rs`, `state/common.rs`, `windows.rs`, `service/windows.rs`, `fixture/windows.rs`

### Context

⌘N opens more main windows. Two windows on one host must not open two ssh connections or
two sign-in sheets.

### Decision

Every main window is an `App` of its own (host on screen, folders, history, selection, sheets)
with its own backend over the one service (`Backend::another`), tagged with a `WindowId`. The
service keeps one connection per host and one transfer engine.

### Rules and examples

- Ids are window-scoped: a command carries the window it came from, so the service knows each
  window's folder on screen and whom a prompt goes to (`service/windows.rs`: `Watch`, `Route`).
- Events about hosts, listings and transfers go to every window (`Sink::emit`); a prompt, or
  `Opened` for a host another window had up already, goes to one (`Sink::emit_to`).
- What every window shares lives in `Common` (the in-app clip, `Saved`, the store, frames, the
  peers). A change one window makes that others show travels as `News`.
- One window leads (`App::leads`): the Dock badge, taskbar progress and notifications go through
  it alone.
- The fixture backend serves windows the same way (`fixture/windows.rs`), so window behaviour is
  testable headless.

### Consequences

New per-window state goes in `App`; new shared state goes in `Common` or travels as `News`. Never
add a second connection or engine per window.

### Agent checklist

- Is new state per window or shared, and is it in the right place?
- Does a new event go to every window, or to one with a reason?
- Does the fixture backend route it the same way as the service?

## Reference: The Fixture Backend and the Demo

**Scope:** `dromon-app/src/fixture/`, `docs/fixtures.md`

### Use for

Knowing what answers the UI when no real service runs.

### Reference

- `FixtureBackend` answers commands synchronously from `Backend::send`, with the same
  `ServiceEvent`s the service sends. It runs the service's own lifecycle machine and a real
  `AuthAttempt`, so statuses, retry errors and log lines are the real ones.
- Transfers on a fixture run the real `dromon_transfer::queue::Queue` with instant fake I/O
  (`fixture/transfers.rs`) against one folder overlay (`fixture/tree.rs`) that file operations
  change too.
- There is no clock: polls happen on `Command::Poll` (focus) or ⌘R, applying the fixture's
  scripted `changes` one step at a time.
- `dromon --fixture <file.json>` runs the real app on a fixture: no service thread, no ssh, no
  ssh config. `DROMON_MOCK=1` (`cargo demo`) uses the built-in demo,
  `crates/dromon-app/assets/fixtures/demo.json`, which scenes, goldens and in-crate tests use.
- `Silent` (never answers) and the test-only `Recorder` (records commands) are the other
  backends, in `state/service.rs`.

### Agent note

The fixture format is [fixtures.md](fixtures.md); change it there when you change the parser
(`fixture/parse.rs` rejects unknown fields).

## Standard: Persistence

**Scope:** `state/prefs.rs`, `persist.rs`, `service/hostfiles.rs`

### Purpose

Remember what the user would expect between launches, without secrets and without disk access
on the UI thread.

### Rules

- `Saved` (`state/prefs.rs`) is everything remembered: sort, settings, hidden files, sidebar
  width, the last folder per host (at most `MAX_FOLDERS`), recent quick connects, the last host
  and window frames. Never a secret, never server data beyond a folder path.
- The app hands each change to a `Store`. The real one (`persist::PrefsWriter`) writes Rune's
  `prefs.json` on the `dromon-prefs` thread, newest snapshot only; it is read once before the UI
  starts. Scenes, fixtures, headless runs and tests use `NoStore`, so nothing on disk changes
  what they render.
- Saved hosts are the service's: `hosts.conf` in ssh_config syntax, read and written on the
  service side (`dromon_ssh::hosts_conf`, atomic writes, 0600 on Unix).
- Nothing reconnects at launch; the last host is only offered on the welcome screen.

### Agent checklist

- Is the new remembered value in `Saved`, with a default when missing?
- Could it hold a secret or file contents? Then it doesn't belong there.
- Do tests and scenes still run on `NoStore`?

## Reference: Binaries and the Crate Root

**Scope:** `crates/dromon-app/src/{main.rs, lib.rs, app_root.rs, hot.rs}`

### Use for

Knowing where a new module is declared and why there are two binaries.

### Reference

| File | Role |
| --- | --- |
| `main.rs` | The `dromon` binary: the askpass helper when ssh starts it with `DROMON_ASKPASS_ENDPOINT` set, `--version`, else `dromon_app::run()`. Release builds on Windows are GUI-subsystem programs. |
| `lib.rs` | The library `dromon_app`, which the end-to-end tests drive (`app`, `state`, `service`, `run`). It only `include!`s `app_root.rs`. |
| `app_root.rs` | The crate's real root: every `mod` declaration (the `ui_*_tests` too), `app()`, `run()`, `on_key`, the `AppSpec`. |
| `hot.rs` | `dromon-hot` (`--features hot`), the live-editing binary for `cargo dev`. Hot patching reaches only the executable's own crate, so it compiles the whole app again by including `app_root.rs`. |

### Agent note

Declare new modules in `app_root.rs`, never in `lib.rs` or `hot.rs`, or `dromon-hot` and the
library drift apart. `cargo dev` patches function-body edits in the view and theme code into the
running app (`scripts/dev.sh` lists the paths); any other edit rebuilds and restarts it.

## Standard: The Rune Pin

**Scope:** root `Cargo.toml`, `rust-toolchain.toml`

### Purpose

Rune is pre-release and changes fast; one known revision keeps builds and goldens reproducible.

### Rules

- Rune and `rune-notify` are pinned to one `dev` revision in `[workspace.dependencies]`, and the
  `[patch.crates-io]` block points Rune's patched crates (winit, wgpu-hal, accesskit, …) at the
  same revision. Bump them together, in one commit, and re-check Rune's own root `Cargo.toml` for
  its current `[patch.crates-io]` list.
- `rust-toolchain.toml` matches the pinned Rune revision's toolchain; bump it with the pin when
  Rune's changes.
- A bump re-renders the goldens and runs the whole suite; look at every golden that moved.
- Never edit Rune's checkout under `~/.cargo` (git checkouts or registry sources). A Rune bug is
  reported upstream as an issue with a headless repro; Dromon works around it in its own code.

### Agent checklist

- Do every `rev` and the `[patch.crates-io]` entries name the same revision?
- Did the goldens and the full test suite run after the bump?

## Reference: Where Things Live in dromon-app

**Scope:** `crates/dromon-app/src/`

### Use for

Finding the file for a change.

### Reference

| Folder or file | What it holds |
| --- | --- |
| `state/` | `App`, `Action`, `Effect`, the service boundary (`service.rs`), keys, sheets, palette, transfers mirror, selection, settings, windows, prefs. Pure. |
| `views/` | One module per part of the window; `views/mod.rs` has `view`, `run`, `effect`, `Paint`. |
| `theme/` | Tokens, type scale, springs, bundled fonts, the contrast audit. See [ui.md](ui.md). |
| `icons/` | The icon set (`IconPair`, 1× and 2× bodies) and its pixel tests. |
| `service/` | The service thread: manager (`mod.rs`), connection task (`conn.rs`), lifecycle, auth routing, polling, wake checks, file ops, host files, the transfer engine task. |
| `fixture/` | The data-driven fake backend and the demo. |
| `scenes.rs`, `scenes_settings.rs` | Named states for `--scene`, the goldens and the audits. |
| `windows.rs` | Rune windows: the main windows' `Fleet`, opening and placing them. |
| `state/settings.rs`, `views/settings.rs` | Settings: the values, and their view. |
| `fmt.rs`, `persist.rs`, `reveal.rs` | Number and date text, the prefs writer, "Show in Finder". |
| `ui_*_tests.rs` | Headless UI tests. See [testing.md](testing.md). |

### Agent note

Module docs (`//!`) at the top of each file say what it owns; read them before editing a file.
