# Dromon

A small, fast SFTP file browser for macOS, Windows and Linux, in Rust on [Rune](https://github.com/anomalyco/rune).
The plan is `.plans/dromon-plan.md`; where implementation stands is `.plans/progress.md`. Read both before larger work.

## Layout

- `crates/dromon-core`: shared plain types (paths, entries, hosts, prompts, errors). No I/O.
- `crates/dromon-ssh`: ssh config discovery, the `ssh` command line, the askpass helper and IPC, the memory-only password store, the per-attempt auth state machine.
- `crates/dromon-sftp`: the SFTP session over the ssh child's stdio (russh-sftp raw session), extensions (posix-rename, limits, copy-data), listing diff, poll schedule.
- `crates/dromon-transfer`: conflict naming, part files, task state machine, progress maths.
- `crates/dromon-app`: the Rune app (lib + `dromon` bin). `state/` is pure app state, `views/` the UI, `service/` the network thread, `fixture/` the data-driven fake backend.
- `crates/dromon-testkit`: the sshd fixture for tests: Docker (testcontainers, `tests/docker/`; on Windows CI through Docker in WSL2), or with `DROMON_FIXTURE_BACKEND=native` a private native `sshd` (macOS CI; creates `dromon_*` users, so CI or `DROMON_FIXTURE_NATIVE_OK=1` only). Tests take users and homes from it (`Role`, `user()`, `home()`), never `/home/...` literals. See `tests/docker/README.md`.
- Rune is pinned to one `dev` rev in the root `Cargo.toml`, together with its `[patch.crates-io]` block; bump them together.

## Commands

`cargo <command>` aliases (`.cargo/config.toml`, the `xtask` crate; `cargo xtask` lists them) run
the scripts under `scripts/`; `bash scripts/<name>.sh` is the same.

```sh
cargo test --workspace                     # everything; the sshd tests run when Docker answers
DROMON_REQUIRE_DOCKER=1 cargo test --workspace   # CI: fail instead of skipping without Docker
cargo lint                                 # clippy --workspace --all-targets -- -D warnings
cargo app [ARGS] / cargo demo [ARGS]       # the app (debug) on your ssh config / on the built-in demo
cargo dev [--mock] [-- APP ARGS]           # live editing: view/theme fn-body edits patch into the running app
cargo goldens                              # re-render tests/goldens (look at every changed image)
cargo goldens --check                      # CI: render to target/goldens, compare within ±2 per channel
cargo flows                                # render tests/flows/* to target/flows (strips, trees)
cargo smoke [--window] [--connect]
cargo repro <issue-number|issue.md>
cargo perf [--gate frames,first-rows,throughput,soak|all]   # §9 performance gates: one table; exits 1 on a wide miss
```

## Verifying a change

Tests stay: unit tests for pure logic, headless UI tests (`ui_tests.rs`, `ui_sheet_tests.rs`), end-to-end tests against the real sshd (`crates/dromon-app/tests/e2e.rs`). Also look at the app:

- `cargo run -p dromon-app -- --fixture <file.json> --scene <name> --snap out.png` renders a state headless; open the PNG. `--tree out.json` dumps the element tree; `--script s.json --strip out.png` replays clicks and keys. The fixture format is `docs/fixtures.md`; without `--fixture`, `DROMON_MOCK=1` uses the built-in demo.
- `--accent a|b|c` renders with another accent candidate (§6.3; default A, Spearmint); `bash scripts/accents.sh` renders the comparison scenes for all three in both themes to `target/accents/`, with a `compare-<theme>.png` contact sheet each.
- A visual change re-renders goldens; look at every image that changed before committing it.
- `DROMON_TRACE=1` names each service event on stderr (never its contents).

## Triaging an issue

1. `bash scripts/repro.sh <number>` fetches the issue, takes its ```` ```json fixture ````, ```` ```json script ```` and ```` ```text args ```` blocks, and writes `target/repro/<number>/` with `summary.md`, `end.png`, `strip.png` and `tree.json`. Read the summary and look at the images.
2. No script block: reproduce from the description by writing a fixture and script yourself (`tests/flows/*` are examples), then run `scripts/repro.sh` on a saved issue body.
3. A fix comes with a test (or a flow under `tests/flows/`) that shows the bug and passes after the fix.
4. Maintainers can comment `/repro` (or add the `repro` label) to have CI run the same script and reply with the outputs.

## Rules

- Behaviour with phases is an explicit `enum` state machine with a pure `step` function, as in Rune.
- Keep files under ~800 lines; tests in sibling `tests.rs` / `*_tests.rs`.
- Nothing blocks or does file I/O on the UI thread; network work lives on the service thread.
- Secrets (passwords, passphrases, codes) are never logged, printed, serialized or put in `Debug` output; undelivered answers are zeroized.
- `ssh` arguments built from user data go through `dromon-ssh`'s checks; never through a shell.
- Releases: `docs/releasing.md`. Signing credentials live only in the `release` environment.
