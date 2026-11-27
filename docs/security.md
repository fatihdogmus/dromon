# Security Guide

> Source: `security.md`
> Purpose: How secrets are kept, the askpass helper and its IPC, how ssh's command line is built, host keys, what tracing may say, and where the release signing boundary is.

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

## Index

- [Decision: Secrets Never Leave Memory](#decision-secrets-never-leave-memory)
- [Standard: Secrets in Memory](#standard-secrets-in-memory)
- [Reference: The Askpass Helper and Its IPC](#reference-the-askpass-helper-and-its-ipc)
- [Decision: ssh Arguments Only Through dromon-ssh](#decision-ssh-arguments-only-through-dromon-ssh)
- [Decision: Host Keys Are ssh's Call](#decision-host-keys-are-sshs-call)
- [Standard: Tracing and Logs](#standard-tracing-and-logs)
- [Reference: The Release Signing Boundary](#reference-the-release-signing-boundary)

---

## Decision: Secrets Never Leave Memory

**Status:** Accepted
**Scope:** every crate; secrets are passwords, key passphrases and keyboard-interactive answers (2FA codes, OTPs)

### Context

Dromon asks for secrets so ssh can sign in, and promises users they are kept in memory only
and never written or logged.

### Decision

A secret is never logged, printed, serialized, written to disk, put in an error message, or
shown by a `Debug` impl. It lives only where it is needed, and is wiped when it is no longer
needed or was never delivered.

### Rules and examples

- Types that hold a secret redact their `Debug`: `PromptAnswer` (`PromptAnswer::Text(<redacted>)`),
  `AskpassEnv` and `SshCommand` (the askpass token), askpass `Reply`. A new type that holds a
  secret does the same, with a test that `format!("{x:?}")` doesn't contain it.
- No secret type derives `Serialize`; `PasswordStore` has none and its `Debug` prints counts.
- `Command::AnswerPrompt` is the only place a secret crosses from the UI to the service; no
  `ServiceEvent` carries one.
- Error messages name what failed, never message contents (`askpass/protocol.rs`).
- Only passwords and passphrases may be remembered (`PromptKind::rememberable`); codes and every
  other prompt are asked each time.

### Consequences

Debugging sign-in relies on prompt kinds, ids and ssh's own stderr, not on what was typed.
End-to-end tests check every event and log line for the secrets they typed.

### Agent checklist

- Can the new value hold a secret? Then is its `Debug` redacted and is it free of `Serialize`?
- Does any new log line, error or event include prompt answers?
- Is there a test that the secret doesn't appear?

## Standard: Secrets in Memory

**Scope:** `dromon-ssh/src/passwords.rs`, `dromon-app/src/service/auth.rs`, `dromon-app/src/state/sheets.rs`, `dromon-app/src/fixture/backend.rs`

### Purpose

Hold secrets for as short a time as possible and wipe them on every path.

### Rules

- `PasswordStore` (memory only, `SecretString`, zeroized on drop) is owned by the service
  thread, keyed by host and `user@host` identity (passwords) or key path (passphrases). It is
  cleared for a host on Disconnect (`Effect::ForgetSecrets`), for an identity when ssh rejects
  it (`AuthAttempt`), and wholly when the UI goes away.
- Typed text lives only in the open sheet's field. Submit moves it into the answer (no copy
  stays behind); cancel wipes it.
- An answer for a prompt that is gone is ignored and wiped (`service/auth.rs`); the fixture
  backend wipes answers after comparing them.
- Secret buffers on the askpass path are `Zeroizing` (tokens, requests, replies, the helper's
  output line).
- The mutex around the store is locked briefly and never across an await.

### Agent checklist

- Is every path that drops a secret (cancel, stale id, error) wiping it?
- Is a new secret buffer `Zeroizing` or a `SecretString`?

## Reference: The Askpass Helper and Its IPC

**Scope:** `dromon-ssh/src/askpass/`, `dromon-app/src/main.rs`

### Use for

Understanding how ssh's questions reach the UI.

### Reference

- Each ssh child gets `SSH_ASKPASS=<the dromon executable>`, `SSH_ASKPASS_REQUIRE=force`,
  `DISPLAY` when empty, `DROMON_ASKPASS_ENDPOINT` and a fresh `DROMON_ASKPASS_TOKEN` per
  connection attempt.
- `main()` checks `dromon_ssh::is_helper_invocation()` before any UI or runtime starts; the
  helper is plain `std` so it starts in milliseconds, sends one request and prints the answer.
- Wire format (`protocol.rs`): one JSON line each way, at most `MAX_MESSAGE_BYTES`, then the
  server closes. Unknown tokens and malformed or oversized requests close the connection without
  forwarding anything.
- Transport: a Unix domain socket in a 0700 per-user folder, or on Windows a named pipe whose
  DACL admits only the current user (`win32.rs`). Both also check the connecting process's user;
  the token stays the primary check.
- Prompts are classified (`classify_prompt`) into `PromptKind`; `AuthAttempt` decides per attempt
  whether a remembered secret answers silently or the user is asked (see the table in
  `dromon-ssh/src/auth.rs`).
- The live updates' second session (`service/live.rs`) signs in again (`ControlMaster=no`, and
  Windows' OpenSSH has no connection sharing). Each run of it binds an askpass endpoint and an
  `AuthAttempt` of its own, so no prompt of another ssh is ever taken for its. A remembered
  password or passphrase answers, exactly as on a reconnect; anything that would need the user
  (a code, a host-key question, a notice) is cancelled and the watch gives up for the session.
  Its prompts never reach the UI.

### Agent note

A change to the wire format, transport or token handling needs `askpass_ipc.rs`,
`askpass_transport.rs` and `askpass_e2e.rs` to pass on all three CI platforms.

## Decision: ssh Arguments Only Through dromon-ssh

**Status:** Accepted
**Scope:** `dromon-ssh/src/command.rs`, `quick.rs`, `hosts_conf.rs`, `process.rs`

### Context

Hosts, users, ports, identity files and jump hosts come from the user (quick connect, saved
hosts) and end up on ssh's command line and in ssh config files. A value starting with `-`, a
line break or an unquotable character could become an ssh option or a second config line.

### Decision

ssh is spawned directly (`tokio::process::Command` with an argument vector), never through a
shell. Every user-supplied value reaches ssh only through `SshCommand::sftp` and the checks
behind it.

### Rules and examples

- `check_arg` refuses empty values, a leading `-` and line breaks or NUL; `config_value` quotes
  or refuses what ssh versions read differently (`"`, backslash sequences, `${`, `%` in `User`).
- Quick-connect input goes through `dromon_ssh::quick_host`, and `hosts.conf` and "Add to SSH
  config" blocks are written with the same value rules, so a file reads back exactly.
- SSH-config hosts pass only their alias; the user's config decides the rest.
- The base options are fixed in `BASE_ARGS` (`BatchMode=no`, keep-alives, `ConnectTimeout`,
  `ControlMaster=no`). Change them only with the plan's §4.3 and a test.
- The live updates' second session (`SshCommand::watch`) is the one command line with a remote
  command, and the only shell text Dromon sends a server: a fixed script with the folder as its
  one value, POSIX single-quoted (`'` → `'\''`; a NUL is refused), after `--`. The same host
  options, base options and askpass as the SFTP session's; `command_tests.rs` checks the quoting
  against a real shell.
- Nothing from an issue body or a fixture is ever run as shell (`scripts/repro.sh` passes blocks
  as files and refuses arguments outside its list).

### Consequences

Some odd but legal values are refused with a message rather than passed through. That is the
intended trade.

### Agent checklist

- Does the new value reach ssh only through `SshCommand` and its checks?
- Is there a test in `command_tests.rs` or `quick_tests.rs` for the new value's edge cases?
- Is any new process spawned without a shell?

## Decision: Host Keys Are ssh's Call

**Status:** Accepted
**Scope:** `dromon-ssh/src/prompt.rs`, `dromon-app/src/service/lifecycle.rs`, the host-key sheet

### Context

Trusting a host key is ssh's security decision; a second implementation would drift from it.

### Decision

Dromon never reads or writes `known_hosts` itself. An unknown key reaches the app as ssh's own
askpass question (`PromptKind::HostKey` with key type and fingerprint); the user's Trust answers
`yes` and ssh records the key. A changed key is refused by ssh; `lifecycle::exit_reason`
explains it and names the `known_hosts` file and line from ssh's "Offending key" output.

### Rules and examples

- Host-key prompts are never remembered or answered silently.
- A host-key refusal fails the connection without retries (`ExitReason::is_refusal`).
- Tests run with a private `known_hosts` (`UserKnownHostsFile`) so the developer's is never
  touched.

### Consequences

Dromon behaves like the user's ssh, `StrictHostKeyChecking` and all.

### Agent checklist

- Does the change leave `known_hosts` to ssh?
- Is a host-key prompt still always shown to the user?

## Standard: Tracing and Logs

**Scope:** `DROMON_TRACE`, connection logs, `eprintln!` anywhere

### Purpose

Make runs observable without exposing what users or servers sent.

### Rules

- `DROMON_TRACE=1` prints one line per service event (`state::service::trace_line`): the kind,
  the host, ids and counts, never paths, names, prompt texts, errors or answers. A new
  `ServiceEvent` variant gets a trace line and a test that its contents don't appear.
- The connection log holds ssh's stderr and the service's own notes, never answers.
- The live watch's output names events only (`--format %e`), never files, and its trace line
  (`Live host=… state=…`) never names the folder.
- Notifications show no more than the transfers popover does.

### Agent checklist

- Does the new trace line name only the kind, host, ids and counts?
- Is there a test that its text stays out?

## Reference: The Release Signing Boundary

**Scope:** `.github/workflows/release.yml`, `.github/release/`

### Use for

Knowing what not to touch casually.

### Reference

Build and package jobs run repository code with no environment, secrets or `id-token`. Signing
and publishing jobs run in the `release` environment (main only, after the one approval in
`release-approval`), check out only `.github/release/`, run only those scripts and SHA-pinned
actions, and verify every artifact's digest. Changes to `release.yml` or `.github/release/` are
reviewed like code that holds the keys. Details, credentials and the policy:
[releasing.md](releasing.md#the-security-boundary).

### Agent note

Never add a build step, a dependency install or an unpinned action to a job that runs in the
`release` environment.
