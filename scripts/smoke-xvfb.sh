#!/usr/bin/env bash
# Smoke test of the real windowed startup path on Linux, under Xvfb with Mesa's software Vulkan
# (lavapipe): the `dromon` binary opens its window, `on_ready` starts the service thread, host
# discovery reads ~/.ssh/config, and the sidebar gets its alias (run 1, Rune's default
# CPU-first window). In run 2 Rune's `--window-script` replays a click on that host in the real
# window (Rune tooling, debug builds; no app code), so the app also connects to the Docker sshd
# fixture and lists the home folder and a subfolder.
#
#   bash scripts/smoke-xvfb.sh          # exits 0 with its evidence lines, or 1 naming what failed
#
# Needs Docker, xvfb-run, libxkbcommon-x11 and mesa-vulkan-drivers (lavapipe). The fixture is
# started by the testkit with DROMON_FIXTURE_KEEP=1 (through dromon-sftp's `list_mixed_folder`
# test, as tests/docker/README.md describes) and removed at the end, with the key folder and
# the temporary HOME. The app runs with HOME set to that temporary folder, so the developer's
# ssh config, known_hosts, agent (SSH_AUTH_SOCK is unset) and app state are never touched.
#
# What the app did is read from `DROMON_TRACE=1`: one stderr line per service event it applied,
# naming the event kind and host only (never paths, prompt text or secrets).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
icd="${VK_ICD_FILENAMES:-/usr/share/vulkan/icd.d/lvp_icd.json}"
seconds="${SMOKE_SECONDS:-10}"
alias=fx-smoke

fail() {
  echo "smoke: FAILED: $*" >&2
  exit 1
}

command -v xvfb-run >/dev/null || fail "xvfb-run is not installed"
[[ -f "$icd" ]] || fail "no Vulkan ICD at $icd (install mesa-vulkan-drivers)"
docker info >/dev/null 2>&1 || fail "Docker is not reachable"

work="$(mktemp -d "${TMPDIR:-/tmp}/dromon-smoke.XXXXXX")"
container=""
keys=""
cleanup() {
  if [[ -n "$container" ]]; then docker rm -f "$container" >/dev/null 2>&1 || true; fi
  if [[ -n "$keys" && -d "$keys" ]]; then rm -rf "$keys"; fi
  rm -rf "$work"
}
trap cleanup EXIT

echo "smoke: building"
cargo build -p dromon-app --quiet
bin="$root/target/debug/dromon"

echo "smoke: starting the sshd fixture (DROMON_FIXTURE_KEEP=1)"
DROMON_FIXTURE_KEEP=1 DROMON_REQUIRE_DOCKER=1 \
  cargo test -p dromon-sftp --quiet --test fixture -- --exact list_mixed_folder >"$work/fixture.log" 2>&1 \
  || { cat "$work/fixture.log" >&2; fail "the fixture test failed"; }
# dromon-testkit: sshd fixture on 127.0.0.1:<port>, container <id>, …
# dromon-testkit: …; clean up with `docker rm -f <id>; rm -r <key folder>`
port="$(sed -n 's/^dromon-testkit: sshd fixture on 127\.0\.0\.1:\([0-9]*\),.*/\1/p' "$work/fixture.log" | head -n1)"
container="$(sed -n 's/.*clean up with `docker rm -f \([0-9a-f]*\); rm -r \([^`]*\)`.*/\1/p' "$work/fixture.log" | head -n1)"
keys="$(sed -n 's/.*clean up with `docker rm -f \([0-9a-f]*\); rm -r \([^`]*\)`.*/\2/p' "$work/fixture.log" | head -n1)"
[[ -n "$port" && -n "$container" && -f "$keys/id_ed25519" ]] || { cat "$work/fixture.log" >&2; fail "couldn't read the fixture's port, container and keys"; }
echo "smoke: fixture on 127.0.0.1:$port, container $container"

# A HOME of our own: an ssh config with one alias for the fixture, its host key already known
# (so connecting asks nothing), and the fixture's key.
home="$work/home"
mkdir -p "$home/.ssh"
chmod 700 "$home/.ssh"
cp "$keys/id_ed25519" "$home/.ssh/id_fixture"
chmod 600 "$home/.ssh/id_fixture"
ssh-keyscan -t ed25519 -p "$port" 127.0.0.1 >"$home/.ssh/known_hosts" 2>/dev/null
[[ -s "$home/.ssh/known_hosts" ]] || fail "ssh-keyscan got no host key"
cat >"$home/.ssh/config" <<EOF
Host $alias
    HostName 127.0.0.1
    Port $port
    User key
    IdentityFile $home/.ssh/id_fixture
    IdentitiesOnly yes
    PreferredAuthentications publickey
    UserKnownHostsFile $home/.ssh/known_hosts
    GlobalKnownHostsFile /dev/null
    StrictHostKeyChecking yes
EOF

# ssh finds ~/.ssh/config through the password database, not $HOME, so on its own it would read
# the developer's config. An `ssh` first on PATH (the app runs `ssh` from PATH) passes
# `-F <our config>` to the real one: discovery's `ssh -G` and the connections read only it.
mkdir -p "$work/bin"
real_ssh="$(command -v ssh)" || fail "no ssh on PATH"
cat >"$work/bin/ssh" <<EOF
#!/bin/sh
exec "$real_ssh" -F "$home/.ssh/config" "\$@"
EOF
chmod 755 "$work/bin/ssh"

app_env=(env -u SSH_AUTH_SOCK PATH="$work/bin:$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config" XDG_DATA_HOME="$home/.local/share"
  XDG_STATE_HOME="$home/.local/state" XDG_CACHE_HOME="$home/.cache" VK_ICD_FILENAMES="$icd")

# How many hosts discovery finds with this HOME: the alias, plus any literal aliases in the
# system's ssh_config (e.g. systemd's `.host`). A headless `--tree` run reads the same config
# files synchronously in `init`, so its sidebar is the reference for the window's.
"${app_env[@]}" "$bin" --tree "$work/tree.json" >/dev/null 2>&1 || fail "the headless --tree run failed"
mapfile -t tree_hosts < <(grep -o '"key": *"host-[^"]*"' "$work/tree.json" | sed 's/.*"host-\(.*\)"/\1/' | sort -u)
[[ " ${tree_hosts[*]} " == *" $alias "* ]] || fail "the headless sidebar lacks $alias: ${tree_hosts[*]}"
expected=${#tree_hosts[@]}
echo "smoke: headless discovery (--tree) lists $expected hosts: ${tree_hosts[*]}"

evidence() { echo "smoke:   $*"; }

# run_window NAME SECONDS [ENV=VALUE…] -- [APP ARGS…]: the binary in a real X11 window under
# Xvfb with DROMON_TRACE=1 for SECONDS, then stopped. It must still be running by then.
run_window() {
  local name="$1" secs="$2"
  shift 2
  local extra_env=()
  while [[ $# -gt 0 && "$1" != -- ]]; do extra_env+=("$1"); shift; done
  [[ $# -gt 0 ]] && shift
  echo "smoke: [$name] $bin $* under Xvfb for $secs s${extra_env[*]:+ (${extra_env[*]})}"
  set +e
  xvfb-run -a -s "-screen 0 1280x800x24" "${app_env[@]}" DROMON_TRACE=1 "${extra_env[@]}" \
    sh -c 'bin="$1" secs="$2"; shift 2; "$bin" "$@" & pid=$!; sleep "$secs"
      if kill -0 "$pid" 2>/dev/null; then kill "$pid"; wait "$pid"; echo "app: still running after $secs s, stopped (exit $?)" >&2
      else wait "$pid"; echo "app: exited early (exit $?)" >&2; exit 3; fi' \
    sh "$bin" "$secs" "$@" >"$work/$name.out" 2>"$work/$name.err"
  local status=$?
  set -e
  [[ $status -eq 0 ]] || { cat "$work/$name.out" "$work/$name.err" >&2; fail "[$name] xvfb-run exited $status"; }
  if grep -qiE "panicked|couldn't start the service thread|Library .* could not be loaded" "$work/$name.err"; then
    cat "$work/$name.err" >&2
    fail "[$name] the app reported an error"
  fi
}

# trace NAME PATTERN: the run's trace lines matching PATTERN (after `dromon-trace: `).
trace() { grep -E "^dromon-trace: $2" "$work/$1.err" || true; }

expect_hosts() {
  local hosts
  hosts="$(trace "$1" "Hosts count=$expected$")"
  [[ -n "$hosts" ]] || { cat "$work/$1.err" >&2; fail "[$1] no 'Hosts count=$expected': the service thread never discovered the hosts"; }
  evidence "[$1] service thread discovered the hosts: $(echo "$hosts" | wc -l) × '$(echo "$hosts" | head -n1)' (aliases, then ssh -G)"
}

# 1. The startup path as a user gets it (Rune's defaults: the first frame on the CPU, then
#    the GPU), no input: the window opens, on_ready starts the service, the sidebar gets the hosts.
run_window startup 5 --
expect_hosts startup
evidence "[startup] $(grep -h '^app: ' "$work/startup.err")"

# 2. Connect through the window: Rune replays a click on the host, then a double click on its
#    `fixture` folder, on the wall clock from the window's first GPU frame. `--window-script`
#    starts its clock only at a GPU-drawn frame, and a window resting on its CPU-drawn first
#    frame never draws another, so this run starts on the GPU (RUNE_CPU_FIRST=0). The pointer
#    move first makes sure the frame with the discovered hosts is laid out before the click.
cat >"$work/connect.json" <<EOF
{"events": [
  {"t": 2.0, "type": "move", "x": 400, "y": 300},
  {"t": 3.0, "type": "tap", "key": "host-$alias"},
  {"t": 6.0, "type": "tap", "key": "rows/fixture"},
  {"t": 6.08, "type": "tap", "key": "rows/fixture"}
]}
EOF
run_window connect "$seconds" RUNE_CPU_FIRST=0 -- --window-script "$work/connect.json"
expect_hosts connect
grep -qh "^rune-script-window" "$work/connect.out" "$work/connect.err" || { cat "$work/connect.out" "$work/connect.err" >&2; fail "Rune never replayed the script in a window"; }
evidence "[connect] real window: $(grep -h "^rune-script-window" "$work/connect.out" "$work/connect.err" | head -n1)"
if grep -qh "^rune-script-error" "$work/connect.out" "$work/connect.err"; then
  cat "$work/connect.out" "$work/connect.err" >&2
  fail "the window script failed: $(grep -h "^rune-script-error" "$work/connect.out" "$work/connect.err")"
fi
evidence "[connect] $(grep -h "^rune-script-done" "$work/connect.out" "$work/connect.err" | head -n1)"
for want in "Status host=$alias status=Connecting" "Status host=$alias status=Ready" "Opened host=$alias"; do
  line="$(trace connect "$want$")"
  [[ -n "$line" ]] || { cat "$work/connect.err" >&2; fail "[connect] no '$want'"; }
  evidence "[connect] $(echo "$line" | head -n1)"
done
pages="$(trace connect "ListingPage host=$alias .*done=true$" | wc -l)"
[[ "$pages" -ge 2 ]] || { cat "$work/connect.err" >&2; fail "[connect] expected two finished listings (home, fixture), saw $pages"; }
evidence "[connect] finished listings (home, then fixture): $pages"
if trace connect "Prompt " | grep -q .; then fail "[connect] a prompt was shown, though the known host key and the key need none"; fi
evidence "[connect] $(grep -c '^dromon-trace:' "$work/connect.err") trace lines, none a prompt; $(grep -h '^app: ' "$work/connect.err")"
echo "smoke: OK"
