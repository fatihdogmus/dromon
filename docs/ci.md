# CI and Pull Requests Guide

> Source: `ci.md`
> Purpose: The workflows and their jobs, caches, concurrency, the pull-request workflow, and the rules for changing `.github/`.

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

Releases (the pipeline, signing, credentials, Homebrew) are [releasing.md](releasing.md); this
guide only places `release.yml` among the workflows.

## Index

- [Reference: Workflows and Jobs](#reference-workflows-and-jobs)
- [Decision: Each Test Job Builds Once](#decision-each-test-job-builds-once)
- [Standard: Caches](#standard-caches)
- [Standard: Concurrency and Cancelled Runs](#standard-concurrency-and-cancelled-runs)
- [Standard: Pull Requests](#standard-pull-requests)
- [Standard: Changing Workflows](#standard-changing-workflows)
- [Runbook: A Red CI Run](#runbook-a-red-ci-run)

---

## Reference: Workflows and Jobs

**Scope:** `.github/workflows/`

### Use for

Knowing what runs where.

### Reference

**`ci.yml`**, on every push to `main`, every pull request and by hand:

| Job | Runner | What it does |
| --- | --- | --- |
| `linux` | ubuntu-24.04 | `cargo lint`, `cargo test --workspace` with the Docker sshd required, `cargo goldens --check` (changed goldens uploaded on failure), `cargo smoke` headless, `--window` under Xvfb and `--connect`, then `cargo flows` uploaded as an artifact |
| `linux-matrix` | ubuntu-24.04 | The server matrix: `dromon-sftp` and `dromon-transfer` `--test matrix` with `DROMON_SERVER_MATRIX=1` (no UI libraries built) |
| `macos`, `macos-intel` | macos-15, macos-15-intel | The whole suite once with the native sshd backend (Apple's sshd through sudo, `dromon_*` users, Homebrew's TOTP PAM module), then the headless smoke |
| `windows` | windows-2025 | Docker in WSL2 (Ubuntu 24.04, dockerd on tcp 2375), the whole suite once with Windows' own OpenSSH and the named-pipe askpass, the headless smoke, then `windows_sshd.rs` against Windows' own OpenSSH server (`/C:/` paths) |
| `linux-package` | ubuntu-22.04 | The `.deb` and Arch package with the release scripts, installed in clean containers |
| `macos-package` | macos-15 | The universal `Dromon.app` and DMG, ad-hoc signed, opened and checked; the DMG uploaded |
| `windows-package` | windows-2025 | `dromon.exe`, the MSI with WiX v5, silent install, check, uninstall |

The package jobs build unsigned installers on every push, so a broken package shows on the push
that broke it, not in the next release.

**`release.yml`**, by hand only: one dispatch picks patch, minor or major (or a dry run), one
approval in the `release-approval` environment, then plan → approve → build → sign → assemble →
publish → land → homebrew. It runs no lint or tests: the commit was tested by `ci.yml`. See
[releasing.md](releasing.md).

**`repro.yml`**, on an issue comment starting with `/repro` from the owner, a member or a
collaborator, or the `repro` label: builds the default branch, runs `cargo repro <issue>`,
uploads the outputs and comments the summary. Nothing from the issue runs as shell.

### Agent note

The jobs' comments in `ci.yml` explain each non-obvious step; read them before changing one.

## Decision: Each Test Job Builds Once

**Status:** Accepted
**Scope:** `ci.yml` test jobs

### Context

`cargo test --workspace` and `cargo build -p dromon-app` (or any `-p` subset) resolve dependency
features differently, so running both compiles about a hundred crates twice.

### Decision

Each test job runs one `cargo test --workspace --locked` and sets `DROMON_BIN:
target/debug/dromon`, so `cargo goldens`, `cargo smoke` and `cargo flows` after it run the binary
the tests built. `--no-fail-fast` on macOS and Windows reports every failing test binary.

### Rules and examples

- A new script step after the tests reads `DROMON_BIN` (the scripts already do); it never runs
  its own `cargo build -p dromon-app`.
- Jobs that build only some crates on purpose (`linux-matrix`, the Windows sshd step) say so in
  a comment.

### Consequences

Test jobs stay within their time limits; a step that rebuilds shows up as a long, slow step.

### Agent checklist

- Does a new step reuse the built binary?
- Does a new `-p` invocation in a test job have a reason in its comment?

## Standard: Caches

**Scope:** `Swatinem/rust-cache` steps, `.github/actions/apt-install`, the WSL `.deb` cache

### Purpose

Keep CI fast without letting a cache change what gets built or installed.

### Rules

- rust-cache saves only on `main` (`save-if: github.ref == 'refs/heads/main'`); pull requests
  restore `main`'s caches.
- The package jobs share caches with `release.yml`'s builds through `shared-key`
  (`release-linux-x64`, `release-macos-universal`, `release-windows-x64`), so a release starts
  warm. Keep the keys and the build settings identical on both sides.
- `CARGO_PROFILE_DEV_DEBUG: line-tables-only` is set in test jobs only. rust-cache hashes
  `CARGO_*` variables into its key, so setting it in a package job would split that job's cache
  from the release build's.
- Ubuntu packages come through `.github/actions/apt-install`: its key is the exact list of `.deb`
  files apt resolves, and apt checks each cached file against the signed lists, so the cache
  can't change what is installed. The `windows` job caches Docker's `.deb` files for WSL the
  same way.
- Never save an empty cache: a cache hit on an empty folder would hold from then on and nothing
  would ever be cached. Save steps check that there is something to save.
- Docker base images come from `mirror.gcr.io` (the test fixture's Dockerfiles, packaging
  containers), not Docker Hub directly, which rate-limits CI.

### Agent checklist

- Does a new cache save only on `main`, and never when empty?
- Did a change to a package job keep its key equal to the release build's?
- Does a new base image come through `mirror.gcr.io`?

## Standard: Concurrency and Cancelled Runs

**Scope:** every workflow

### Purpose

A newer push makes an older run pointless; don't let the old one hold runners.

### Rules

- `ci.yml` groups runs by pull request (or ref) with `cancel-in-progress: true`: a newer push
  cancels the older run. `repro.yml` does the same per issue.
- `release.yml` has one group with `cancel-in-progress: false`: releases queue, never cancel
  each other.
- A step that must run after an earlier failure (uploading flows or a repro's outputs) uses
  `if: ${{ !cancelled() }}`, never `always()`: `always()` keeps a superseded run going and holds
  up the newer one. The one `always()` is cleanup of the temporary keychain in `sign-macos`.

### Agent checklist

- Does a new "even on failure" step use `!cancelled()`?

## Standard: Pull Requests

**Scope:** every change to `main`

### Purpose

Every change on `main` is reviewed, tested, and one readable line in the release notes.

### Rules

- Work on a branch and open a pull request to `main`; never push to `main`. (The release
  workflow's `land` job is the one automated writer.)
- Pull requests are squash merged: one commit per pull request on `main`, whose subject is the
  pull request's title. `.github/release/changelog.sh` turns that title into the release-notes
  line, so write it for users ("Upload button: pick files to upload", not "fix toolbar.rs").
- Label it: `feature` (New), `bug` (Fixes), `performance` (Faster), or `skip-changelog` for
  refactors, tests, CI and docs nobody would notice. Unlabelled pull requests land under
  *Other changes*.
- Fill in `.github/pull_request_template.md`: what and why, how it was verified (the commands
  and what they showed), which goldens changed and that each was looked at, and the checklist.
- One concern per pull request: no unrelated renames, reformatting or drive-by fixes. A pure
  reformat is its own pull request.

### Agent checklist

- Is the title a user-facing sentence, and is there exactly one release-notes label?
- Does the description list the commands run and the goldens looked at?

## Standard: Changing Workflows

**Scope:** `.github/`

### Purpose

Keep CI reproducible and the release boundary intact.

### Rules

- Pin every third-party action by full commit SHA with the version in a comment, as the
  existing steps do.
- `permissions: contents: read` at the top; a job that needs more (`repro`: `issues: write`)
  asks for it alone. Checkouts use `persist-credentials: false`.
- Test jobs set `DROMON_REQUIRE_DOCKER=1`, so a fixture that can't start fails instead of
  skipping.
- Multi-line scripts for WSL run through `wsl-bash`; scripts passed from pwsh keep CRLF endings.
- `release.yml` and `.github/release/` are the signing boundary
  ([releasing.md](releasing.md#the-security-boundary)); review changes there like code that
  holds the keys.
- Comments in workflows state the durable reason for a step. Run ids, dates and one-off
  incidents belong in the pull request, not in the file.

### Agent checklist

- Are new actions SHA-pinned, with least permissions?
- Did a workflow comment get a run id or incident note? Move it to the pull request.

## Runbook: A Red CI Run

**Scope:** `ci.yml` failures

### Use when

A job fails on a pull request.

### Procedure

1. Read the failing step's log in full, from the first error, not the last line.
2. `Goldens unchanged`: download the `goldens-linux` artifact, look at `diff/*.png` and the
   `.new.png` beside each. Meant: `cargo goldens` and commit the images you looked at. Not meant:
   fix the view.
3. Fixture tests on macOS or Windows only: run the same test on Linux with Docker first; then
   read the platform notes in [test-servers.md](test-servers.md) (native backend, WSL2).
4. A step that hits its timeout after a long compile: something rebuilt; check for a `-p` build
   after `cargo test --workspace` ([Decision: Each Test Job Builds Once](#decision-each-test-job-builds-once)).
5. A package job: rebuild locally with the same script (`cargo package-linux --verify`,
   `packaging/macos/build-app.sh`, `packaging/windows/build-msi.ps1`).
6. Re-run a job only when the failure is understood as unrelated to the change; say which in the
   pull request.

### Success signals

The cause is named in the pull request, and the fix (or the test that shows it) is in the
diff.
