# Dromon: progress

Tracks where implementation stands against `dromon-plan.md`. Newest first.

## 2026-11-25 · M0 spikes (Linux) + UI shell

**Workspace**: Rune pinned at `241d08d1d8dca18250c44f96fb3a99683d1351ea` with its `[patch.crates-io]` block; toolchain 1.99.1. Crates per §4.2.

| Spike | Status | Evidence |
|---|---|---|
| S1 Rune pinned build + headless `--snap` | ✅ Linux · builds and tests on macOS, Windows in CI; headless render there non-blocking (passed once on Windows) | `cargo build -p dromon-app`; `scripts/goldens.sh` |
| S2 `ssh -s host sftp` + russh-sftp over child stdio | ✅ Linux, macOS (Apple's sshd), Windows (inbox `ssh.exe`) in CI | `crates/dromon-sftp/tests/fixture.rs`, `crates/dromon-transfer/tests/io.rs` |
| S3 Askpass round trip | ✅ Linux, macOS, Windows in CI with each OS's own ssh: host key, password, wrong password, passphrase, keyboard-interactive + TOTP (not macOS: no TOTP PAM there), ProxyJump, publickey,password; confirm prompts Linux/macOS only. Windows: named pipe `\\.\pipe\dromon-askpass-<pid>-<random>`, user-only DACL, client-user check | `crates/dromon-ssh/tests/askpass_e2e.rs`, `askpass_transport.rs`; helper start ≈ 2–6 ms |
| S4 `copy-data` via russh-sftp | ✅ `RawSftpSession::extended`; 1 GiB in ~1.7 s vs ~15 s streaming | `fixture.rs` |
| S5 Rune features (menus, clipboard files, drop, fonts, badge, notify) | ✅ fonts, tnum, titlebar; native popup menus (drawn on Linux); clipboard files; file drop; Dock badge / taskbar (`WindowHandle::set_taskbar`; Windows' bar is indeterminate: Rune's `TaskbarProgress` has no fraction); notifications (`rune-notify`) — headless-tested, real OS pending | `state/notify.rs`, `views/transfers.rs` |
| S6 wake / network-change signal | ✅ fallback: Rune has no wake/network events, so the service detects sleep (wall clock past monotonic, or a tick > 10 s late) and probes every live session (`realpath .`, 5 s) on wake and on window focus (≤ every 30 s per host); waiting reconnects retry at once. Verified in CI on all three OSes with a SIGSTOPped server; a real sleep/resume per OS is still to confirm | `service/wake.rs`, `service/conn_checks.rs`, `tests/wake_e2e.rs` |

**Done**
- `dromon-core`: `RemotePath`, `Entry`/`EntryKind`, natural sort, `sort_entries`, `HostSpec`, `PromptKind` (incl. `Confirm`, `Notice`), errors.
- `dromon-ssh`: config discovery (Include/globs/patterns), `ssh -G` parsing, command builder with injection guards, spawn, prompt classifier, askpass IPC (Unix socket, per-connection tokens), memory-only `PasswordStore` keyed by `user@host` identity, `AuthAttempt`, backoff.
- `dromon-sftp`: `Sftp` over `RawSftpSession` (capabilities, limits, paged listing, pipelined `read_at`/`write_at`, posix-rename, copy-data + streaming fallback), listing diff, adaptive poll schedule.
- `dromon-transfer`: Chrome-style naming, part-file names, task state machine, speed EMA/ETA, formatting.
- `dromon-app`: theme tokens (§6.3), Inter + Geist Mono bundled, sidebar/toolbar/list/status, virtualized rows, navigation/history/selection/sort/filter/hidden toggle, 10 scenes with goldens in `tests/goldens/`, mock `Backend`.
- Docker fixture `tests/docker/` (Debian trixie, OpenSSH 10.0p2 with PAM; users `key`, `pw`, `kbd` keyboard-interactive password + TOTP, `keypw` publickey,password, `enc`), run by `crates/dromon-testkit` (testcontainers, one container per test binary, removed on exit): plain `cargo test --workspace` runs the fixture tests when Docker is reachable and skips them otherwise (`DROMON_REQUIRE_DOCKER=1` fails instead); the 1 GiB S4 test is `--ignored`. See `docs/test-servers.md`.

**Tooling**
- `dromon --fixture <file.json>` (`crates/dromon-app/src/fixture/`, format in `docs/fixtures.md`): the real app on a data-driven fake backend (hosts, folder trees with errors and generators, scripted sign-in steps through the service's own lifecycle machine and `AuthAttempt`). No service thread, ssh or ssh config. The old mock is now the built-in demo fixture (`assets/fixtures/demo.json`, `DROMON_MOCK=1`); goldens unchanged.
- `scripts/flows.sh` renders `tests/flows/*` (strip, tree, video with ffmpeg); `scripts/smoke-app.sh` (headless, `--window`, `--connect`) replaces `smoke-xvfb.sh`; `scripts/repro.sh` turns an issue (`.github/ISSUE_TEMPLATE/bug.yml`, example `docs/examples/issue-repro.md`) into headless output and `summary.md`.
- `scripts/dev.sh [--mock]`: live editing through Rune's launcher (`rune-dev --local`, Dioxus `dx` 0.7.10 fetched and SHA-256-checked into `target/tools`). Function-body edits in `src/views/**` and `src/theme/**` patch the running app in about 2 s, with its state kept; other edits rebuild and restart it. It runs `dromon-hot` (`--features hot`, `src/hot.rs`), the app compiled as one crate: subsecond patches only the executable's own crate, so `lib.rs` and `hot.rs` both `include!("app_root.rs")`. The fonts moved to `crates/dromon-app/assets/fonts/`, since the launcher mirrors only the crates. Known gap in Rune's launcher: a non-body edit inside a patched file prints "restart needed" and stalls until rerun.

**Since then**
- Icons are pixel-crisp: each has a 1× body (1 pt strokes on half-pixels) and a 2× body (1.5 pt on quarter-pixels), checked by a pixel test (`crates/dromon-app/src/icons/tests.rs`).
- The Docker fixture runs through testcontainers (`dromon-testkit`) with a PAM image; see the "Done" entry above.

- Service thread (§4.1): `dromon-service` thread with a tokio runtime, started from `on_ready`; `ChannelBackend` → per-host tasks → `AppHandle::update_if`. Hosts discovered from ssh config (+ `ssh -G`), askpass server with per-attempt tokens, `AuthAttempt` routing, paged listings, explicit connection state machine (`service/lifecycle.rs`) with silent reconnect and backoff. `main()` runs the askpass helper before any UI. `DROMON_MOCK=1` runs on the built-in demo fixture.
- Sheets (§6.8): password/passphrase/other, host key, confirm, notice toasts, connection log (2,000 lines per host). Scenes `auth-password`, `auth-password-error`, `host-key`, `confirm`, `connection-log`.

- Persistence (`state/prefs.rs`, `persist.rs`): sort, dotfiles shown, sidebar width and the last folder per host (200 hosts, least recently used dropped) in Rune's `prefs.json` (`~/.config/dromon`, `~/Library/Application Support/dromon`, `%APPDATA%\dromon`). Read before the UI starts; written by a `dromon-prefs` thread (newest snapshot only), flushed on quit. A host opens in its remembered folder; if that fails to list, home replaces it once, without a history entry. Headless runs, scenes, fixtures and tests neither read nor write it. Window frames aren't remembered yet (Rune keeps them only within a session). Nothing reconnects at launch (§3.1), so the last host isn't stored.
- M2 transfer engine, verified end to end against the Docker sshd (`crates/dromon-app/tests/transfers_e2e.rs`: downloads with bytes and mtimes, "name (1)" on a second download, file and folder-tree uploads, an upload clash answered Keep both, a 64 MiB download that resumes after the server kills the session):
  - `dromon-transfer::queue`: pure model of jobs, items and files; per-host scheduler (3 files); upload clashes one at a time with apply-to-all; pause/resume/cancel/retry keep progress; a dropped host parks running files.
  - `dromon-transfer::io`: depth-first scans (links to folders, specials and unreadable folders skipped with reasons), pipelined download/upload into part files (16 requests in flight), resume when the source's size and mtime are unchanged, mtime kept both ways, atomic finish (`posix-rename` for Replace, a free name when the target appeared meanwhile). Local Docker: 89–108 % of scp down, 112–124 % up.
  - `service/transfers.rs`: the engine task on the service thread; connection tasks hand it each host's SFTP session on open/reconnect and tell it when the session goes; snapshots to the UI at most every 100 ms.
  - A download fetches the source as it is when the copy starts (a retry after "changed on the server" works); only a change during the copy fails it.
  - Open issues: a cancelled download restarted at once against a changed source can, rarely, get the cancelled copy's last chunk written into the new part; upload resume trusts that the server applied writes in order (true for OpenSSH); no posix-rename fallback test (the fixture has it); Windows and macOS untested.

- M2 UI (transfers, on the app state of `state/transfers.rs`):
  - Works: the toolbar transfers button (`views/transfers.rs`: arrow in a 2 pt ring of the active jobs' bytes, two clipped half rings turned on the `move` spring; count badge; quiet glyph until the session has a job); the popover (380 × ≤480, anchored, modal, Downloads / Uploads with counts, rows with icon, 3 pt bar, `progress_line` or the state's text, pause/resume/cancel/retry on hover, finished rows reveal, errors/skipped as a tooltip, Clear finished, empty tabs); the conflict sheet (`views/conflict.rs`: Replace (danger) / Skip / Keep both (default), "Apply to all N conflicts"); ⌘D (Ctrl+D); file drops on the list or a folder row with outline, row lift and pill (`views/drop.rs`, `Action::DropHover`); ⌘V with files (root `on_paste_content`). The list's shortcuts don't reach behind the conflict sheet. While the popover is open a clash waits in its row ("Resolve…" closes it and the sheet comes up).
  - Faked: fixtures run transfers on the real `Queue` with instant fake I/O (`fixture/transfers.rs`): downloads name against an in-memory set, uploads clash against the tree and land in it (the folder on screen relists); a fixture's `transfers` key adds still pictures of mid-progress jobs (`docs/fixtures.md`). Scenes `transfers`, `transfers-uploads`, `transfer-conflict`, `drop-list`, `drop-folder`, `toolbar-ring`; flows `upload-conflict`, `download`.
  - The UI runs on the real engine (`service/transfers.rs`) in the app; closing the window with transfers running asks "N transfers in progress" (Quit / Cancel, `views/quit.rs`, Rune's `on_close_request`; macOS ⌘Q and logout bypass it).
  - The conflict sheet shows the sizes and dates of both items (Existing / Incoming, "newer" in words).
  - Missing: the done flourish and the ring's pulse; ⌘⇧J; drag from a real Finder/Explorer window is untested (headless drags only); Dock badge, taskbar progress and notifications (S5).

- The sshd fixture tests run on all three OSes in CI, each with its own system OpenSSH client:
  - Linux: the Docker image (testcontainers).
  - Windows (`windows` job, after the plain suite): the same image on Docker in WSL2 (`windows-2025` has nested virtualisation; dockerd answers on tcp 2375 via a systemd drop-in, published ports reach Windows through WSL's localhost forwarding). Windows' own `ssh.exe` 9.5 with our named-pipe askpass passes every askpass e2e (host key, password, wrong password, passphrase, keyboard-interactive + TOTP, ProxyJump, publickey,password), SFTP and transfer test. Found on the way: Windows' `ssh-keyscan` gives up on sntrup761 (not used any more); the `docker` CLI over TCP returned no `exec` output (the testkit uses the Docker API); an askpass answer must end in `\n`, not `\r\n`.
  - macOS (`macos` job, after the plain suite): Apple's own `sshd` through the testkit's native backend (users `dromon_*` via `dscl`, root watchdog cleans up). Everything passes, the 3 TOTP tests included: Homebrew's `google-authenticator-libpam` loads into Apple's sshd (its OpenPAM clears library validation for sshd-session) through a PAM service of the kbd user's own (`PAMServiceName` in its Match block); OpenPAM asks `Password:` without Linux-PAM's trailing space (`Fixture::password_prompt`). Apple-silicon runners can't run Docker (no nested virtualisation).
  - S3 (askpass) is therefore verified with real prompts on Linux, macOS and Windows; S1/S2 (build, ssh + SFTP) on all three in CI.
  - The app's own end-to-end tests (`e2e.rs`, `transfers_e2e.rs`, `fileops_e2e.rs`: headless UI and the service against the real server) run on all three too, nothing skipped on Windows (key copies get a current-user-only ACL via `copy_private_key`, `NUL` for "no file", the host key's fingerprint read on the server).
  - Found by running them on macOS: work cut off by a dropped connection could wait forever when the reconnect was handled before the lost task's outcome (two channels, no order); outcomes now carry the sessions they used (`Sftp::same_session`) and run again at once on a replaced session.

- M3 backend:
  - File operations (`Command::FileOp`: Mkdir, Rename, Delete with progress; `Sftp::remove_all` counts first, never follows links), relisting the folder on screen; `state/ops.rs`.
  - Copies and moves as transfer jobs (`RemoteCopy`, `Move`, `CrossHostCopy`, `CrossHostMove`): copy-data or streaming, posix-rename with merge on Replace, cross-host move deletes a source only after verifying its copy, then tidies emptied folders.
  - Fixtures: one folder tree (`fixture/tree.rs`) for transfers and file operations.

- M3 UI: inline new folder (⌘⇧N) and rename (F2), delete sheet (Enter doesn't delete) and progress toast, copy path (⌥⌘C), clipboard (⌘C/⌘X/⌘V while the OS clipboard still holds the clip's paths; Finder files still upload) and ⌥⌘V, drag rows onto folders/breadcrumbs (move, ⌥/Ctrl copies) and hosts (copy), context menus (native via Rune where available, drawn otherwise). A job's end lists its folders on screen again (`JobView.folders`); ⌘R / F5 refresh. Needs a real window: native menu placement, ⌥⌘C/⌥⌘V reporting, Ctrl+Alt on Windows (AltGr).

- M4 (all items built; headless and CI-verified, real desktops pending):
  - Adaptive polling (§4.5) with diff animations and the vanished-folder climb; a job's end relists its folders.
  - Quick connect (paste `user@host:port`, More, file picker), Saved (`hosts.conf`), Recent, host menu, Add to SSH config with backup; ssh config files watched (2 s mtime poll).
  - ⌘K palette (fuzzy, Hosts / Go to folder… / Actions), ⌘L, ⌘1–9, ⌘⇧J.
  - Settings page (⌘,, the palette, the macOS app menu, the sidebar's gear) in the main window, in place of the toolbar, list and status bar, the sidebar beside it: Back / Escape (or picking a host) return to the folder view as it was. Sections Appearance (theme, density), Transfers (download folder, transfers at once, runtime), Browsing (hidden files by default) in a tablist; each window has its own page, a change reaches every window. No second window.
  - Several windows (⌘N / ⌘W): one service, per-window folders polled, prompts routed, shared clip and settings; frames restored.
  - Theme: contrast audit test (all pairs, states, both themes, three accents), reduce motion, increased contrast, reduce transparency (forced only: Rune can't read the OS setting), light goldens. Accent: Spearmint until the user picks (`scripts/accents.sh`).
  - Icons complete (audio, video, PDF, log).
  - Conflict sheet sizes and dates; Dock badge, taskbar, notifications, done flourish.
- Transfer engine robustness: work cut off by drops is replayed as itself and at once on a replaced session; runs tagged so a stale Finished can't start a second copy; stress test kills sessions at varying moments (`transfers_e2e`, ignored by default).
- M5 server matrix (`DROMON_SERVER_MATRIX=1`, CI linux): Debian trixie (OpenSSH 10), bullseye (8.4, no copy-data: streamed copies), Alpine 3.24, toxiproxy (latency, caps, reset mid-transfer → resume); 100 / 10k / 100k folders, symlink loops, unreadable folders. CI windows: the runner's own OpenSSH Server, `/C:/` paths, round trip, rename, copy, delete.

- Windows streaming: a copy streamed through one session (servers without copy-data) runs fine with Windows' `ssh.exe` against the runner's native sshd (5 × 16 MiB, ~1.5 s each, the app's 10 s timeout: `windows_sshd.rs`). The Docker fixture's path on Windows CI goes through WSL's localhost relay, which occasionally stalls such a connection (2 runs): a CI-only flake in `remote_io`'s streamed test, not the app. Fixture base images come from `mirror.gcr.io` (Docker Hub rate-limited CI with 429s).
- Launch (§12 "Restore window/sidebar/last host; never auto-connect"): the host opened last is saved (`Saved.last_host`) and offered on the welcome screen as "Connect to <host>"; nothing connects by itself.
- M5 performance (`scripts/perf.sh`, one table; this Linux box, release builds):
  - First rows of a 10k folder: 14 ms over the sshd (gate 150 ms) ✅
  - Throughput vs scp: 1 GiB 92 % down / 101 % up; 8,192 small files 153 % / 139 % ✅ (localhost, cipher-bound ~300 MB/s)
  - 10 GiB soak: SHA-1 both ways, RSS +5.6 / +11.3 MiB ✅
  - Scroll 100k rows, headless CPU per frame: p99 3.7 ms gentle ✅, 6.1 ms fast flick (over the 4.2 ms half-frame target, under 8.3 ms; the cost is Rune's commit and paint). The real gate (M-series Mac, 120 Hz) needs a Mac.
  - Fixed on the way: natural sort allocations, re-sorting per streamed page (merge), per-rebuild hashing, name copies per key press (Down-key frame 20.6 → 7.2 ms in 100k), the diff's maps; and a real bug: ↓/End/Page Down didn't scroll the list (the reveal was scoped inside the viewport).
  - Remaining hotspot (Rune): re-keying 100k virtual rows when the rows change (filter keystroke ~36 ms, refresh ~220 ms in 100k).

- M5 accessibility (§6.9): an audit test over every scene and the Settings page (AccessKit tree via `Runtime::accessibility_json`: role, name, action, keyboard reach, duplicates, images: 1079 offences → 0); the list is a grid with sortable column headers and an active-descendant rowgroup; Tab order per state with focus traps and focus return, a ring at every stop (`ui_keyboard_tests`); polite announcements for transfers, connections, deletes and the vanished folder. Shift+F10 / the Menu key open the context menu at the cursor row; the reconnect banner's Sign in shows the waiting prompt in this window (`Command::ShowPrompts`). Needs real screen readers (VoiceOver, Narrator, Orca); macOS Tab follows the system keyboard-navigation setting.

**Next**
1. The first release, 0.1.0: Actions → Release, `minor`, one approval (`release-approval`). The pipeline now bumps the version itself (`plan`: commit on `release/<version>`), tags plain semver (`0.1.0`), publishes the release, lands the version commit on main and calls homebrew.yml (docs/releasing.md). Proven before the rework (cancelled run 45784569998): every build, the MSI, Arch, macOS Developer ID signing and notarization (app and DMG stapled, Gatekeeper "Notarized Developer ID") and GPG signing; Windows ships unsigned (no Windows signing yet).
2. The accent pick (user).
3. A first run in a real window on macOS, Windows and a Linux desktop (real GPU, input, Wayland, screen readers, sleep/wake, notifications, the 120 Hz scroll gate).

**Service-thread caveats**
- Verified end to end (`crates/dromon-app/tests/e2e.rs`, Docker fixture): the real `App` in a headless `Runtime` + the real service thread + system `ssh` + the `dromon` binary as askpass helper + SFTP, driven by clicks, typing and Return: host-key sheet with the server's real fingerprint, listings incl. dir symlinks, dangling links, dotfiles and permission denied; wrong then right password; keyboard-interactive password + TOTP (code not rememberable); key passphrase; a server-side session kill reconnects silently (password from memory, no sheet, folder relisted); Disconnect forgets the password. No typed secret reaches any event or log line. `dromon-app` is now lib + bin for this (`app()`, `run()`, `pub mod state`, `pub mod service`).
- Verified under Xvfb + lavapipe (`scripts/smoke-app.sh --window` and `--connect`): the real windowed startup (`on_ready` → service thread → discovery → sidebar, `update_if`), and, through Rune's `--window-script`, a click that connects and lists in the real window. `DROMON_TRACE=1` names each applied event on stderr (kind + host only). Not yet on a real desktop or GPU; sheet focus and motion in a window are untested.
- `--window-script` starts only at a GPU-drawn frame; a window resting on its CPU-drawn first frame (Rune's Linux default) never starts it, so the connect run uses `RUNE_CPU_FIRST=0`.
- Discovery reads `$HOME/.ssh/config`, but ssh reads the password database's home: with `HOME` overridden the sidebar and ssh disagree (the smoke script wraps `ssh` with `-F`). Fixing it belongs in `dromon-ssh`'s `ConfigSources`.
- Quit is best-effort: Rune may exit before the service thread clears the password store (process memory goes away regardless).
- A second prompt queues silently behind an answered sheet; the reconnect banner's "Sign in" button does nothing yet.

**Known issues / decisions to revisit**
- Linux: Rune's `Titlebar` keeps the native frame, so a real window shows a system header above the toolbar. (Accepted.)
- No keyboard focus ring on rows yet (selection cursor stands in). (Later.)
- Keyboard-interactive is tested end to end against PAM (`askpass_e2e.rs`): `(kbd@host) Password: ` classifies as `Password` (remembered), `(kbd@host) Verification code: ` as `Other` (never remembered). A wrong code makes the server restart the round at the password; since `AuthAttempt` can't tell which answer failed, that retry shows `WRONG_SIGN_IN` ("Check your password and code") and asks for both again.
- Passphrase prompts cut key paths at 100 bytes (ssh's own `%.100s`).
- Task state: `Paused` from `Preparing` resumes to `Running` in `step`; the queue sets `Preparing` itself when the scan isn't finished (`queue.rs`, `resume`).
