# sshd fixture

A Debian trixie OpenSSH server (PAM enabled) for the integration tests in
`crates/dromon-sftp/tests/fixture.rs` and `crates/dromon-ssh/tests/askpass_e2e.rs`. Users,
passwords and how each one logs in: `Dockerfile`, `sshd_config` and `pam-sshd`.

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
