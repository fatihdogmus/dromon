# Dromon: Guide for Agents

A small, fast SFTP file browser for macOS, Windows and Linux, in Rust on
[Rune](https://github.com/anomalyco/rune). The product and technical plan is
`.plans/dromon-plan.md` (scope, design spec §6, shortcuts §7, testing §9, decision log §12).

> **Documentation upkeep:** Update documentation only when a change affects a durable
> architectural rule, public contract, configuration requirement or workflow that contributors
> need to know. Keep `AGENTS.md` to repository-wide guidance; put topic detail in the relevant
> document under `docs/`, preferring an existing one.

> **Documentation scope:** NEVER add one-off, task-specific or incident details to `AGENTS.md` or
> any document: no run ids, pull-request numbers, dates of incidents, coverage figures, test-run
> summaries or "today we found". State the durable rule and, in a sentence, why it holds.
> Reports of what a task did belong in the conversation or the pull request.

> **Lazy-read policy:** Do NOT read every referenced document upfront. Open a document only when
> the current task touches its topic. **However, if a task requires modifying code in an area a
> document covers, you MUST read that document before making any changes.**

## Referenced Documents

| Topic | Document | When to Read | Summary |
|---|---|---|---|
| **Architecture** | [docs/architecture.md](docs/architecture.md) | Adding or moving code between crates, touching the `Backend`/`Command`/`ServiceEvent` boundary, state machines, windows, fixtures, persistence, the crate root or the Rune pin. | Crates and dependency direction, UI thread vs service thread, `Action` → step → `Effect`, enum state machines, several windows on one service, the fixture backend, prefs, `dromon-hot`, the Rune pin. |
| **UI** | [docs/ui.md](docs/ui.md) | Changing views, theme, icons, sheets, menus, the palette, toasts, shortcuts, accessibility, motion, scenes or goldens. | Tokens, surfaces, one menu model, `state/keys.rs`, the accessibility audit and Tab tests, the icon pixel grid, springs and reduce motion, scenes, goldens, headless tooling, flows, script events and element keys. |
| **Testing** | [docs/testing.md](docs/testing.md) | Adding, changing or deleting tests or test infrastructure, or deciding where a test goes. | Test layers, state and headless UI patterns, end-to-end tests against the sshd fixture, backends and env vars, the server matrix, perf gates, coverage, building once. |
| **Security** | [docs/security.md](docs/security.md) | Touching prompts, passwords, askpass, the ssh command line, host keys, logging or tracing, or the release workflow. | Secrets never leave memory, the askpass IPC, ssh arguments only through `dromon-ssh`, host keys left to ssh, `DROMON_TRACE`, the signing boundary. |
| **SFTP and transfers** | [docs/transfers.md](docs/transfers.md) | Changing `dromon-sftp`, `dromon-transfer`, listings, polling, or how the app runs transfers. | One session per host, extension detection, listings and diffs, the pure queue, part files and resume, conflicts, copies and moves, progress maths. |
| **CI and pull requests** | [docs/ci.md](docs/ci.md) | Changing `.github/`, investigating a red run, or opening a pull request. | Workflows and jobs, building once, caches, concurrency, PR title and labels, workflow rules, a red-run runbook. |
| **Clean code** | [docs/clean-code.md](docs/clean-code.md) | Any non-trivial production change or refactor. | rustfmt, file size and test files, state machines, naming, doc comments, errors and server data, the UI thread, lints, a review checklist. |
| **Fixtures** | [docs/fixtures.md](docs/fixtures.md) | Writing a fixture for a test, flow or bug repro, or changing the fixture parser. | The JSON format: hosts, servers, sign-in steps, folder trees, generators, changes, transfers, windows, the demo. |
| **Releasing** | [docs/releasing.md](docs/releasing.md) | Touching `release.yml`, `.github/release/`, packaging or signing. | The release run, the security boundary, credentials, signing setup, Homebrew. |
| **Test servers** | [docs/test-servers.md](docs/test-servers.md) | Changing `tests/docker/`, `dromon-testkit`, the native backend, the WSL2 setup or the server variants, or debugging a fixture test. | One server per test binary, users and roles, the Docker, WSL2 and native backends, TOTP, the matrix variants, Windows' own sshd, environment variables, debugging. |
| **Packaging** | [docs/packaging.md](docs/packaging.md) | Changing installers, the icon, package metadata or version handling. | The `packaging/` tree, fixed identifiers, version numbers, changing the icon, checking a package by hand. |

## Project Structure

```
dromon/
├─ Cargo.toml               workspace; Rune pinned to one rev + its [patch.crates-io] block
├─ rust-toolchain.toml      the pinned Rune's toolchain
├─ .cargo/config.toml       the `cargo <command>` aliases (run through xtask)
├─ crates/
│  ├─ dromon-core/          plain shared types and errors; no I/O
│  ├─ dromon-ssh/           ssh config, the ssh command line, askpass, password store, auth
│  ├─ dromon-sftp/          the SFTP session, extensions, listing diff, poll schedule
│  ├─ dromon-transfer/      jobs, the pure queue, naming, part files, progress, I/O
│  ├─ dromon-app/           the Rune app (lib + `dromon` and `dromon-hot` binaries)
│  │  ├─ src/state/         pure app state: Action, Effect, Command, ServiceEvent
│  │  ├─ src/views/         the UI; src/theme/, src/icons/
│  │  ├─ src/service/       the service thread (tokio): connections, polling, transfers
│  │  ├─ src/fixture/       the data-driven fake backend; assets/fixtures/demo.json
│  │  └─ tests/             end-to-end tests against a real sshd (e2e.rs, *_e2e.rs)
│  └─ dromon-testkit/       the sshd fixture for tests (Docker or native)
├─ xtask/                   developer-command runner
├─ scripts/                 goldens, flows, smoke, repro, perf, dev, accents, package-linux
├─ tests/                   docker/ (sshd images), goldens/ (PNG per scene), flows/
├─ packaging/               installers and the icon
├─ docs/                    the guides in the table above
└─ .github/                 workflows, apt-install action, release scripts, PR template
```

## Build, Test, and Development Commands

`cargo xtask` lists the aliases; each runs a script under `scripts/` or a cargo command.

```sh
cargo fmt --all                            # format (rustfmt.toml); CI checks it
cargo lint                                 # cargo fmt --all --check, clippy -D warnings, the 800-line file limit
cargo test --workspace                     # every test; the sshd tests run when Docker answers
DROMON_REQUIRE_DOCKER=1 cargo test --workspace   # fail instead of skipping without Docker (CI)
cargo app [ARGS]                           # the app (debug) on your ssh config
cargo demo [ARGS]                          # the app on the built-in demo, no servers
cargo dev [--mock] [-- APP ARGS]           # live editing: view/theme fn-body edits patch in
cargo goldens [--check]                    # re-render tests/goldens (--check: compare, as CI)
cargo flows [OUT DIR] [FLOW…]              # render tests/flows/* to strips and trees
cargo smoke [--window] [--connect]         # smoke-test the dromon binary
cargo repro <issue|issue.md>               # replay a bug report headless into target/repro
cargo perf [--gate …|all]                  # performance gates as one table
cargo accents                              # accent candidates side by side
cargo package-linux [--arch …] [--verify]  # release binary and Linux packages
```

Headless looks: `cargo demo --scene <name> --snap out.png`, `--tree out.json`,
`--script s.json --strip out.png`, `--fixture <file.json>` ([docs/ui.md](docs/ui.md#reference-headless-tooling)).
After `cargo test --workspace`, pass `DROMON_BIN=target/debug/dromon` to the scripts instead of
building again ([docs/testing.md](docs/testing.md#standard-build-once)).

## Architecture Rules

- Lower crates never depend on `dromon-app` or Rune; pure logic lives in the lowest crate that
  can own it, with no clock and no I/O.
- The UI thread never blocks and does no file or network I/O. Networking is the service
  thread's; the UI and service talk only through `Command` and `ServiceEvent` (`state/service.rs`).
- `state/` is pure and Rune-free; views turn input into an `Action`, `App::act` steps, and the
  view carries out the returned `Effect`s with its `Tx`.
- Behaviour with phases is an explicit `enum` with a pure `step` function, as in Rune.
- A new `Command` is answered by both the service and the fixture backend.
- Rune's revs in `[workspace.dependencies]` and `[patch.crates-io]` are bumped together; never
  edit Rune's checkout under `~/.cargo`.

## Security Rules

- Secrets (passwords, passphrases, codes) are never logged, printed, serialized, written to disk
  or shown by `Debug`; undelivered answers are zeroized; passwords live only in the service's
  memory-only `PasswordStore`.
- User data reaches `ssh` only through `dromon-ssh`'s command builder and its checks, never
  through a shell.
- `DROMON_TRACE=1` names events (kind, host, ids, counts), never their contents.
- Signing credentials live only in the `release` environment; `release.yml` and `.github/release/`
  are the signing boundary ([docs/releasing.md](docs/releasing.md)).

## UI Rules

- Colours come from theme tokens, never literals; motion uses the theme's springs and respects
  reduce motion.
- Every control has a role, a name, a stable key and a keyboard path; the accessibility audit and
  Tab tests must pass.
- A visual change re-renders the goldens; look at every changed image before committing it.

## Testing Rules

- A fix comes with a test that fails without it and passes with it; new behaviour comes with a
  test. Tests stay; never ignore or delete a failing test to get green.
- Tests that need a server take users and homes from the fixture (`Role`, `Fixture::user`,
  `Fixture::home`), never `/home/...` literals, and skip cleanly without Docker.

## Code Style Rules

- Format with `cargo fmt --all` before committing (`rustfmt.toml`: `max_width = 120`,
  `use_small_heuristics = "Max"`); never hand-format against rustfmt. A pure reformat is its own
  pull request; its squash commit on `main` is listed in `.git-blame-ignore-revs`.
- `cargo lint` passes: the format check, clippy with `-D warnings` (with the complexity lints
  and `clippy.toml`'s thresholds) and the file-size limit. Refactor a hit, or put
  `#[expect(<lint>, reason = "…")]` on the one item. `#[allow(<lint>, reason = "…")]` only where
  the lint fires in some builds but not others, or for a module's deliberate exception such as
  the Windows FFI's `unsafe_code`; never module- or crate-wide otherwise, never a raised
  threshold ([docs/clean-code.md](docs/clean-code.md#standard-lints)).
- No `.rs` file has more than 800 lines (a hard limit, checked by `cargo lint`); split along a
  real seam, with tests in a sibling `*_tests.rs` / `tests.rs`.
- Nothing a server, a file or the user sends may panic the app; failures are `dromon_core::Error`.

## Triaging an Issue

1. `cargo repro <number>` takes the issue's fenced `json fixture`, `json script` and `text args`
   blocks and writes `target/repro/<number>/` (`summary.md`, `end.png`, `strip.png`,
   `tree.json`). Read the summary and look at the images.
2. No script block: reproduce from the description with a fixture ([docs/fixtures.md](docs/fixtures.md))
   and a script (`tests/flows/*` are examples), then run `cargo repro` on a saved issue body.
3. The fix comes with a test (and, for a journey, a flow) that shows the bug.

## Avoid Overengineering

- Do not preserve backward compatibility by default. Remove obsolete paths instead of adding
  compatibility layers, fallbacks or migrations, unless the change explicitly requires them.
  (Remembered settings are the exception users would notice: `Saved`'s fields are optional, so
  an older `prefs.json` still loads with defaults.)
- Choose the simplest implementation that fully meets the current requirements. Avoid
  speculative abstractions, traits with one implementation, configuration and indirection.
- Grow the app in layers: the smallest version that works end to end first, then each capability
  on top of a product that already works. Never trade a working app for unfinished complexity.
- Keep concerns separated along the existing seams: pure step vs I/O, state vs views, the
  crates' boundaries.
- Prefer established, maintained crates when they reduce complexity; check the workspace's
  dependencies and their documentation before writing your own or adding a new one.
- Make decisions for the long term; no stopgap meant to be replaced later.
- Do not add guards for states that current invariants make unreachable; handle the realistic
  failures (servers, networks, platforms) instead.
- Leave no dead code: when replacing a path, remove the old one, its flags, parameters and
  helpers in the same change.
- Prefer one canonical code path, fail-fast diagnostics and explicit recovery over silent
  fallbacks or a second temporary path.
- Temporary code (a debugging aid, a narrow transition) says in the same diff why it exists and
  when it is deleted.

## Agent instructions

- Work on a branch per change and open a pull request to `main`; never push to `main`. Pull
  requests are squash merged; their title is the release-notes line, written for users. Fill in
  `.github/pull_request_template.md` and add one label: `feature`, `bug`, `performance` or
  `skip-changelog` ([docs/ci.md](docs/ci.md#standard-pull-requests)).
- DO NOT COMMIT OR PUSH unless explicitly asked. Commit messages are in English; the title says
  what changed, in a short sentence; the body, if any, says why.
- Use `git mv` to move or rename files, so history follows them.
- Change files with the editor tools, not shell redirection, `sed -i` or heredocs, so every change
  is a reviewable diff.
- Each git worktree builds into its own `target/`. Don't share `CARGO_TARGET_DIR` between
  worktrees: `target/debug/dromon`, which scripts and end-to-end tests run, would be another
  worktree's build.
- Never edit Rune's checkout or any other dependency source under `~/.cargo`.
- Look at every changed golden before committing it; re-rendering is not reviewing.
- **NEVER use `tail`, `head`, `grep` or `sed` to cut or filter long command output** (cargo,
  scripts, tests). When output is too long, the tool saves all of it to a file and returns its
  path; read that file with the read or grep tool instead, so no error is hidden.
- **NEVER redirect command output to a file** with `>`, `>>`, `2>`, `&>` or `tee`, including for
  long or background commands. Let the output return to the tool, which keeps oversized output
  itself.
