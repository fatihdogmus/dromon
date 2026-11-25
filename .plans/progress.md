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
- Docker fixture `tests/docker/` (users `key`, `pw`, `kbd`, `enc`); run with `bash tests/docker/setup.sh && DROMON_SSH_FIXTURE=1 cargo test --workspace`.

**Next**
1. Service thread (§4.1): tokio runtime via `rune::task::serve`, real `Backend` for `state::service::{Command, ServiceEvent}`, askpass server + auth sheets, host discovery feeding the sidebar, `main()` helper-mode switch.
2. Host-key and password sheets (§6.8), connection log.
3. Persistence (last host/folder, sort, sidebar width).

**Known issues / decisions to revisit**
- Icons: 1.5 pt strokes on half-pixels are soft at 2×; quarter-pixel centring would be crisp (`crates/dromon-app/src/icons/mod.rs`).
- Linux: Rune's `Titlebar` keeps the native frame, so a real window shows a system header above the toolbar.
- No keyboard focus ring on rows yet (selection cursor stands in).
- Keyboard-interactive (`PromptKind::Other`) is unit-tested only: Alpine sshd has no PAM.
- Passphrase prompts cut key paths at 100 bytes (ssh's own `%.100s`).
- Task state: `Paused` from `Preparing` resumes to `Running`; the engine must remember whether the scan finished.
