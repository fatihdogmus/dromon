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
- [Decision: Local Copies of Files Being Edited](#decision-local-copies-of-files-being-edited)
- [Standard: Tracing and Logs](#standard-tracing-and-logs)
- [Reference: The Release Signing Boundary](#reference-the-release-signing-boundary)
- [Decision: Updates Install Only What the Release Key Signed](#decision-updates-install-only-what-the-release-key-signed)
- [Decision: Bug Reports Are Built Locally, Without Names](#decision-bug-reports-are-built-locally-without-names)

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
- What a quick preview reads (Space) is treated the same way, since a config file can hold
  secrets: it stays in memory and never touches disk (no temporary file, no cache on disk), is
  never logged (`DROMON_TRACE` counts its bytes), shows only sizes in `Debug` (`PreviewBytes`,
  `state::preview::Text`), and is dropped when the panel closes.

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
shell. Every user-supplied value reaches ssh only through `SshCommand::sftp` (or, for "Open in
Terminal", `SshCommand::terminal`) and the checks behind it.

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
- "Open in Terminal" starts a second ssh, the user's terminal app's own, built by
  `SshCommand::terminal`: the same host options and checks, `-t`, and no askpass environment,
  so that ssh asks for passwords, passphrases and codes in the terminal. Dromon never hands it
  a secret it remembers. The folder reaches the server's shell in a remote command
  (`cd -- '<folder>' && exec "$SHELL" -l || exec "$SHELL" -l`), single-quoted by
  `command.rs`'s `quote`.
- Terminal apps start from an argument vector (`dromon-app/src/terminal.rs`), never through a
  local shell. The one exception is macOS's Terminal and iTerm2, which only open files: there
  the launcher writes a `.command` script (mode 0700, a fresh name in the app's cache folder)
  whose only command is the ssh line from `SshCommand::shell_line`, every word quoted by the
  same tested `quote`, and which deletes itself as it starts; `open -a Terminal <script>` runs
  it. No Apple Events are sent, so the hardened app needs no automation entitlement. On
  Windows the console fallback is `conhost.exe` with ssh's argument vector, not `cmd /c`.

### Consequences

Some odd but legal values are refused with a message rather than passed through. That is the
intended trade.

### Agent checklist

- Does the new value reach ssh only through `SshCommand` and its checks?
- Is there a test in `command_tests.rs` or `quick_tests.rs` for the new value's edge cases?
- Is any new process spawned without a shell, and does anything a shell parses go through
  `quote`?

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

## Decision: Local Copies of Files Being Edited

**Status:** Accepted
**Scope:** `dromon-app/src/service/remote_edit.rs`, `state/remote_edit.rs`, `views/mod.rs` (`Effect::OpenFile`)

### Context

Editing in a local editor (⌘E) needs a copy of a remote file on disk for the user's app to
open, and remote files can hold secrets (an `.env`, a config). Quick preview never writes one;
this feature has to.

### Decision

Copies live only in a private folder of the app's cache, only as long as the edit, and nothing
about their contents is logged or traced.

### Rules and examples

- **Where.** `<the app's cache folder>/editing/<run>/<host>-<hash of host and path>/<file name>`
  (`rune::dirs::AppDirs`): one folder per service run, one per edited file in it, the file's own
  name kept so the editor shows it (on Windows, what its names can't hold becomes `_`). The
  folder is `service::Config::editing_root`: only the real app's `Config::for_app` names the
  cache folder; a test passes its own temporary folder, or `None`, where an edit fails at once,
  so no test ever writes to or sweeps the user's cache.
- **Who can read them.** Every folder from `editing` down is `0700` and each copy `0600` on
  Unix, so what an editor writes there (its temporary files, a renamed save) stays private too.
  A link in a folder's place is refused. On Windows the user's local cache folder is private to
  them already and what's made in it inherits that.
- **How long.** An edit's folder goes when it stops (Stop editing), when its host disconnects
  and when the service stops (quitting). Each run holds a lock on `<run>.lock` while it lives;
  at start the service removes every run folder whose lock nobody holds (a crash, or a quit
  that ended before the service could clean up), never a living run's (another Dromon's).
- **Only the service thread touches them.** The UI thread never reads or writes a copy: it
  gets the copy's path in `RemoteEditEvent::Opened` and hands it to Rune's `Tx::open_path`
  (`Effect::OpenFile`), which opens the system's default app off the UI thread (no shell).
- **Nothing logged.** Events carry ids, paths, sizes and dates, never bytes; trace lines name
  the kind, host and id only (`RemoteEditOpened host=prod id=…`), never a path or an error.
  Dromon's own line when a copy can't be opened names no path (Rune's stderr line, when the
  system's opener fails later, names the local copy's path; never its contents).

### Consequences

A crash can leave a copy on disk until Dromon starts again. Users who need a file never to
touch the disk can still use quick preview, which keeps it in memory.

### Agent checklist

- Does any new code write a copy outside the edit's folder, or log its path or contents?
- Is every new folder made with `private_dir`, and every copy restricted to the user?
- Does a new test's `Config` keep `editing_root` out of the user's cache folder?

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
- Files being edited in a local editor are never logged or traced beyond their kind, host and
  id ([Local Copies of Files Being Edited](#decision-local-copies-of-files-being-edited)).

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
`release` environment. The one install there, `sign-linux`'s Ubuntu `minisign` package, comes from
Ubuntu's signed archive through `.github/actions/apt-install` and saves no cache
([releasing.md](releasing.md#the-security-boundary)).

## Decision: Updates Install Only What the Release Key Signed

**Status:** Accepted
**Scope:** `dromon-app/src/service/update*.rs`, `state/update.rs`, `crates/dromon-app/assets/release-key.pub`, `.github/release/sign-minisign.sh`

### Context

The updater downloads a program and runs it, or puts it in place of the running app. Whoever can
change what it installs owns every machine that runs Dromon, so it is part of the signing
boundary.

### Decision

An update installs only when `SHA256SUMS.minisig` is a pre-hashed minisign signature of the
release's `SHA256SUMS` by the key built into the app (`crates/dromon-app/assets/release-key.pub`,
through `include_str!`), with the trusted comment `dromon <version>`, and the downloaded file's SHA-256 is
the one `SHA256SUMS` lists for its name, exactly once. Anything else refuses it with a sentence
the user reads, and changes nothing.

### Rules and examples

- HTTPS only: the agent refuses `http` and redirects to it; only tests' local endpoints allow it.
- A check is one unauthenticated GET of `releases/latest` with the user agent `Dromon`: no
  version, no identifiers, no cookies, no telemetry. Drafts and pre-releases are ignored and a
  version never goes down.
- Nothing from GitHub's answer but the tag is used, and only once it parses as a version: file
  URLs and the release page are built from it, never taken from the response.
- The signature is checked before the large download; a file that fails is deleted.
- A build whose key file holds no usable key still checks and tells, but installs nothing, and
  Settings says why.
- Installers run programs by absolute path with argument vectors, never a shell (`hdiutil`,
  `codesign`, `ditto`, `msiexec` from System32). On macOS the new app must also pass `codesign
  --verify --deep --strict` against a requirement naming the bundle id, a Developer ID
  certificate and the running app's team; an app without a Developer ID (development builds)
  never replaces itself.
- Tests sign with a test-only key (`service/update_testdata/`), never the release key; each check
  has a test that fails without it.

### Consequences

A compromised GitHub account, release asset or network can make an update fail, never install
something else; only the minisign secret can. Rotating or losing that key costs one release
installed by hand ([releasing.md](releasing.md#2b-updates-the-minisign-key)).

### Agent checklist

- Does a new download or URL go through the same agent, HTTPS only, built from the version?
- Is anything installed before both checks pass, or does a failure leave something changed?
- Does a change to the signing step still verify against the committed public key?

## Decision: Bug Reports Are Built Locally, Without Names

**Status:** Accepted
**Scope:** `dromon-app/src/state/report.rs`, `state/record*.rs`, `state/redact.rs`, `views/report.rs`, `.github/ISSUE_TEMPLATE/bug.yml`

### Context

"Report a Bug…" gathers diagnostics for people who can't write a repro, and Record Steps… lets
them record one. Users browse servers whose names, users and paths can be private, and Dromon
has no server of its own to send anything to.

### Decision

The report, and a recording in it, is built on the machine, from what the window already knows
or saw while recording, and shown whole before anything happens. Open GitHub only opens the
browser at the repository's new-issue form with the report in its query (and puts on the
clipboard what the link can't hold, as the sheet says first); the user edits and submits it
there, or uses Copy report and sends it however they like. Nothing is sent by Dromon, and a
recording is never written to disk: it lives in the window and goes with it.

### Rules and examples

- Names are replaced, always (`state/redact.rs`): hosts become `host-1`, `host-2` … (a quick
  connect's `user@address:port` id goes whole). In the report's sentences a folder's path is
  `<path>` and a file's `<dir>/file-3.txt`, with only a short plain extension kept. A recording
  needs names that still work as names: folders `dir-4`, files `file-3.txt`, paths
  `/dir-1/dir-2`, with a hidden one's leading dot. Files and folders are numbered by name, so
  the same name is the same stand-in everywhere; one redactor serves the whole report, its
  diagnostics, fixture, script and steps.
- Errors are rendered from their variant with the stand-in in the path's place, never by
  searching their text. Free-form messages (ssh's reasons, I/O errors, an update's failure)
  are left out with a sentence saying why.
- Events go in as their trace lines (already kinds, ids and counts, [Tracing and
  Logs](#standard-tracing-and-logs)) with the hosts replaced; actions by kind only. Never a
  secret, a prompt's text, a log line or file contents.
- A recording keeps each step's action and the rows it named, and of each server: its home,
  the kinds of sign-in question it asked and whether an answer was refused, the first listing of
  each folder (sizes, dates and permissions kept; owners and groups dropped) and listing errors.
  It never reads the prompt sheet's field: the fixture accepts placeholders (`pw`, `123456`)
  and the replay types those, or `wrong` where the server refused. Host-key fingerprints are
  made up, prompt and notice texts are generic, a failed connection's ssh text is left out.
  Text typed into the filter or a search (pieces of names) isn't recorded, only that it
  happened; a rename's or ⌘L's text is a name or a path, and goes through the redactor. Local
  paths (uploads, the download folder) don't enter it: those steps are named by kind only.
- Tests fill a window with distinctive names, a recording with listings, a rename to a secret
  name, a ⌘L path, a password answered with a secret string and an upload from a local path, and
  check none reaches the report, the link, any of its fields or what Open GitHub copies.

### Consequences

Reports say less than a log would; a maintainer who needs more asks for the repro form or the
connection log, which the user reads before sharing. A recording replays what the window did,
not the real servers: anything their real names or contents decide can't show in a replay.

### Agent checklist

- Does new report or recording content come from kinds and counts, through the redactor?
- Does the no-names test still seed every new kind of name the report could carry?
- Does a new recorded step read anything a user types into a prompt? It must not.
