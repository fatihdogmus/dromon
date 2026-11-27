# Testing Guide

> Source: `testing.md`
> Purpose: The test layers and what each owns, the sshd fixture, the server matrix, performance gates, coverage, where a new test goes and how to run the suite without rebuilding it twice.

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

How the sshd fixture works inside (image, users, backends, TOTP, variants, environment
variables) is [test-servers.md](test-servers.md); this guide says how to test with it.

## Index

- [Decision: Fixes Come With Tests, and Tests Stay](#decision-fixes-come-with-tests-and-tests-stay)
- [Reference: Test Layers](#reference-test-layers)
- [Standard: Where a New Test Goes](#standard-where-a-new-test-goes)
- [Pattern: State Tests](#pattern-state-tests)
- [Pattern: Headless UI Tests](#pattern-headless-ui-tests)
- [Standard: End-to-End Tests Against the sshd Fixture](#standard-end-to-end-tests-against-the-sshd-fixture)
- [Reference: Fixture Backends and Environment](#reference-fixture-backends-and-environment)
- [Reference: The Server Matrix](#reference-the-server-matrix)
- [Reference: Performance Gates](#reference-performance-gates)
- [Recipe: Measure Coverage](#recipe-measure-coverage)
- [Standard: Run What the Change Affects](#standard-run-what-the-change-affects)
- [Standard: Build Once](#standard-build-once)
- [Reference: Commands](#reference-commands)

---

## Decision: Fixes Come With Tests, and Tests Stay

**Status:** Accepted
**Scope:** every change

### Context

Most Dromon bugs are about timing, platforms or server differences, and come back when nothing
holds them down.

### Decision

A fix comes with a test that fails without it and passes with it. A new behaviour comes with a
test of that behaviour. Tests are not deleted to make a change pass.

### Rules and examples

- Run the new test against the code before the fix and see it fail for the reason the bug
  names. A test that passes either way proves nothing.
- Pick the lowest layer that shows the bug: a pure step test over a UI test, a UI test over an
  end-to-end test. Go higher only when the bug lives in the wiring.
- A test that changes meaning (because the behaviour changed on purpose) is rewritten to the new
  behaviour in the same change, and the pull request says so.
- `#[ignore]` is for slow or opt-in tests only (`--ignored` runs them: the 1 GiB copies, perf,
  the transfer stress test), each with a reason string. Never ignore a failing test.
- A UI journey may also get a flow (`tests/flows/`, [ui.md](ui.md#recipe-add-a-flow)), but a flow
  is a picture, not an assertion.

### Consequences

The suite grows with the product; slow layers stay small because most behaviour is pure.

### Agent checklist

- Did the new test fail before the fix?
- Is it at the lowest layer that shows the bug?
- Were any tests removed or ignored, and is that explained?

## Reference: Test Layers

**Scope:** the workspace

### Use for

Knowing which layer owns a behaviour.

### Reference

| Layer | Where | Owns | Needs |
| --- | --- | --- | --- |
| Pure unit | Sibling `*_tests.rs` / `tests.rs`, or a short inline `mod tests` | Parsers, naming, diff, schedules, `step` fns, prompt classification, command-line checks, progress maths, the queue | nothing |
| State | `crates/dromon-app/src/state/*_tests.rs`, `state/tests.rs` | `App::act` and `App::apply` on the demo fixture; commands sent (`Recorder`) | nothing |
| Fixture backend | `crates/dromon-app/src/fixture/*tests.rs` | That the fake backend answers like the service | nothing |
| Service | `crates/dromon-app/src/service/*tests.rs` | The real manager and connection tasks against a fake `ssh` script and OpenSSH's `sftp-server` | Unix, `sftp-server`, `python3` (skips otherwise) |
| Headless UI | `crates/dromon-app/src/ui_*_tests.rs` | The real view, handlers and `on_key` in Rune's headless `Runtime` | nothing |
| Accessibility | `ui_a11y_tests.rs`, `ui_keyboard_tests.rs`, `ui_access_tests.rs`, `theme/contrast_tests.rs` | Roles, names, reach, Tab order, focus traps, preferences, contrast ([ui.md](ui.md#decision-accessibility-is-tested)) | nothing |
| Icons | `icons/tests.rs` | Pixel-exact edges, icons on whole points | nothing |
| Goldens | `tests/goldens/`, `cargo goldens --check` | What every scene looks like ([ui.md](ui.md#recipe-change-something-visible)) | Vulkan (lavapipe on CI), Pillow |
| Flows | `tests/flows/`, `cargo flows` | Reviewable strips of journeys (artifacts, not assertions) | the binary |
| Crate integration | `crates/dromon-{ssh,sftp,transfer}/tests/*.rs` | askpass end to end, SFTP, transfer I/O against a real sshd | the sshd fixture |
| App end to end | `crates/dromon-app/tests/*_e2e.rs`, `tests/e2e.rs` | The whole app headless, the real service, system `ssh`, the `dromon` binary as askpass helper, a real sshd | the sshd fixture |
| Server matrix | `crates/{dromon-sftp,dromon-transfer}/tests/matrix.rs` | The same checks against several servers | Docker |
| Smoke | `cargo smoke [--window] [--connect]` | The binary starts headless, in a window, and connects | display or Xvfb; Docker for `--connect` |
| Perf | `cargo perf`, `#[ignore]` perf tests | Frame time, first rows, throughput, soak | release build, fixture |

### Agent note

`cargo test --workspace` runs every layer above except goldens, flows, smoke, perf, the full
matrix and `#[ignore]` tests.

## Standard: Where a New Test Goes

**Scope:** every crate

### Purpose

Keep tests next to what they test and files small.

### Rules

- Unit tests for `foo.rs` go in `foo_tests.rs` beside it, wired with
  `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;` (a `mod.rs` uses `tests.rs`). A short
  inline `mod tests` is fine for a small module; move it out before the file nears ~800 lines.
- A new headless UI area gets its own `ui_<area>_tests.rs`, declared under `#[cfg(test)]` in
  `app_root.rs`. Each file keeps its own small helpers (`runtime()`, `click`, `key`, …).
- Tests that need a real server go in the crate's `tests/` folder (one test binary per file,
  one fixture per binary); app end-to-end files are named `*_e2e.rs`.
- A test against a new server capability uses `Fixture::supports` / `Fixture::has`, never an
  assumption about the server.

### Agent checklist

- Is the test in the sibling file of the code it covers?
- Did a new `ui_*_tests.rs` get declared in `app_root.rs`?

## Pattern: State Tests

**Scope:** `crates/dromon-app/src/state/`

### Use when

Testing an `Action`, a `ServiceEvent` or a pure state rule.

### Pattern

Build an `App` on the demo fixture (`fixture::app(fixture::demo())`), act, then read state and
effects. To check what the app sends without anything answering, give it a `Recorder` and read
`Recorder::sent()`; to drive the service side by hand, feed `ServiceEvent`s to `App::apply`.

```rust
#[test]
fn selecting_a_host_connects_and_lists_its_start() {
    let mut s = app();
    s.act(Action::SelectHost(prod()));
    let c = s.conn().unwrap();
    assert_eq!(c.status, ConnStatus::Ready);
    assert_eq!(c.cwd.as_str(), "/var/www/site");
}
```

### Agent checklist

- Does the test go through `act`/`apply` rather than setting fields?
- Does it assert the observable state or the commands sent, not private details?

## Pattern: Headless UI Tests

**Scope:** `crates/dromon-app/src/ui_*_tests.rs`

### Use when

The behaviour is in the view, a handler, `on_key`, focus or the element tree.

### Pattern

`app().headless_runtime(SIZE, Env::new(true, 1.0))` runs the real app without a window. Set the
state from a scene or the demo, then drive it with pointer and key events, a frame after each.

```rust
fn runtime() -> Runtime<App> {
    let mut rt = app().headless_runtime(SIZE, Env::new(true, 1.0));
    rt.update(|s, _| *s = scenes::base());
    rt.frame(0.0);
    rt
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}
```

- Find elements by key (`rt.find`, `rt.elements()`); read state from `rt.state`.
- Keys: `rt.key(KeyEvent { key, mods })`. Use the platform's primary modifier (⌘ on macOS, Ctrl
  elsewhere: `Mods { cmd: true, logo: mac, ctrl: !mac, .. }`), so tests pass on every OS.
- File pickers: `rt.file_dialog()` shows what was asked, `rt.resolve_file_dialog(Some(path))`
  answers it (`None` cancels). No OS dialog opens.
- Reduce motion and other preferences: `Runtime::set_system_reduced_motion` or
  `UiState::access`.

### Agent checklist

- Does the test click and type through keys a user could reach, not call handlers directly?
- Does it advance frames after each input?
- Does it pass on macOS and on Windows and Linux (primary modifier, Delete key)?

## Standard: End-to-End Tests Against the sshd Fixture

**Scope:** crate `tests/` folders, `crates/dromon-testkit`

### Purpose

Prove the real path (system `ssh`, askpass, SFTP, the engine, the app) against a real OpenSSH
server, on every OS CI runs.

### Rules

- Get the server with `dromon_testkit::fixture("<test name>").await` (or `fixture_of(Variant, …)`).
  It returns `None` after printing a skip line when the backend can't run here; the test then
  returns and passes. With `DROMON_REQUIRE_DOCKER=1` (CI) it panics instead.
- Never write user names or home folders: take them from the fixture by `Role`
  (`Role::Key`, `Password`, `Kbd`, `KeyThenPassword`, `EncryptedKey`) with `Fixture::user`,
  `Fixture::home`, `Fixture::path`, `Fixture::destination`. The native backend's users are
  `dromon_*` under `/Users` or `/home`; a `/home/...` literal breaks it.
- Prompt texts differ by platform (OpenPAM vs Linux-PAM): use `Fixture::password_prompt(Role)`.
- What only some servers or backends can do is asked: `Fixture::supports(Feature, test)` prints
  the skip line; `Fixture::has(Feature)` is silent.
- Each test isolates itself: its own ssh config (`-F`), its own `known_hosts`, `IdentitiesOnly
  yes`, and its own scratch folder on the server. The developer's `~/.ssh` is never read.
- Every event and log line an end-to-end test sees is checked for the secrets it typed; keep that
  check in new sign-in tests ([security.md](security.md)).
- Windows: the tests run with the inbox `ssh.exe`; use `dromon_testkit::NULL_DEVICE` and
  `copy_private_key` (current-user ACL) instead of Unix assumptions.

### Agent checklist

- Does the test skip cleanly without Docker and fail with `DROMON_REQUIRE_DOCKER=1`?
- Are there no user-name or home-folder literals?
- Does it isolate ssh config and `known_hosts`?

## Reference: Fixture Backends and Environment

**Scope:** `crates/dromon-testkit`

### Use for

Choosing and debugging the server a test runs against.

### Reference

- Without any setting, the Docker backend runs and the fixture tests skip when Docker doesn't
  answer; `DROMON_REQUIRE_DOCKER=1` (CI) makes that a failure.
- `DROMON_FIXTURE_BACKEND=native` runs the platform's own sshd as root and creates system users:
  CI or throwaway machines only.
- `DROMON_FIXTURE_KEEP=1` keeps the server for debugging.

Every variable, the backends and the debugging steps:
[test-servers.md](test-servers.md#reference-environment-variables).

### Agent note

Read [test-servers.md](test-servers.md) before changing `tests/docker/` or `dromon-testkit`.

## Reference: The Server Matrix

**Scope:** `crates/dromon-sftp/tests/matrix.rs`, `crates/dromon-transfer/tests/matrix.rs`, `dromon_testkit::matrix`

### Use for

Checking compatibility across servers.

### Reference

- A matrix test is one check; `matrix::run(check, |fixture| …)` runs it against every variant of
  `matrix::variants()` and prints one `matrix:` line per server, then fails if any did.
- Variants: `Trixie` (the default, OpenSSH 10), `Bullseye` (OpenSSH 8.4: no `copy-data`, no
  `limits`), `Alpine` (musl, no PAM), `Toxic` (trixie behind toxiproxy: latency, caps, resets).
- Without `DROMON_SERVER_MATRIX=1` only `Trixie` runs, so a plain `cargo test` starts nothing new.
  The native backend skips the matrix.
- A check that needs a capability returns `Outcome::unless(fixture, feature)`.
- Run it: `DROMON_SERVER_MATRIX=1 cargo test -p dromon-sftp -p dromon-transfer --test matrix`.

### Agent note

A new SFTP or transfer behaviour that depends on server features belongs in the matrix, not
only in the default fixture. The variants and how they are built:
[test-servers.md](test-servers.md#reference-server-variants-and-the-matrix).

## Reference: Performance Gates

**Scope:** `scripts/perf.sh`, `crates/dromon-app/examples/perf.rs`, `crates/dromon-app/tests/perf_e2e.rs`, `crates/dromon-transfer/tests/perf.rs`

### Use for

Checking that a change didn't slow the app down.

### Reference

| Gate | Measures | Target |
| --- | --- | --- |
| `frames` | UI-thread CPU per frame scrolling 100,000 rows headless, p99 | half a 120 Hz frame |
| `first-rows` | First page of a 10,000-entry folder to its first painted frame | 150 ms |
| `throughput` | Engine I/O vs `scp` on a large file and on many small files | 80 % of scp |
| `soak` | A large file down and up, SHA-1 checked, memory sampled (not in the default set) | no growth, no mismatch |
| `preview` | Quick preview of a 256 KB text (not in the default set): frames with the panel open, the pointer over it, the text scrolled 24 and 280 pt a frame, p99; then single frames: a text arriving after Space, the next file's text arriving while the cursor may still move and showing once it rested, Up to a text the panel kept, and a minified file's one 256 KB line; the Down key and Down again listed as info. The window's first preview is listed as info only: a window rasterises its first mono glyphs once, which a later preview doesn't pay | p99 rows: half a 120 Hz frame (a whole one is a wide miss); single frames: a whole 120 Hz frame |

`cargo perf [--gate frames,first-rows,throughput,soak,preview|all]` prints one table and exits 1
on a wide miss. Release builds; the perf tests are `#[ignore]` and the script runs them.

### Agent note

CI hardware varies, so the table is informational there. Run the gate a change could affect
locally and quote its line in the pull request.

## Recipe: Measure Coverage

**Scope:** local runs; CI does not measure coverage

### Use when

Looking for untested code in an area you are changing.

### Steps

1. Install the tool once: `cargo install cargo-llvm-cov` (it adds the `llvm-tools` component
   when asked).
2. Run the suite under it, leaving test code out of the report:
   ```sh
   cargo llvm-cov --workspace --html \
     --ignore-filename-regex '(_tests|/tests)\.rs$|/tests/|dromon-testkit/|xtask/'
   ```
   The regex drops sibling test files (`*_tests.rs`, `tests.rs`), integration tests, the
   testkit and xtask. Inline `#[cfg(test)] mod tests` blocks still count in their file.
3. Open `target/llvm-cov/html/index.html` and read the files you touched.

### Done when

You know which branches of the changed code no test reaches, and either covered them or said
why not. Coverage figures don't go into documents or commit messages.

## Standard: Run What the Change Affects

**Scope:** local runs, by people and agents

### Purpose

Fast feedback: a change is checked by the tests that can see it, in minutes, not by the whole
suite. CI runs everything on every pull request, on every platform.

### Rules

- After a change, run only the tests it affects, never the full suite as a habit:
  - the changed crate's tests, narrowed by test-name filter:
    `cargo test -p dromon-app --lib properties` (a module), `-- ui_menu` (a UI test file),
    `--test properties_e2e` (one end-to-end binary);
  - a changed lower crate (`dromon-core`, `dromon-ssh`, `dromon-sftp`, `dromon-transfer`) also
    runs the tests of the crates above it that use what changed;
  - the goldens of the scenes the change reaches (`cargo goldens --check <scene>…`), the flows
    it touches (`cargo flows <flow>…`), the perf gate it could move (`cargo perf --gate …`);
  - `cargo lint` always: it's fast and CI fails without it.
- Run the full suite locally only when the change reaches everything: a Rune bump, workspace
  dependencies or features, the test harness (`dromon-testkit`, `tests/docker/`), shared
  chrome for goldens (see [ui.md](ui.md#recipe-change-something-visible)).
- Every command needs a way to run a subset. When one lacks it, add that first (a filter
  argument, a `--list`), in its own change, before relying on running everything.
- Pick one build shape for the session: keep to `-p` runs (or to `--workspace`); mixing them
  recompiles about a hundred crates ([Build Once](#standard-build-once)).
- The pull request says which tests ran locally; CI's green run covers the rest.

### Agent checklist

- Did you run the tests of the changed code, and only those (plus `cargo lint`)?
- Did a change to a lower crate run the tests of the crates that use it?
- Did you check the goldens of the scenes the change reaches, by name?

## Standard: Build Once

**Scope:** local runs and CI

### Purpose

Avoid compiling the workspace twice in one session.

### Rules

- `cargo test --workspace` and `cargo build -p dromon-app` (or a `-p` subset) resolve
  dependency features differently (feature unification), so the second recompiles about a
  hundred crates. After a full `cargo test --workspace`, run the scripts on the binary it built:
  `DROMON_BIN=target/debug/dromon cargo goldens` (also `cargo smoke`, `cargo flows`).
- CI test jobs run one `cargo test --workspace` each and set `DROMON_BIN` for the scripts after
  it ([ci.md](ci.md)).
- Each git worktree builds into its own `target/`. A shared `CARGO_TARGET_DIR` makes
  `target/debug/dromon`, which the scripts and end-to-end tests run, another worktree's build.

### Agent checklist

- Did you reuse the binary the test run built instead of building again?
- Is `CARGO_TARGET_DIR` unset (or private to this worktree)?

## Reference: Commands

**Scope:** the workspace

### Use for

Running the suite.

### Reference

```sh
cargo test --workspace                           # everything; sshd tests run when Docker answers
DROMON_REQUIRE_DOCKER=1 cargo test --workspace   # fail instead of skipping (CI)
cargo test -p dromon-app --lib ui_keyboard       # one area, by test-name filter
cargo test -p dromon-sftp -- --ignored           # the slow copy-data tests
DROMON_FIXTURE_KEEP=1 cargo test -p dromon-sftp --test fixture -- list_mixed_folder   # keep the server
DROMON_SERVER_MATRIX=1 cargo test -p dromon-sftp -p dromon-transfer --test matrix
DROMON_BIN=target/debug/dromon cargo goldens --check
cargo goldens --check menu-row file-ops-light    # just these scenes; cargo goldens --list names them
cargo flows                                      # tests/flows/* into target/flows
cargo flows download                             # just these flows
cargo smoke [--window] [--connect]
cargo perf [--gate …|all]
```

### Agent note

Run what the change affects ([the standard above](#standard-run-what-the-change-affects));
CI runs the full suite on every pull request.
