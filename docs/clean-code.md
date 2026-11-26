# Clean Code Guide

> Source: `clean-code.md`
> Purpose: Rust conventions as practised in Dromon: formatting, file size and test files, state machines, naming, doc comments, errors, the UI thread, lints, and a review checklist.

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

The rules for scope and simplicity (no compatibility layers, no speculative abstractions, no dead
code) are [Avoid Overengineering in AGENTS.md](../AGENTS.md#avoid-overengineering); they apply to
every change.

## Index

- [Standard: Formatting](#standard-formatting)
- [Standard: Files and Their Tests](#standard-files-and-their-tests)
- [Pattern: Phases Are Enums With a Pure Step](#pattern-phases-are-enums-with-a-pure-step)
- [Standard: Naming](#standard-naming)
- [Standard: Doc Comments](#standard-doc-comments)
- [Standard: Errors and Server Data](#standard-errors-and-server-data)
- [Standard: Nothing Blocks the UI Thread](#standard-nothing-blocks-the-ui-thread)
- [Standard: Lints](#standard-lints)
- [Standard: Review Checklist](#standard-review-checklist)

---

## Standard: Formatting

**Scope:** every `.rs` file in the workspace

### Purpose

One mechanical format, so diffs show only real changes and nobody argues about layout.

### Rules

- Format with `cargo fmt --all`, which reads the repository's `rustfmt.toml`
  (`max_width = 120`, `use_small_heuristics = "Max"`). Wide lines are the house style: short
  expressions stay on one line up to 120 columns.
- CI checks it: `cargo fmt --all --check` is part of `cargo lint`. Run `cargo fmt --all` before
  every commit.
- Never hand-format against rustfmt: no manual alignment it would undo, no `#[rustfmt::skip]`
  to keep a layout you prefer. Change `rustfmt.toml` only in its own pull request.
- A pure reformat (after a rustfmt or config change) is its own commit with nothing else in it,
  and its hash goes into `.git-blame-ignore-revs`, so `git blame` skips it.

### Agent checklist

- Did `cargo fmt --all --check` pass before committing?
- Does the diff contain formatting changes outside the lines you meant to touch? Then they belong
  in a separate reformat commit.

## Standard: Files and Their Tests

**Scope:** every crate

### Purpose

Files short enough to read whole, with their tests next to them.

### Rules

- Keep a file under about 800 lines. Before it grows past that, split it along a real seam (a
  sub-machine, a group of actions, one platform's transport), not into arbitrary halves.
- Tests go in a sibling file, wired from the module:
  ```rust
  #[cfg(test)]
  #[path = "command_tests.rs"]
  mod tests;
  ```
  A `mod.rs` uses `tests.rs`. A small module may keep a short inline `mod tests`.
- Declare new `dromon-app` modules in `app_root.rs` (see
  [architecture.md](architecture.md#reference-binaries-and-the-crate-root)).
- One concern per file; the module doc says what it owns.

### Agent checklist

- Is any touched file over ~800 lines after the change?
- Are new tests in the sibling test file?

## Pattern: Phases Are Enums With a Pure Step

**Scope:** every crate

### Use when

Something moves through stages: a connection, an edit, a task, a popover, an animation.

### Pattern

An explicit `enum` of phases and a pure `step(state, event)` that returns the next phase and
what to do, with no I/O and no clock; the owner performs the effects and passes time in. The
existing machines and their files are listed in
[architecture.md](architecture.md#pattern-phases-as-enum-state-machines).

```rust
pub fn step(m: &mut Lifecycle, event: Event) -> Vec<Effect> { … }   // service/lifecycle.rs
pub fn step(state: &TaskState, ev: TaskEvent) -> Option<TaskState> { … }   // dromon-transfer
```

- Prefer one enum over several booleans whose combinations mean phases.
- Each transition gets a unit test of `step`.

### Agent checklist

- Could the new flags be one enum?
- Is the step pure, and tested without a runtime?

## Standard: Naming

**Scope:** every crate

### Purpose

Names that read as plain English and say what something is in the user's terms.

### Rules

- Types are nouns of the domain (`RemotePath`, `JobRequest`, `ConflictChoice`); actions name what
  the user did (`Action::GoUp`, `Action::Download`), events what happened
  (`ServiceEvent::Opened`).
- Functions say what they return or do (`unique_name`, `exit_reason`, `shortcut`); predicates
  read as questions (`is_refusal`, `rememberable`).
- Test names are sentences of the behaviour, snake_case:
  `double_click_opens_a_folder_and_back_returns`, `answers_never_print_their_secret`.
- Element keys are lower-case and hyphenated (`tb-back`, `sheet-submit`, `rows/<name>`); they are
  part of the scripting surface, so renaming one is a change to flows and issue scripts.
- User-facing text is short, plain and specific ("You don't have access to /var/log"), with the
  same words the UI uses elsewhere.

### Agent checklist

- Does the name say what the thing is in the domain, not how it's built?
- Does a renamed key break `tests/flows/*`?

## Standard: Doc Comments

**Scope:** every crate

### Purpose

Every file explains itself; every public item says what it is for.

### Rules

- Each module starts with `//!`: what it owns, the rules it keeps, and where the neighbouring
  parts are, in plain sentences. Plan sections are cited as `§4.6`.
- Public items get `///` saying what the item is and anything a caller must know (invariants,
  units, when it returns `None`). Not a restatement of the signature.
- Match the density of the surrounding file. A comment inside a function explains *why*
  (a platform quirk, an ssh version difference), never what the next line does.
- Docs describe the code as it is now. No changelogs, dates, ticket numbers or "we found today" in
  comments.

### Agent checklist

- Does a new file have a module doc, and new public items a `///`?
- Is any comment narrating the code instead of explaining a reason?

## Standard: Errors and Server Data

**Scope:** every crate

### Purpose

Failures reach the user as a sentence they can act on; nothing a server sends can crash the app.

### Rules

- User-facing failures are `dromon_core::Error`; each variant's message reads well in the UI
  as is. Lower layers map their errors onto it (`dromon-sftp/src/error.rs`,
  `From<CommandError>`). Error types use `thiserror`.
- Never panic on data from a server, a file or the user: listings, prompt texts, ssh's stderr,
  config files, fixtures. Return an `Error` or skip the entry with a reason.
- `unwrap`/`expect` only for invariants the code itself guarantees, with an `expect` message
  that names it. Tests may unwrap freely.
- Local I/O errors name the path and the OS message (`Error::Io`).
- Error messages never quote secrets or message contents ([security.md](security.md)).

### Agent checklist

- Can any new `unwrap` be reached by server or user data?
- Does a new error message read well in the UI on its own?

## Standard: Nothing Blocks the UI Thread

**Scope:** `dromon-app` (`state/`, `views/`, `windows.rs`)

### Purpose

The window answers on the frame the user presses, always.

### Rules

- No file or network I/O, no `std::thread::sleep`, no waiting on a channel or lock that another
  thread holds for long, in views, handlers or state updates.
- Work goes to the service thread as a `Command`, or to a short-lived or dedicated thread
  (`reveal.rs`, `persist.rs`); results come back as events.
- On the service thread, blocking calls go through `spawn_blocking`; a mutex is never held
  across an await.

### Agent checklist

- Does any new code on the UI thread touch the disk or network, or wait?
- Is new blocking work on the service thread in `spawn_blocking`?

## Standard: Lints

**Scope:** the workspace

### Purpose

Warnings are errors, everywhere, the same way CI checks them.

### Rules

- `cargo lint` runs `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --
  -D warnings`. It must pass before every push.
- `unsafe_code` is denied workspace-wide; `dbg!` and `todo!` are clippy warnings, so errors
  under `-D warnings`.
- Fix the code rather than silencing a lint. An `#[allow(...)]` is narrow (one item) and says
  why in a comment.

### Agent checklist

- Does `cargo lint` pass?
- Does every new `#[allow]` carry its reason?

## Standard: Review Checklist

**Scope:** every change

### Purpose

The questions to ask of a diff before asking anyone else.

### Rules

- Is the change in the lowest crate that can own it, with a pure core and a thin I/O shell?
- Are phases an enum with a pure step, and is every new behaviour tested at the lowest layer
  ([testing.md](testing.md))?
- Does the UI thread stay free of I/O and waiting?
- Can any secret reach a log, an event, `Debug`, an error or the disk?
- Does user data reach ssh only through `dromon-ssh`'s checks?
- Are colours tokens, controls named and reachable, and changed goldens looked at
  ([ui.md](ui.md))?
- Is the old path removed rather than kept beside the new one?
- Does `cargo fmt --all --check` and `cargo lint` pass, and does the diff hold only this change?

### Agent checklist

- Was each question answered by reading the diff, not from memory?
