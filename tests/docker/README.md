# sshd fixture

A Debian trixie OpenSSH server (PAM enabled) for the integration tests in
`crates/dromon-sftp/tests/fixture.rs`, `crates/dromon-ssh/tests/askpass_e2e.rs`,
`crates/dromon-transfer/tests/io.rs` and `crates/dromon-app/tests/{e2e,transfers_e2e}.rs`. Users,
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
The testkit prints `dromon-testkit: sshd fixture on 127.0.0.1:<port> …` when its container is up.

## How it works

- **Image**: built from the files in this folder (not `README.md`) as
  `dromon-testkit-sshd:<hash of those files>`. The build is skipped while that tag exists, so it
  happens once per change. Old tags stay behind: `docker image rm $(docker image ls -q dromon-testkit-sshd)`.
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
- **Cleanup**: sshd is started by a root `sh` watchdog that checks once a second whether the test
  process still exists; when it is gone (normal exit, panic, `kill -9`) the watchdog stops sshd,
  `pkill -u`s the fixture users' leftover sessions and deletes the folder. The users and their
  homes stay. `DROMON_FIXTURE_KEEP=1` skips the watchdog and prints how to log in and clean up.
- **`Fixture::exec`** runs `sudo -n <command>` (as root, like `docker exec`); `kill_sessions` is
  `pkill -u` as in the container; `Fixture::sha1` uses `shasum -a 1` on macOS.

Not available natively: the TOTP second factor (`libpam-google-authenticator`; macOS has no such
module). Tests that need it ask `Fixture::supports(Feature::Totp, …)`, which prints
`skipping <test>: the native sshd has no TOTP PAM module (Docker fixture only)`:
`keyboard_interactive_password_then_verification_code` and
`keyboard_interactive_wrong_code_fails_cleanly` (askpass) and
`keyboard_interactive_password_then_code` (app e2e). Everything else runs.

To run it locally, use a throwaway VM or container, never your own machine: it adds users and
runs a root sshd. A Debian/Ubuntu container works as a stand-in for the macOS runner, with a
non-root user that has passwordless sudo, `openssh-server`, `openssh-client`, `sudo` and the
toolchain (plus `libxkbcommon-dev libwayland-dev` for `dromon-app`); give the user the uid that
owns the checkout so the mounted folder is writable:

```sh
docker run --rm -v "$PWD:/src" -w /src \
  -e CI=1 -e DROMON_FIXTURE_BACKEND=native -e DROMON_REQUIRE_DOCKER=1 \
  -e CARGO_TARGET_DIR=/src/target-native <image with that user> \
  cargo test --no-fail-fast -p dromon-testkit -p dromon-ssh -p dromon-sftp -p dromon-transfer
```

On a throwaway macOS VM: `CI=1 DROMON_FIXTURE_BACKEND=native DROMON_REQUIRE_DOCKER=1 cargo test …`
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
