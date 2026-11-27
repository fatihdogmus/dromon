# Test Servers Guide

> Source: `test-servers.md`
> Purpose: The sshd fixture behind the integration and end-to-end tests: the image, users and roles, the Docker, WSL2 and native backends, TOTP, the server variants of the matrix, Windows' own sshd, environment variables and debugging.

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

How to write a test against the fixture (roles, skips, isolation) is
[testing.md](testing.md#standard-end-to-end-tests-against-the-sshd-fixture). This guide is how
the fixture itself works: read it before changing `tests/docker/` or `crates/dromon-testkit`.

## Index

- [Decision: One Server per Test Binary](#decision-one-server-per-test-binary)
- [Reference: Users, Roles and Seed Data](#reference-users-roles-and-seed-data)
- [Reference: The Docker Backend](#reference-the-docker-backend)
- [Reference: Docker in WSL2 on Windows](#reference-docker-in-wsl2-on-windows)
- [Standard: The Native Backend](#standard-the-native-backend)
- [Reference: TOTP for the kbd User](#reference-totp-for-the-kbd-user)
- [Reference: Server Variants and the Matrix](#reference-server-variants-and-the-matrix)
- [Reference: Windows OpenSSH Server](#reference-windows-openssh-server)
- [Reference: Environment Variables](#reference-environment-variables)
- [Recipe: Run the Native Backend in a Container](#recipe-run-the-native-backend-in-a-container)
- [Recipe: Run the Fixture Tests on Windows](#recipe-run-the-fixture-tests-on-windows)
- [Runbook: Debug a Fixture Test](#runbook-debug-a-fixture-test)

---

## Decision: One Server per Test Binary

**Status:** Accepted
**Scope:** `crates/dromon-testkit`

### Context

Integration tests need a real OpenSSH server with known users, keys and data, on every OS CI
runs, without a setup step and without leaving servers behind.

### Decision

`dromon_testkit::fixture(test)` starts one server per test process (test binary) on first use,
shared by all its tests, and ends it when the process exits. Each process generates its own two
ed25519 client keypairs with `ssh-keygen` into a fresh `$TMPDIR/dromon-testkit-<pid>-…` folder
and checks that a public-key login works before the first test runs.

### Rules and examples

- No key is in any image: public keys are copied into the server before it starts.
- Statics are never dropped, so cleanup can't rely on `Drop`: the Docker backend holds a
  lifeline connection, the native backend a root watchdog (below). Both also clean up after a
  panic or `kill -9`.
- Key folders of processes that are gone are deleted on the next start.
- When the backend can't run here, `fixture` prints `skipping <test>: …` and returns `None`;
  with `DROMON_REQUIRE_DOCKER=1` it panics instead, for either backend (the name means "the
  fixture must run").
- When the server is up, the testkit prints `dromon-testkit: <variant> sshd fixture on
  127.0.0.1:<port>, <server banner>, …`; `scripts/smoke-app.sh --connect` reads the port from it.

### Consequences

`cargo test --workspace` needs no setup: with Docker it runs the fixture tests, without it they
skip. A test binary pays the server's start once.

### Agent checklist

- Does a testkit change keep cleanup working on panic and `kill -9`?
- Does it keep the skip line and the `DROMON_REQUIRE_DOCKER=1` failure?

## Reference: Users, Roles and Seed Data

**Scope:** `tests/docker/` (`Dockerfile`, `sshd_config`, `pam-sshd`, `seed.sh`), `dromon_testkit::Role`

### Use for

Picking the user a test signs in as.

### Reference

| `Role` | Docker user | Native user | Signs in with |
| --- | --- | --- | --- |
| `Key` | `key` | `dromon_key` | public key only; owns the seed data |
| `Password` | `pw` | `dromon_pw` | the SSH `password` method only (`PASSWORD`) |
| `Kbd` | `kbd` | `dromon_kbd` | keyboard-interactive through PAM: password, then a TOTP code where `Feature::Totp` |
| `KeyThenPassword` | `keypw` | `dromon_keypw` | public key, then password (`publickey,password`) |
| `EncryptedKey` | `enc` | `dromon_enc` | public key with an encrypted private key (`PASSPHRASE`) |

- Docker homes are `/home/<user>`; native homes are `/Users/dromon_*` (macOS) or
  `/home/dromon_*` (Linux). Tests take names and homes from `Fixture::user`, `Fixture::home`,
  `Fixture::path` and `Fixture::destination`, never literals.
- `seed.sh` (run as the key user at start) makes `fixture/mixed` (about 100 entries: files,
  folders, links to a folder, a file and nothing, dotfiles, a FIFO, an unreadable folder) and
  `fixture/many` (10,000 entries). `seed.sh <home>` seeds another home (the native backend's).
- The 1 GiB `fixture/big/big.bin` is made over ssh on first use, by the `--ignored` tests only.
- Passwords, the passphrase and the TOTP secret are fixed test values; ports are published on
  127.0.0.1 only.

### Agent note

How each user logs in is decided by the `Match` blocks in `tests/docker/sshd_config` and by
`pam-sshd`; the native backend reuses those blocks, so a change there changes both backends.

## Reference: The Docker Backend

**Scope:** `crates/dromon-testkit/src/docker.rs`

### Use for

Knowing what happens when a test asks for the default server.

### Reference

1. Docker must answer a ping; otherwise the skip line.
2. The image is built from `tests/docker/Dockerfile` and the files the testkit lists for it
   (`DEBIAN_FILES`: `sshd_config`, `pam-sshd`, `google_authenticator`, `entrypoint.sh`,
   `seed.sh`), tagged `dromon-testkit-sshd:<hash of the build context>`. The build is skipped
   while that tag exists, so it happens once per change. Old tags stay:
   `docker image rm $(docker image ls -q dromon-testkit-sshd)`.
3. The container starts with ports 22 (sshd) and 9999 (lifeline) published on 127.0.0.1 at
   random host ports; the testkit waits for sshd's `Server listening on` line.
4. The test process holds a TCP connection to 9999 for its whole life. When the process exits,
   the kernel closes it, the entrypoint stops sshd, and Docker removes the container
   (`AutoRemove`). A container whose lifeline never connects ends after 120 s.
5. `docker ps -a --filter label=dev.dromon.testkit.pid` lists fixture containers.

Base images come through `mirror.gcr.io`, not Docker Hub directly, which rate-limits CI.
`Fixture::exec` runs a command in the container (the Docker API, not the `docker` CLI);
`kill_sessions` ends a user's sessions.

### Agent note

A new file the image needs goes into the Dockerfile and the testkit's file list together, or the
tag won't change when it does.

## Reference: Docker in WSL2 on Windows

**Scope:** the `windows` job in `.github/workflows/ci.yml`, the testkit on Windows

### Use for

Running the Linux fixture with Windows' own OpenSSH client.

### Reference

- Windows runners' Docker runs Windows containers only, so the `windows` job installs Ubuntu
  24.04 in WSL2 with `docker.io`, gives its dockerd a second listener on tcp 2375 (a systemd
  drop-in, since docker.io's unit owns dockerd), and runs the whole suite with
  `DOCKER_HOST=tcp://127.0.0.1:2375`.
- Ports the container publishes on 127.0.0.1 inside WSL reach Windows at the same port through
  WSL's localhost forwarding, so the testkit works unchanged.
- On Windows the testkit uses the inbox `ssh`/`ssh-keygen` (`openssh`), `NUL` for the null
  device (`NULL_DEVICE`), and resets copied private keys' ACL to the current user
  (`copy_private_key`).
- Unix-only cases skip on Windows (a private `ssh-agent -a <socket>`: Windows' agent is a
  per-machine service).

### Agent note

The WSL relay can drop a connection that is busy both ways; CI skips that one streamed-copy test
there and covers the same copy against Windows' own sshd instead
([Windows OpenSSH Server](#reference-windows-openssh-server)).

## Standard: The Native Backend

**Scope:** `crates/dromon-testkit/src/native.rs`, `DROMON_FIXTURE_BACKEND=native`

### Purpose

Run the same fixture where Docker can't run (GitHub's Apple-silicon macOS runners have no
nested virtualisation), with the platform's own sshd.

### Rules

- It changes the machine (system users, a root sshd), so it starts only when `CI` is set or
  `DROMON_FIXTURE_NATIVE_OK=1`, and only as root or with passwordless `sudo -n`. Otherwise every
  test skips. Never run it on your own machine: use a throwaway VM or container.
- Per test process, as root: a fresh `/var/tmp/dromon-testkit-sshd.XXXXXXXX` with its own host
  key, `sshd_config`, `sshd.pid`, `sshd.log` and `authorized_keys/<user>` (the testkit's keys,
  not the users' `~/.ssh`).
- The five users are created or reset (macOS `dscl`, hidden, group `staff`, added to
  `com.apple.access_ssh` when Remote Login is limited; Linux `useradd`), then seeded with
  `seed.sh <home>` as `dromon_key`.
- sshd is the platform's (`/usr/sbin/sshd`, the first known `sftp-server` path), checked with
  `sshd -t`, run as `sshd -D -e -f <dir>/sshd_config` on 127.0.0.1 at a free port with
  `UsePAM yes`, `AllowUsers` the five and the image's `Match` blocks. `StrictModes no`: the key
  files are root's under world-writable `/var/tmp`.
- Cleanup: a root `sh` watchdog polls the test process once a second; when it is gone it stops
  sshd and its sessions, deletes the folder and the kbd PAM service, and `pkill -u`s the users'
  leftovers only when no other fixture is live (test binaries run back to back and share the
  users). A lock (`/var/tmp/dromon-testkit-lock`) orders that check against a start. Users and
  homes stay for the next run.
- `Fixture::exec` runs `sudo -n <command>`; `Fixture::sha1` uses `shasum -a 1` on macOS.
- The server matrix skips here: its servers are Docker images.

### Agent checklist

- Does a change keep the `CI` / `DROMON_FIXTURE_NATIVE_OK` gate?
- Does the watchdog still clean up without touching a live fixture's processes?

## Reference: TOTP for the kbd User

**Scope:** `tests/docker/pam-sshd`, `crates/dromon-testkit/src/{totp.rs, native/totp.rs}`

### Use for

Tests of keyboard-interactive second factors.

### Reference

- Docker: `pam-sshd` adds libpam-google-authenticator with the image's `google_authenticator`
  secret; `totp_now` computes the code (RFC 6238, 30 s, 6 digits).
- Native: where a `pam_google_authenticator.so` is installed (macOS: Homebrew's
  `google-authenticator-libpam`, which the macOS CI jobs install; Debian:
  `libpam-google-authenticator`) and sshd has `PAMServiceName` (OpenSSH 9.8+), the kbd user gets
  `~/.google_authenticator` and a PAM service of its own, `/etc/pam.d/<folder name in lower
  case>`: the platform's sshd policy with `auth required <module>` appended, selected by
  `PAMServiceName` in its `Match` block. `/etc/pam.d/sshd` and the other users are untouched.
- After start, a probe login as the kbd user prints the prompts it saw, or on failure the
  policy, `sshd.log` and (macOS) code signatures and library-validation log lines.
- macOS: Apple's `sshd-session` is a platform binary, so library validation first refuses the
  Homebrew module; Apple's OpenPAM then clears library validation for the process and loads it.
  Its prompt is `Password:` without Linux-PAM's trailing space: tests take it from
  `Fixture::password_prompt(Role::Kbd)`.
- Without a module, `Feature::Totp` is off and the TOTP tests skip with a line saying why.

### Agent note

Ask with `Fixture::supports(Feature::Totp, test)`; never assume a second factor exists.

## Reference: Server Variants and the Matrix

**Scope:** `dromon_testkit::{Variant, matrix}`, `tests/docker/{bullseye, alpine, toxiproxy}/`

### Use for

Adding a server or changing what the matrix runs.

### Reference

| Variant | Image | What differs |
| --- | --- | --- |
| `Trixie` (default, `fixture()`) | `Dockerfile` | OpenSSH 10, PAM, `inotify-tools` (live updates): everything |
| `Bullseye` | `bullseye/Dockerfile` | OpenSSH 8.4p1: posix-rename, no `limits` (32 KiB chunks), no `copy-data` (same-host copies stream), no `inotifywait` (polls only); packages from a fixed snapshot.debian.org date |
| `Alpine` | `alpine/Dockerfile`, `alpine/sshd_config` | musl OpenSSH, busybox tools, no PAM: `kbd` can't log in, no TOTP, no `inotifywait` (polls only) |
| `Toxic` | `Dockerfile` + `toxiproxy/` | trixie behind toxiproxy: added latency, a bandwidth cap, replies sliced into small pieces (`toxiproxy::BASE_TOXICS`) |

- `fixture_of(Variant, test)` starts one server per variant and test process on first use.
  Every image has the same five users, takes the same keys and runs the same `entrypoint.sh` and
  `seed.sh`; the testkit adds the shared files to each build context, and the tag hashes it.
- Capabilities are asked, not assumed: `Fixture::has(Feature::CopyData | Limits | PosixRename |
  Toxics | Totp)`, `Fixture::supports(…, test)`, `Outcome::unless(fixture, feature)` in a
  matrix check. The capability check asserts the session detects what the fixture says.
- toxiproxy runs in its own container (with its own lifeline) next to sshd; its proxy forwards
  to the sshd container on the Docker network, and the fixture's port is the proxy's. Toxics are
  added and removed through its HTTP API (`Toxiproxy::add`, `remove`, `reset_peers`); a check
  that changes them uses `matrix::run_exclusive`, since toxics apply to every live connection.
- The 100k-entry folder is made on the server in the matrix run only (`Outcome::matrix_only()`)
  and deleted again.
- Only the trixie image installs `inotify-tools`, so the matrix sees both sides of live updates
  (`crates/dromon-app/tests/live_e2e.rs`): trixie and toxic go live, bullseye and alpine fall
  back to polls. Tests ask the server (`command -v inotifywait` through `Fixture::exec`) rather
  than assume it; the native backend's servers usually have none, and those checks skip.
- CI: the `linux-matrix` job runs both `--test matrix` binaries with `DROMON_SERVER_MATRIX=1`.

### Agent note

A new variant is a folder under `tests/docker/`, a `Variant` arm, an image spec in `docker.rs`
and the features it lacks; checks then skip it through `Outcome::unless`.

## Reference: Windows OpenSSH Server

**Scope:** `crates/dromon-transfer/tests/windows_sshd.rs`

### Use for

Checking Windows servers, whose SFTP paths are `/C:/Users/<user>`.

### Reference

- Opt-in: without `DROMON_WINDOWS_SSHD=1` its tests skip. It checks that the home and the drive
  (`/C:`) resolve and list, a file and a folder tree round-trip with mtimes, renames, a
  same-host copy and a delete.
- The `windows` CI job enables the inbox `sshd`, writes a fresh key to
  `%ProgramData%\ssh\administrators_authorized_keys` (ACL: Administrators and SYSTEM only, as
  sshd requires for that file), checks one login, then runs it with `DROMON_WINDOWS_SSHD_KEY`,
  `DROMON_WINDOWS_SSHD_USER` and `DROMON_WINDOWS_SSHD_PORT` (optionally
  `DROMON_WINDOWS_SSHD_HOST`, default 127.0.0.1).
- The same variables point it at any Windows machine with sshd and a key-authorized user.

### Agent note

This is the Windows coverage for streamed same-session copies; keep it when changing copies.

## Reference: Environment Variables

**Scope:** `crates/dromon-testkit`

### Use for

Choosing and debugging the server a test runs against.

### Reference

| Variable | Effect |
| --- | --- |
| (none) | Docker backend; skip when Docker doesn't answer |
| `DROMON_REQUIRE_DOCKER=1` | Fail instead of skipping when the fixture (either backend) can't start; CI sets it |
| `DOCKER_HOST` | A Linux Docker elsewhere (`tcp://127.0.0.1:2375` in WSL2 on Windows) |
| `DROMON_FIXTURE_BACKEND=native` | The platform's own sshd as root (see [The Native Backend](#standard-the-native-backend)) |
| `DROMON_FIXTURE_NATIVE_OK=1` | Allow the native backend outside CI (throwaway machines only) |
| `DROMON_FIXTURE_KEEP=1` | Keep the server and key folder after the tests; print how to log in and clean up |
| `DROMON_SERVER_MATRIX=1` | Matrix checks run against every variant, not only `Trixie` |
| `DROMON_WINDOWS_SSHD=1` (+ `_KEY`, `_USER`, `_PORT`, `_HOST`) | Run `windows_sshd.rs` against a Windows OpenSSH server |

### Agent note

The constants are `BACKEND_ENV`, `REQUIRE_DOCKER_ENV`, `NATIVE_OK_ENV`, `KEEP_ENV` and
`matrix::MATRIX_ENV` in the testkit.

## Recipe: Run the Native Backend in a Container

**Scope:** Linux, a stand-in for the macOS CI jobs

### Use when

Changing `native.rs` or a test that behaves differently on the native backend.

### Steps

1. Use a Debian or Ubuntu image with a non-root user that has passwordless sudo and the uid that
   owns the checkout, plus `openssh-server`, `openssh-client`, `sudo`, the toolchain,
   `libxkbcommon-dev libwayland-dev` (for `dromon-app`) and `libpam-google-authenticator` (for
   TOTP).
2. Run:
   ```sh
   docker run --rm -v "$PWD:/src" -w /src \
     -e CI=1 -e DROMON_FIXTURE_BACKEND=native -e DROMON_REQUIRE_DOCKER=1 \
     -e CARGO_TARGET_DIR=/src/target-native <image> \
     cargo test --no-fail-fast -p dromon-testkit -p dromon-ssh -p dromon-sftp -p dromon-transfer
   ```
3. On a throwaway macOS VM: `brew install google-authenticator-libpam`, then the same variables
   and `cargo test …` as an admin with passwordless sudo. Remove the users afterwards
   (`sudo dscl . -delete /Users/dromon_key; sudo rm -rf /Users/dromon_key`, and the other four;
   Linux `sudo userdel -r dromon_key`).

### Done when

The tests pass with no skip lines other than those the backend prints by design.

## Recipe: Run the Fixture Tests on Windows

**Scope:** a Windows machine with WSL2

### Use when

Reproducing a Windows-only fixture failure.

### Steps

1. In a WSL2 distro with Docker and localhost forwarding (the default), add a listener with
   `/etc/systemd/system/docker.service.d/tcp.conf`:
   `[Service]`, `ExecStart=`, `ExecStart=/usr/bin/dockerd -H fd:// -H tcp://127.0.0.1:2375
   --containerd=/run/containerd/containerd.sock`, then `systemctl daemon-reload && systemctl
   restart docker`. The port has no authentication: never bind it wider on a reachable machine.
2. In PowerShell:
   ```powershell
   $env:DOCKER_HOST = 'tcp://127.0.0.1:2375'
   $env:DROMON_REQUIRE_DOCKER = '1'
   cargo test -p dromon-testkit -p dromon-ssh -p dromon-sftp -p dromon-transfer
   ```

### Done when

The testkit's start line shows the server on 127.0.0.1 and the tests run without skipping.

## Runbook: Debug a Fixture Test

**Scope:** any fixture test

### Use when

A fixture test fails and you need to look at the server.

### Procedure

1. Keep the server: `DROMON_FIXTURE_KEEP=1 cargo test -p dromon-sftp --test fixture --
   list_mixed_folder`. The start line prints the port and key folder.
2. Log in as the test did: `ssh -F /dev/null -i <key folder>/id_ed25519 -p <port>
   key@127.0.0.1`.
3. Read sshd's log: `docker logs <id>` (native: `<server folder>/sshd.log`).
4. Clean up as printed: `docker rm -f <id>; rm -r <key folder>`.

### Success signals

You can reproduce the test's request by hand and see sshd's side of it.
