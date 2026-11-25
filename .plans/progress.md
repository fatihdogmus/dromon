# Dromon: progress

Tracks where implementation stands against `dromon-plan.md`. Newest first.

## 2026-11-25 · M0 spikes (Linux) + UI shell

**Workspace**: Rune pinned at `241d08d1d8dca18250c44f96fb3a99683d1351ea` with its `[patch.crates-io]` block; toolchain 1.99.1. Crates per §4.2.

| Spike | Status | Evidence |
|---|---|---|
| S1 Rune pinned build + headless `--snap` | ✅ Linux · ⏳ macOS, Windows | `cargo build -p dromon-app`; `scripts/goldens.sh` |
| S2 `ssh -s host sftp` + russh-sftp over child stdio | ✅ Linux · ⏳ macOS, Windows | `cargo run -p dromon-sftp --example s2_list`; `crates/dromon-sftp/tests/fixture.rs` |
| S3 Askpass round trip | ✅ Linux (host key, password, wrong password, passphrase, confirm, ProxyJump) · ⏳ Windows inbox OpenSSH | `crates/dromon-ssh/tests/askpass_e2e.rs`; helper start ≈ 2–4 ms |
| S4 `copy-data` via russh-sftp | ✅ `RawSftpSession::extended`; 1 GiB in ~1.7 s vs ~15 s streaming | `fixture.rs` |
| S5 Rune features (menus, clipboard files, drop, fonts, badge, notify) | ◐ fonts + tnum + titlebar done; rest pending | |
| S6 wake / network-change signal | ⏳ | |

**Done**
- `dromon-core`: `RemotePath`, `Entry`/`EntryKind`, natural sort, `sort_entries`, `HostSpec`, `PromptKind` (incl. `Confirm`, `Notice`), errors.
- `dromon-ssh`: config discovery (Include/globs/patterns), `ssh -G` parsing, command builder with injection guards, spawn, prompt classifier, askpass IPC (Unix socket, per-connection tokens), memory-only `PasswordStore` keyed by `user@host` identity, `AuthAttempt`, backoff.
- `dromon-sftp`: `Sftp` over `RawSftpSession` (capabilities, limits, paged listing, pipelined `read_at`/`write_at`, posix-rename, copy-data + streaming fallback), listing diff, adaptive poll schedule.
- `dromon-transfer`: Chrome-style naming, part-file names, task state machine, speed EMA/ETA, formatting.
- `dromon-app`: theme tokens (§6.3), Inter + Geist Mono bundled, sidebar/toolbar/list/status, virtualized rows, navigation/history/selection/sort/filter/hidden toggle, 10 scenes with goldens in `tests/goldens/`, mock `Backend`.
- Docker fixture `tests/docker/` (Debian trixie, OpenSSH 10.0p2 with PAM; users `key`, `pw`, `kbd` keyboard-interactive password + TOTP, `keypw` publickey,password, `enc`), run by `crates/dromon-testkit` (testcontainers, one container per test binary, removed on exit): plain `cargo test --workspace` runs the fixture tests when Docker is reachable and skips them otherwise (`DROMON_REQUIRE_DOCKER=1` fails instead); the 1 GiB S4 test is `--ignored`. See `tests/docker/README.md`.

**Since then**
- Icons are pixel-crisp: each has a 1× body (1 pt strokes on half-pixels) and a 2× body (1.5 pt on quarter-pixels), checked by a pixel test (`crates/dromon-app/src/icons/tests.rs`).
- The Docker fixture runs through testcontainers (`dromon-testkit`) with a PAM image; see the "Done" entry above.

- Service thread (§4.1): `dromon-service` thread with a tokio runtime, started from `on_ready`; `ChannelBackend` → per-host tasks → `AppHandle::update_if`. Hosts discovered from ssh config (+ `ssh -G`), askpass server with per-attempt tokens, `AuthAttempt` routing, paged listings, explicit connection state machine (`service/lifecycle.rs`) with silent reconnect and backoff. `main()` runs the askpass helper before any UI. `DROMON_MOCK=1` runs on mock data.
- Sheets (§6.8): password/passphrase/other, host key, confirm, notice toasts, connection log (2,000 lines per host). Scenes `auth-password`, `auth-password-error`, `host-key`, `confirm`, `connection-log`.

**Next**
1. App-level end-to-end tests against the testkit fixture (service + real askpass helper binary), and a first run in a real window on macOS/Linux desktop.
2. Persistence (last host/folder, sort, sidebar width).
3. M2: transfer engine, downloads, uploads, transfers popover.

**Service-thread caveats**
- Never run in a real window yet (this dev box has no display): `on_ready`, `update_if`, sheet focus and motion are only tested headless.
- Quit is best-effort: Rune may exit before the service thread clears the password store (process memory goes away regardless).
- A second prompt queues silently behind an answered sheet; the reconnect banner's "Sign in" button does nothing yet.

**Known issues / decisions to revisit**
- Linux: Rune's `Titlebar` keeps the native frame, so a real window shows a system header above the toolbar. (Accepted.)
- No keyboard focus ring on rows yet (selection cursor stands in). (Later.)
- Keyboard-interactive is tested end to end against PAM (`askpass_e2e.rs`): `(kbd@host) Password: ` classifies as `Password` (remembered), `(kbd@host) Verification code: ` as `Other` (never remembered). A wrong code makes the server restart the round at the password; since `AuthAttempt` can't tell which answer failed, that retry shows `WRONG_SIGN_IN` ("Check your password and code") and asks for both again.
- Passphrase prompts cut key paths at 100 bytes (ssh's own `%.100s`).
- Task state: `Paused` from `Preparing` resumes to `Running`; the engine must remember whether the scan finished.
