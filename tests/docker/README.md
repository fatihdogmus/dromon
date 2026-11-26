# sshd fixture

A Debian trixie OpenSSH server (PAM enabled) for the integration tests in
`crates/dromon-sftp/tests/fixture.rs`, `crates/dromon-ssh/tests/askpass_e2e.rs`,
`crates/dromon-transfer/tests/io.rs` and `crates/dromon-app/tests/{e2e,transfers_e2e}.rs`, and
other servers for the [server matrix](#server-matrix) (`*/tests/matrix.rs`). Users,
passwords and how each one logs in: `Dockerfile`, `sshd_config` and `pam-sshd`. Where Docker
can't run (macOS CI), the same users and data on the machine's own sshd: see
[Native sshd](#native-sshd-macos-ci).

## Running the tests

There is nothing to set up: `crates/dromon-testkit` builds the image and starts one container per
test binary with testcontainers whenever Docker is reachable.

```sh
cargo test --workspace                       # fixture tests run if Docker answers, else skip
DROMON_REQUIRE_DOCKER=1 cargo test --workspace   # CI: fail instead of skipping
cargo test -p dromon-sftp -- --ignored       # the slow S4 tests (1 GiB copy-data vs streaming)
```

Without Docker every fixture test prints `skipping <test>: Docker is not reachable (…)` and passes.
The testkit prints `dromon-testkit: <variant> sshd fixture on 127.0.0.1:<port>, <server banner>, container <id> …` when its container is up (`scripts/smoke-app.sh --connect` reads the port from it).

## How it works

- **Image**: built from the files in this folder (not `README.md` or the subfolders, which are the
  [server matrix](#server-matrix)'s) as `dromon-testkit-sshd:<hash of those files>`. The build is
  skipped while that tag exists, so it happens once per change. Old tags stay behind:
  `docker image rm $(docker image ls -q dromon-testkit-sshd)`.
- **Keys**: each test process generates its keypairs with `ssh-keygen` into
  `$TMPDIR/dromon-testkit-<pid>-…/` and copies the public keys into the container before it starts;
  no key is in the image. Folders of finished processes are deleted on the next start.
- **Ports**: 22 (sshd) and 9999 (lifeline) are published on 127.0.0.1 at random host ports.
- **Cleanup**: the test process holds a connection to port 9999 for its whole life. When it exits
  (also on a panic or `kill -9`) the container stops and Docker removes it (`AutoRemove`), a second
  or two later. A container nobody connects to ends after 120 s.
  `docker ps -a --filter label=dev.dromon.testkit.pid` lists fixture containers.
- **Seed data**: `seed.sh` (run as `key` at start) makes `fixture/mixed` and `fixture/many`. The
  1 GiB `fixture/big/big.bin` is made over ssh on first use by the `--ignored` tests only.
  `seed.sh <home>` seeds another home (the native backend's key user); without an argument it is
  `/home/key`.

## Server matrix

The same SFTP and transfer checks against several real servers (`.plans/dromon-plan.md` §9, M5):
`crates/dromon-sftp/tests/matrix.rs` (capabilities, listings of 100, 10k and 100k entries with
paging and timings, stat/mkdir/rename/remove_all, symlink loops, unreadable folders, reads and
writes, copy-data or streaming) and `crates/dromon-transfer/tests/matrix.rs` (download, upload,
same-host copy, moves, symlink loops in scans, resume after a connection reset). Each check runs
against every server at once and prints one line per server; it fails after all of them ran if
any failed:

```text
matrix: sftp copy-data or streaming        bullseye  ok       0.3 s  no copy-data: streamed
```

```sh
cargo test -p dromon-sftp -p dromon-transfer --test matrix          # trixie only (as in every cargo test)
DROMON_SERVER_MATRIX=1 cargo test -p dromon-sftp -p dromon-transfer --test matrix   # every server
```

| Variant (`dromon_testkit::Variant`) | Image | What differs |
|---|---|---|
| `Trixie` (the default, `fixture()`) | `Dockerfile` | OpenSSH 10, PAM: everything |
| `Bullseye` | `bullseye/Dockerfile` | OpenSSH 8.4p1: posix-rename, but no `limits@openssh.com` (32 KiB chunks) and no `copy-data` (same-host copies stream); packages from a fixed snapshot.debian.org date (bullseye has left the mirrors) |
| `Alpine` | `alpine/Dockerfile`, `alpine/sshd_config` | Alpine's musl OpenSSH, busybox tools, no PAM: `kbd` can't log in, no TOTP |
| `Toxic` | `Dockerfile` + `toxiproxy/` | the trixie image behind toxiproxy: about 20 ms (±10) more round trip, 4 MB/s each way, replies sliced into ~1.4 KB pieces (`toxiproxy::BASE_TOXICS`); tests add toxics and reset connections through toxiproxy's HTTP API |

- **Selection**: `dromon_testkit::fixture_of(Variant, test)` starts one server per variant and test
  process on first use (`fixture(test)` is `Trixie`); `matrix::run(check, |fixture| …)` runs a
  check against `matrix::variants()`: all four with `DROMON_SERVER_MATRIX=1`, else `Trixie` only,
  so a plain `cargo test` starts nothing new. On the native backend the matrix skips (its servers
  are Docker images).
- **Same users and data**: every image has the five users, takes the same keys and runs the same
  `entrypoint.sh` and `seed.sh` (under busybox on Alpine); `bullseye/` also reuses `sshd_config`,
  `pam-sshd` and `google_authenticator`. The testkit adds those shared files to each variant's
  build context; the tag hashes the whole context.
- **Capabilities are asked, not assumed**: `Fixture::has(Feature::CopyData | Limits | PosixRename |
  Toxics | Totp)` (silent), `Fixture::supports(…, test)` (prints the skip line),
  `Outcome::unless(fixture, feature)` in a matrix check. The capability check asserts that what
  the session detects matches the fixture's answer.
- **toxiproxy**: a container of `toxiproxy/Dockerfile` (Shopify's binaries, `toxiproxy.sh`) next
  to the sshd container, with its own lifeline; its proxy `sshd` forwards `0.0.0.0:2222` to the
  sshd container's address on the Docker bridge (`UPSTREAM`), and the testkit adds the base toxics
  through the API (port 8474, published on 127.0.0.1 like 2222; `Fixture::toxiproxy()`). The
  resume check (`matrix::run_exclusive`: no other check uses that server meanwhile) adds 1 MB/s
  caps and more latency, cuts a download and an upload at 2 MiB with `reset_peer`
  (`Toxiproxy::reset_peers`), reconnects and checks that both resume from their part files and
  end with the exact bytes.
- **Big folders**: `fixture/mixed` (100) and `fixture/many` (10k) are seeded; the 100k folder is
  made by one command on the server in the matrix run only (`Outcome::matrix_only()`), listed
  paged and whole, and deleted again.
- **Time**: about 2 minutes for both binaries on a warm machine (images cached); the 100k listing
  behind toxiproxy (about 30 s per listing) is the slowest check.

CI: the `linux` job's `Server matrix (Docker sshd variants)` step.

## Windows OpenSSH server (CI)

`crates/dromon-transfer/tests/windows_sshd.rs` checks Windows' own OpenSSH server, whose SFTP
paths are `/C:/Users/<user>`: the home folder and the drive (`/C:`, which the server reads as
`C:\`) resolve and list, a file and a folder tree round-trip with their mtimes, renames, a
same-host copy and a delete. It is opt-in: without `DROMON_WINDOWS_SSHD=1` its tests skip. The
`windows` job's `Tests (Windows OpenSSH server, /C:/ paths)` step starts the inbox `sshd`
(installing the `OpenSSH.Server` capability if missing), writes a fresh key to
`%ProgramData%\ssh\administrators_authorized_keys` (the runner's user is an administrator; the
file's ACL is Administrators and SYSTEM only, as sshd requires), checks one login and runs it with
`DROMON_WINDOWS_SSHD_KEY`, `DROMON_WINDOWS_SSHD_USER` and `DROMON_WINDOWS_SSHD_PORT` (and
optionally `DROMON_WINDOWS_SSHD_HOST`, default 127.0.0.1). The same variables point it at any
Windows machine with sshd and a key-authorized user.

## Native sshd (macOS CI)

GitHub's Apple-silicon macOS runners can't run Docker (no nested virtualisation), so the testkit
has a second backend: `DROMON_FIXTURE_BACKEND=native` (`crates/dromon-testkit/src/native.rs`).
The `macos` job in `.github/workflows/ci.yml` runs every fixture test with it (askpass end to end,
`dromon-sftp`'s `fixture`, `dromon-transfer`'s `io`, `dromon-app`'s `e2e` and `transfers_e2e`).
`docker` stays the default; Linux and Windows CI use it unchanged.

What it does, once per test binary:

- **Safety gate**: it creates system users and runs sshd as root, so it refuses to start unless
  `CI` is set (GitHub sets it) or `DROMON_FIXTURE_NATIVE_OK=1`, and it needs root: either the
  process is root or `sudo -n true` works (GitHub's runners have passwordless sudo). Otherwise each
  test prints `skipping <test>: …` and passes; `DROMON_REQUIRE_DOCKER=1` makes that a failure, as
  for Docker (the name is kept: it means "the fixture must run").
- **Server folder**: as root, a fresh `/var/tmp/dromon-testkit-sshd.XXXXXXXX` (755) holding its
  own ed25519 host key, the generated `sshd_config`, `sshd.pid`, `sshd.log` and
  `authorized_keys/<user>` (the testkit's public keys, not the users' own `~/.ssh`). Folders left
  by processes that are gone are deleted on the next start.
- **Users**: the same five, prefixed so they can't collide with real accounts: `dromon_key`,
  `dromon_pw`, `dromon_kbd`, `dromon_keypw`, `dromon_enc`, with homes `/Users/dromon_*` (macOS,
  `dscl`, primary group `staff`, hidden; added to `com.apple.access_ssh` when Remote Login is
  limited to that group) or `/home/dromon_*` (Linux, `useradd` + `chpasswd`). `pw`, `kbd` and
  `keypw` get `dromon-test-pw`; `key` and `enc` no usable password (Linux `*`, macOS a random
  one). An existing user is reused and reset, so reruns are fine. Tests get the names and homes
  from `Fixture::user(Role)` and `Fixture::home(Role)`, never `/home/...` literals.
- **Seed**: `seed.sh <home>` as `dromon_key`, so the data is the image's.
- **sshd**: the platform's own (`/usr/sbin/sshd`; sftp-server `/usr/libexec/sftp-server` on
  macOS, `/usr/lib/openssh/sftp-server` on Debian/Ubuntu, the first of the known paths that
  exists), `sshd -t` first, then `sshd -D -e -f <folder>/sshd_config` on `127.0.0.1:<free port>`
  with `UsePAM yes` (the platform's PAM stack), `AllowUsers` the five, and the Match blocks of
  `sshd_config` here, so every user has the same methods as in Docker. `StrictModes no`: the
  key files are root's but live under world-writable `/var/tmp`.
- **TOTP** for `dromon_kbd`, as `pam-sshd` gives `kbd`, when a `pam_google_authenticator.so` is
  installed (macOS: Homebrew's `google-authenticator-libpam`, which the CI job installs; Debian:
  `libpam-google-authenticator`) and sshd has `PAMServiceName` (OpenSSH 9.8+): the image's
  `google_authenticator` as `~/.google_authenticator` (mode 400), and a PAM service of its own,
  `/etc/pam.d/<folder name in lower case>`: a copy of the platform's `/etc/pam.d/sshd` with
  `auth required <module>` appended (what Homebrew's caveat tells an admin to add). The Match
  block of `dromon_kbd` selects it with `PAMServiceName` (and `LogLevel DEBUG1`, so `sshd.log`
  names the service), so `/etc/pam.d/sshd` and the other users are untouched. After start, a
  probe login as `dromon_kbd` with the password and a code prints the prompts it saw, or, if it
  fails, the policy, `sshd.log` and on macOS `codesign -dv` of the module and of
  `sshd`/`sshd-session`/`sshd-auth` and the unified log's PAM and library-validation lines.
  macOS: `sshd-session` is a platform binary, so the first `dlopen` of the Homebrew module is
  refused by library validation; Apple's OpenPAM then clears it (`csops(CS_OPS_CLEAR_LV)`,
  allowed by the `com.apple.private.security.clear-library-validation` entitlement of
  `sshd-session`) and loads the module. The password prompt is then OpenPAM's
  `(dromon_kbd@127.0.0.1) Password:`, without Linux-PAM's trailing space; tests take it from
  `Fixture::password_prompt(Role::Kbd)`. Without a module, `Fixture::supports(Feature::Totp, …)`
  prints `skipping <test>: the native sshd has no TOTP PAM module (Docker fixture only)` and
  `keyboard_interactive_password_then_verification_code`,
  `keyboard_interactive_wrong_code_fails_cleanly` (askpass) and
  `keyboard_interactive_password_then_code` (app e2e) skip; the start line says why.
- **Cleanup**: sshd is started by a root `sh` watchdog that checks once a second whether the test
  process still exists; when it is gone (normal exit, panic, `kill -9`) the watchdog stops sshd,
  ends that sshd's sessions (its process tree) and deletes the folder and the kbd PAM service.
  The fixture users' other leftovers (what a session left running) are `pkill -u`ed only when
  no other fixture is live (its test process runs, or it is kept and its sshd runs): all
  fixtures share the users, and the next test binary's start (`seed.sh`) overlaps the previous
  one's cleanup. A lock (`/var/tmp/dromon-testkit-lock`) orders that check against a start
  making its folder. The next start deletes folders whose test process and watchdog are both
  gone, and services no folder names. The users and their homes stay.
  `DROMON_FIXTURE_KEEP=1` skips the watchdog and prints how to log in and clean up.
- **`Fixture::exec`** runs `sudo -n <command>` (as root, like `docker exec`); `kill_sessions` is
  `pkill -u` as in the container; `Fixture::sha1` uses `shasum -a 1` on macOS.

To run it locally, use a throwaway VM or container, never your own machine: it adds users and
runs a root sshd. A Debian/Ubuntu container works as a stand-in for the macOS runner, with a
non-root user that has passwordless sudo, `openssh-server`, `openssh-client`, `sudo` and the
toolchain (plus `libxkbcommon-dev libwayland-dev` for `dromon-app`, and
`libpam-google-authenticator` for the TOTP tests); give the user the uid that owns the checkout
so the mounted folder is writable:

```sh
docker run --rm -v "$PWD:/src" -w /src \
  -e CI=1 -e DROMON_FIXTURE_BACKEND=native -e DROMON_REQUIRE_DOCKER=1 \
  -e CARGO_TARGET_DIR=/src/target-native <image with that user> \
  cargo test --no-fail-fast -p dromon-testkit -p dromon-ssh -p dromon-sftp -p dromon-transfer
```

On a throwaway macOS VM: `brew install google-authenticator-libpam` for the TOTP tests, then
`CI=1 DROMON_FIXTURE_BACKEND=native DROMON_REQUIRE_DOCKER=1 cargo test …`
as an admin with passwordless sudo. Remove the users afterwards with
`sudo dscl . -delete /Users/dromon_key; sudo rm -rf /Users/dromon_key` (and the other four), on
Linux `sudo userdel -r dromon_key`.

## Windows CI via WSL2

The `windows-docker` job in `.github/workflows/ci.yml` runs the fixture tests of `dromon-testkit`,
`dromon-ssh` (askpass end to end), `dromon-sftp` and `dromon-transfer` on `windows-2025` with
Windows' own OpenSSH (`C:\Windows\System32\OpenSSH`) and the named-pipe askpass, against this same
Linux image. The runner's own Docker runs Windows containers only, so the job installs Ubuntu 24.04
in WSL2 with `docker.io`, gives its dockerd a second listener on `tcp://0.0.0.0:2375` (systemd
drop-in), and runs the tests with `DOCKER_HOST=tcp://127.0.0.1:2375`. The ports the container
publishes on 127.0.0.1 inside WSL reach Windows at the same 127.0.0.1 port through WSL's localhost
forwarding, so the testkit works unchanged; on Windows it uses `NUL` for `/dev/null`, the inbox
`ssh`/`ssh-keygen`, and resets the ACL of copied private keys to the current user.

To reproduce on a Windows machine: in a WSL2 distro with Docker (and localhost forwarding, the
default), add the same listener, e.g. `/etc/systemd/system/docker.service.d/tcp.conf` with
`[Service]`, `ExecStart=` and `ExecStart=/usr/bin/dockerd -H fd:// -H tcp://127.0.0.1:2375
--containerd=/run/containerd/containerd.sock`, then `systemctl daemon-reload && systemctl restart
docker` (127.0.0.1 is enough with localhost forwarding; the port has no authentication, so don't
bind it wider on a machine others can reach). Then, in PowerShell:

```powershell
$env:DOCKER_HOST = 'tcp://127.0.0.1:2375'
$env:DROMON_REQUIRE_DOCKER = '1'
cargo test -p dromon-testkit -p dromon-ssh -p dromon-sftp -p dromon-transfer
```

`docker` on Windows (used by `Fixture::exec`) follows `DOCKER_HOST` too. Unix-only cases:
`add_key_to_agent_is_a_confirm_prompt` (needs a private `ssh-agent -a <socket>`; Windows' agent
is a per-machine service).

## Debugging

```sh
DROMON_FIXTURE_KEEP=1 cargo test -p dromon-sftp --test fixture -- list_mixed_folder
```

keeps the container running after the tests (no lifeline) and its key folder (never swept), and
prints how to log in (`ssh -F /dev/null -i <key folder>/id_ed25519 -p <port> key@127.0.0.1`) and
how to clean up (`docker rm -f <id>; rm -r <key folder>`). `docker logs <id>` shows sshd's log.
