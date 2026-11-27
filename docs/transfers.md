# SFTP and Transfers Guide

> Source: `transfers.md`
> Purpose: The SFTP session (dromon-sftp), the transfer engine (dromon-transfer), and how the app drives them on the service thread.

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

User-facing behaviour (what each paste or drag does, conflict sheets, the popover) is the plan,
`.plans/dromon-plan.md` §3.5–§3.7, §4.4–§4.6. Server-compatibility tests are
[testing.md](testing.md#reference-the-server-matrix).

## Index

- [Decision: One SFTP Session per Host](#decision-one-sftp-session-per-host)
- [Standard: Extensions Are Detected, Never Assumed](#standard-extensions-are-detected-never-assumed)
- [Reference: Listings, Diffs and Polling](#reference-listings-diffs-and-polling)
- [Decision: A Pure Queue and an I/O Layer](#decision-a-pure-queue-and-an-io-layer)
- [Decision: Part Files and Resume](#decision-part-files-and-resume)
- [Standard: Conflicts and Naming](#standard-conflicts-and-naming)
- [Reference: Copies and Moves](#reference-copies-and-moves)
- [Reference: Progress Maths](#reference-progress-maths)
- [Pattern: How the App Drives the Engine](#pattern-how-the-app-drives-the-engine)
- [Decision: Editing in a Local Editor Uploads Through the Engine's Write Path](#decision-editing-in-a-local-editor-uploads-through-the-engines-write-path)

---

## Decision: One SFTP Session per Host

**Status:** Accepted
**Scope:** `dromon-sftp/src/session.rs`, `dromon-app/src/service/conn.rs`

### Context

Each ssh process means one sign-in. Users sign in once per connection, so everything for a host
must share one ssh child.

### Decision

`Sftp` is one session over the ssh child's stdin and stdout, built on russh-sftp's
`RawSftpSession` (not its high-level `SftpSession`, which hides the extension list and raw
`extended` and `readdir` calls). Every listing, operation and transfer for a host is multiplexed
over it.

### Rules and examples

- The connection task owns the session and hands clones to the transfer engine
  (`EngineMsg::HostUp`) and to file operations; `Sftp::same_session` tells sessions apart after
  a reconnect.
- The version handshake has no timeout (ssh may be waiting on a prompt); every request after it
  has a reply timeout (`Sftp::set_timeout`, 10 s by default), so every request ends and every
  task reports back.
- Errors map onto `dromon_core::Error` (`dromon-sftp/src/error.rs`); `Error::ConnectionLost`
  means "interrupted, resume later", not "failed".
- Server data never panics the app: a malformed reply is an `Error`, not an `unwrap`.

### Consequences

A slow transfer and a listing share one channel; reads and writes pipeline instead of opening
more sessions.

### Agent checklist

- Does new work reuse the host's session rather than open another? (The live updates' watch is
  the one exception: a plain ssh command, no SFTP, that signs in only from memory.)
- Does every new request end with a reply, an error or the timeout?

## Standard: Extensions Are Detected, Never Assumed

**Scope:** `dromon-sftp` (`Capabilities`, `copy_data.rs`), every caller

### Purpose

Work on every server in the matrix: OpenSSH 8.4 to 10, busybox, Windows OpenSSH.

### Rules

- `Capabilities::from_extensions` reads what the server advertised: `posix-rename@openssh.com`,
  `limits@openssh.com`, `copy-data`.
- Chunk size comes from `limits` (`Capabilities::chunk_size`), capped at `MAX_CHUNK`; without it
  `DEFAULT_CHUNK` (32 KiB).
- `copy-data` (same-host server-side copy) is an `SSH_FXP_EXTENDED` request encoded in
  `copy_data.rs`; without it the copy streams through the app.
- Paths are `RemotePath`, never `std::path` (Windows servers use `/C:/…`).
- Symlinks to folders are never followed in recursive work (scans, deletes): no loops.

### Agent checklist

- Is there a fallback for every extension the change uses?
- Is the fallback covered by the `Bullseye` matrix variant?

## Reference: Listings, Diffs and Polling

**Scope:** `dromon-sftp` (`list_paged`, `diff.rs`, `poll.rs`), `dromon-app/src/service/{poll.rs, live.rs, conn_live.rs}`

### Use for

Changing how folders load and refresh.

### Reference

- Listings stream in pages (`Sftp::list_paged`; the app uses 512 entries, `service::conn::PAGE`),
  so the first rows paint before the folder is read.
- `diff(old, new)` compares by name, then `(kind, size, mtime)`, into inserted, removed and
  changed sets for the row animations.
- `PollSchedule` is pure: `base_interval(entries, Visibility)` by folder size and focus,
  doubled while visible but unfocused, none above 20,000 entries, backing off (up to
  `PollSchedule::MAX_FACTOR`) while polls take more than a quarter of the interval. The caller
  sleeps for `next_delay`.
- The app polls only folders on screen in a visible window (`service/poll.rs`, `service/windows.rs`),
  one listing per host at a time; a navigation replaces a poll; a poll that finds nothing new
  sends nothing.
- Live updates (`service/live.rs`, wired in `service/conn_live.rs`): on servers with
  `inotifywait`, a second ssh session per host watches one folder, the focused (else a visible)
  window's on that host (`Watch::live_folder`), not its subfolders. Each change it reports
  starts the same poll a schedule would, `live::DEBOUNCE` (150 ms) after the first change it
  hasn't covered, so diffs, animations and the one-listing rule are unchanged. While a folder is
  watched its own polls stretch `LIVE_STRETCH` (×4) as a safety net for what inotify misses (NFS,
  a full event queue).
- The watch is a pure machine (`live::Phase`, `live::step`): it moves with the folder on screen,
  stops while no window shows the host, the session is down or the setting ("Live updates when
  the server supports them", `Command::LiveUpdates`) is off. No `inotifywait`, a sign-in only the
  user could answer, or a watch that never starts make the host `Unavailable` until its session
  comes up again; a watch that ends later leaves its folder to normal polls until another folder
  is shown. The windows hear `ServiceEvent::Live`; the status bar shows "Live", or says why not on
  the connection's tooltip.
- The fixture backend has no live updates: `Command::LiveUpdates` changes nothing there.

### Agent note

Selection and scroll never move because of a poll; that rule lives in `state/refresh.rs` and its
tests.

## Decision: A Pure Queue and an I/O Layer

**Status:** Accepted
**Scope:** `dromon-transfer` (`queue.rs`, `state.rs`, `io/`), `dromon-app/src/service/transfers.rs`

### Context

Transfers combine scheduling, conflicts, pauses, reconnects and real I/O. Mixing them makes
every bug a timing bug.

### Decision

The engine is three parts:

- `queue::Queue`: the pure model of every job, item and file, with the per-host scheduler. No
  I/O and no clock. The engine reports facts (`named`, `exists`, `scanned`, progress, a host
  going down); actions (`queue::Action`: `Name`, `FreeRemoteName`, `Scan`, `Start`, `Abort`,
  `Cleanup`, `Tidy`) come out of `Queue::poll`, `pause` and `cancel` only.
- `io`: scans, single-file copies and moves over SFTP and the local disk.
- The engine task on the service thread (`service/transfers.rs`), which runs each action as a
  tokio task and feeds the result back.

### Rules and examples

- A job's request is a `JobRequest` (`Download`, `Upload`, `RemoteCopy`, `Move`,
  `CrossHostCopy`, `CrossHostMove`); a scan turns each picked item into `DirPlan`s and
  `FilePlan`s with final destinations, so conflicts are settled before any byte moves.
- Task states (`TaskState`: queued, preparing, running, paused, done, failed, cancelled) move
  only through the pure `dromon_transfer::step`.
- At most `per_host` files run per host (default `PER_HOST` = 3, Settings' "Transfers at once").
  A cross-host job needs both hosts up and counts against both limits.
- A dropped host aborts its running files (part files stay) and parks the job; naming, scanning
  and tidying work it cut off is replayed as the same action once the hosts are back.
- The fixture backend drives the same `Queue` with instant fake I/O (`fixture/transfers.rs`),
  so UI tests see real scheduling.

### Consequences

Scheduling and conflict rules are unit-tested in `queue_tests.rs` without a server; I/O is tested
against the sshd fixture (`dromon-transfer/tests/io.rs`, `remote_io.rs`, `transfers_e2e.rs`).

### Agent checklist

- Is new scheduling logic in `Queue`, tested without I/O?
- Does a new kind of work survive a host going down mid-way (replayed or resumed)?

## Decision: Part Files and Resume

**Status:** Accepted
**Scope:** `dromon-transfer/src/io/`, `naming.rs`

### Context

Agents and web servers must never see half-written files, and an interrupted transfer must not
start over.

### Decision

Every copy writes a part file next to its destination and renames it into place when complete:
downloads `<name>.dromon-part` (`download_part_name`), uploads and remote copies
`.<name>.dromon-part` (`upload_part_name`, hidden on the server), renamed with `posix-rename`
(atomic replace for Replace).

### Rules and examples

- Bytes are appended strictly in offset order, so a part's length is always a verified prefix of
  the source. Requests pipeline (`CopyOptions::in_flight`, 16 by default).
- Resume continues from the part's length only when the source still has the planned size and
  mtime; otherwise it starts over.
- The engine pauses or cancels by dropping a copy's future. At every await point the
  destination holds either nothing of ours or the finished file. Cancel deletes the part
  (`Action::Cleanup`); pause keeps it.
- mtime is kept both ways.
- `is_part_file` recognises leftovers.
- A refused write or rename of a remote part names the destination folder, not the hidden part
  (`io::cant_write_in`: "You don't have permission to write in /srv/x"); each failed file is also
  a line in its host's Connection log ("Upload failed: a.txt: …").

### Consequences

A copy is safe to drop at any point. Code that writes to a destination outside the part file
breaks this and is a bug.

### Agent checklist

- Does the change still write only to the part file until the final rename?
- Is resume still conditional on the source's size and mtime?

## Standard: Conflicts and Naming

**Scope:** `naming.rs`, `queue.rs`, `views/conflict.rs`

### Purpose

No transfer silently overwrites anything.

### Rules

- Downloads never ask: `unique_name` picks a free Chrome-style name (`report (1).pdf`; `.tar.gz`
  and friends kept whole; never loops forever), checked against the disk and names the queue has
  reserved (`Queue::is_reserved_local`). The name is re-checked at the end.
- Uploads, copies and moves into an existing name ask: `ConflictChoice::Replace` (a folder
  merges), `KeepBoth` (a free remote name), `Skip`, with apply-to-all. One clash shows at a time.
- A file that appeared at the destination after the scan gets a free name unless the user chose
  Replace (`FilePlan::Upload { overwrite }`).

### Agent checklist

- Can the change overwrite anything the user didn't choose to replace?
- Are naming edge cases covered in the naming tests?

## Reference: Copies and Moves

**Scope:** `dromon-transfer/src/io/{copy.rs, moves.rs}`

### Use for

Knowing what each job kind does on the server.

### Reference

| Job | How |
| --- | --- |
| `RemoteCopy` | `copy-data` when the server has it (always from the start: it is fast), else streamed through the app on the one session |
| `Move` | One `posix-rename` per picked item; Replace on a folder merges child by child; each step atomic, so a move cut off mid-way leaves each item in one place |
| `CrossHostCopy` | Streamed: reads pipelined on the source's session, writes on the destination's, in offset order |
| `CrossHostMove` | The copy, then `delete_verified`: the source is deleted only when the destination has the copied size and the source is unchanged; `remove_empty_dirs` tidies emptied folders afterwards |

### Agent note

Never delete a source before its copy is verified, and never follow a link to a folder when
moving or deleting recursively (`Sftp::remove_all` doesn't).

## Reference: Progress Maths

**Scope:** `dromon-transfer/src/{progress.rs, format.rs}`

### Use for

Speeds, times and the toolbar ring.

### Reference

- `SpeedMeter`: an exponential moving average of bytes per second with a ~3 s time constant,
  correct at irregular sample intervals; reset on pause and reconnect.
- `eta`: `None` before 2 s of data or with no usable speed.
- `Progress::fraction` clamps to 0–1; `aggregate` sums jobs for the ring (saturating).
- `format.rs`: decimal SI units, ASCII digits, `·` separators, rounded times ("about 2 min").
  `progress_line` is the popover's text.

### Agent note

Pure and clock-free: the caller passes time in, so all of it is unit-tested.

## Pattern: How the App Drives the Engine

**Scope:** `state/transfers.rs`, `state/service.rs`, `service/transfers.rs`

### Use when

Adding a transfer action or showing more of a job.

### Pattern

1. The view turns input into an `Action` (`Download`, `Upload`, `Transfer(...)`,
   `ResolveConflict`, a paste or drag).
2. State builds a `JobRequest` and sends `Command::Transfer(TransferCommand::Start(request))`;
   pause, resume, cancel, retry, resolve, clear finished and `PerHost` are the other
   `TransferCommand`s.
3. The engine task updates the `Queue`, runs its actions, and sends
   `ServiceEvent::Transfers(Vec<JobView>)` when something changed, at most every `EMIT_EVERY`
   (100 ms).
4. `state::Transfers` replaces its mirror with each snapshot; a job that finished changing a
   folder on screen relists it; the Dock badge, taskbar and notifications come from the leading
   window's `Watch` ([architecture.md](architecture.md#decision-several-main-windows-one-service)).

### Agent checklist

- Does the UI only mirror `JobView`s, never keep its own job state?
- Does the fixture engine handle the new command too?

## Decision: Editing in a Local Editor Uploads Through the Engine's Write Path

**Status:** Accepted
**Scope:** `dromon-app/src/service/remote_edit.rs`, `state/remote_edit.rs`, `fixture/remote_edit.rs`

### Context

Edit (a text file's row menu, the palette, ⌘E) downloads a file into a local copy that the
user's own app opens, and every save there goes back to the server. A save must never leave a
half-written file on the server, and must not silently overwrite a change someone else made
there meanwhile.

### Decision

Each edit is a task of its host's connection (`Edits` in `conn.rs`) that uses
`dromon_transfer::io::download` and `io::upload` directly, on the host's session: the same part
files and atomic rename as every transfer, but no queue job. They run quietly: a job per save
would flood the transfers list, and the queue's name-clash check isn't the question an edit
asks. The transfers popover's Editing section shows each edit instead.

### Rules and examples

- **Saves.** The local copy's modification time and size are looked at every second
  (`WATCH_EVERY`); a change that held still for a whole look is a save (pure `Watch`), so an
  editor's write-a-temporary-file-and-rename counts once.
- **The conflict check.** Before each upload the server's copy is `lstat`ed; if its size or
  mtime differs from what the edit last downloaded or uploaded (or it's gone), nothing uploads
  and the UI asks (`RemoteEditEvent::Conflict`): **Replace** (upload with `overwrite`),
  **Keep both** (upload without it: `upload`'s own free name, `notes (1).txt`, from
  `unique_name`), **Discard my changes** (download the server's copy into a staging folder,
  then rename it over the local copy).
- After Keep both the edit stays on its original path, whose server copy is still the newer
  one, so the next save asks again: no save ever replaces a change the user hasn't seen.
- An upload never resumes from a part file (an earlier save's part holds other bytes); a
  download of the server's copy always starts over too.
- After an upload to the edit's path, the server's copy (stat again) is the next base.
- An upload cut off by a lost connection is tried again when the host's session is back; any
  other failure keeps the local copy and waits for Retry or the next save.
- The fixture saves every edit of a host when a window gains focus (`Command::Poll`), checking
  the tree as the service checks the server ([fixtures.md](fixtures.md)).

### Consequences

Edit uploads don't count against "Transfers at once" and show no progress bar, only how each
edit stands. Where the local copies live and how they're cleaned up is
[security.md](security.md#decision-local-copies-of-files-being-edited).

### Agent checklist

- Does a change to the edit path still write only through `io::upload`/`io::download`?
- Is every upload preceded by the conflict check (or an explicit Replace/Keep both)?
