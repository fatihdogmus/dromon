#!/usr/bin/env bash
# Smoke tests of the dromon binary. Each mode runs the app with a throwaway HOME (its own ssh
# config, known_hosts and app state; SSH_AUTH_SOCK unset), so the developer's are never touched.
#
#   bash scripts/smoke-app.sh             # headless (any OS, no display): see below
#   bash scripts/smoke-app.sh --window    # the real window for a few seconds
#   bash scripts/smoke-app.sh --connect   # Linux + Docker: connect through a real window
#   bash scripts/smoke-app.sh --window --connect   # modes run in the order given
#
# Modes:
#   (default)  a. `--fixture <demo> --scene listing --snap`: a non-empty PNG;
#              b. real mode, `--tree` with an empty ~/.ssh/config: the app starts and its
#                 sidebar has no hosts but literal aliases of the system's ssh_config.
#   --window   the binary in a real window for DROMON_SMOKE_SECONDS (8) with DROMON_TRACE=1;
#              on Linux without $DISPLAY or $WAYLAND_DISPLAY, under `xvfb-run -a` with Mesa's
#              software Vulkan (lavapipe, VK_ICD_FILENAMES). Fails if it exits early or reports
#              an error; the trace must show the service thread started (`Hosts count=`).
#   --connect  Linux + Docker: the sshd fixture (dromon-testkit, DROMON_FIXTURE_KEEP=1), an ssh
#              config with one alias for it, and Rune's `--window-script` clicking that host and
#              its `fixture` folder in a real window under Xvfb: the trace must show
#              Connecting → Ready → Opened and two finished listings, and no prompt.
#
# Env: DROMON_SMOKE_SECONDS (window mode, 8), DROMON_SMOKE_CONNECT_SECONDS (connect mode, 10),
# VK_ICD_FILENAMES (lavapipe's ICD on Linux), DROMON_SMOKE_KEEP=1 keeps the work folder.
# Prints `smoke: OK` and exits 0 when every mode passed; prints `smoke: FAILED: <why>` and exits
# 1 otherwise (2 on bad arguments). Removes its temporary folders and containers either way.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

modes=()
for a in "$@"; do
  case "$a" in
    --window | --connect) modes+=("${a#--}") ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "smoke: unknown argument $a (expected --window or --connect)" >&2; exit 2 ;;
  esac
done
[[ ${#modes[@]} -gt 0 ]] || modes=(headless)

fail() {
  echo "smoke: FAILED: $*" >&2
  exit 1
}
note() { echo "smoke: $*"; }
evidence() { echo "smoke:   $*"; }

os="$(uname -s)"
work="$(mktemp -d "${TMPDIR:-/tmp}/dromon-smoke.XXXXXX")"
container=""
keys=""
app_pid=""
cleanup() {
  if [[ -n "$app_pid" ]]; then kill "$app_pid" 2>/dev/null || true; fi
  if [[ -n "$container" ]]; then docker rm -f "$container" >/dev/null 2>&1 || true; fi
  if [[ -n "$keys" && -d "$keys" ]]; then rm -rf "$keys"; fi
  if [[ "${DROMON_SMOKE_KEEP:-0}" == 1 ]]; then echo "smoke: kept $work" >&2; else rm -rf "$work"; fi
}
trap cleanup EXIT

note "building"
cargo build -p dromon-app --quiet || fail "the build failed"
bin="$root/target/debug/dromon"
[[ -x "$bin.exe" ]] && bin="$bin.exe"
[[ -x "$bin" ]] || fail "no binary at $bin"
icd="${VK_ICD_FILENAMES:-/usr/share/vulkan/icd.d/lvp_icd.json}"

# new_home NAME: a fresh HOME with an empty ~/.ssh/config; prints its path.
new_home() {
  local home="$work/$1"
  mkdir -p "$home/.ssh" "$home/.config" "$home/.local/share" "$home/.local/state" "$home/.cache"
  chmod 700 "$home/.ssh"
  : >"$home/.ssh/config"
  echo "$home"
}

# use_home HOME: sets `app_env`, the `env` command line the app runs under.
app_env=()
use_home() {
  local home="$1"
  app_env=(env -u SSH_AUTH_SOCK HOME="$home" USERPROFILE="$home" XDG_CONFIG_HOME="$home/.config"
    XDG_DATA_HOME="$home/.local/share" XDG_STATE_HOME="$home/.local/state" XDG_CACHE_HOME="$home/.cache")
}

# Literal aliases in the system's ssh_config (and what it includes from ssh_config.d), which a
# sidebar shows even with an empty user config (systemd ships `.host`, for one).
system_aliases() {
  local files=(/etc/ssh/ssh_config /etc/ssh/ssh_config.d/*.conf)
  [[ -n "${PROGRAMDATA:-}" ]] && files+=("$PROGRAMDATA/ssh/ssh_config")
  for f in "${files[@]}"; do
    [[ -r "$f" ]] || continue
    sed -n 's/^[[:space:]]*[Hh][Oo][Ss][Tt][[:space:]=]\{1,\}//p' "$f" | tr ' \t' '\n\n'
  done | grep -v '^$' | grep -v '[*?!]' | sort -u || true
}

# tree_hosts FILE: the host aliases a `--tree` dump's sidebar shows.
tree_hosts() {
  grep -o '"key": *"host-[^"]*"' "$1" | sed 's/.*"host-\(.*\)"/\1/' | sort -u || true
}

headless() {
  local log
  use_home "$(new_home headless)"
  # a. The demo fixture, headless.
  log="$work/fixture-snap.log"
  "${app_env[@]}" "$bin" --fixture "$root/crates/dromon-app/assets/fixtures/demo.json" --scene listing \
    --snap "$work/listing.png" --size 1080x680 --scale 1 >"$log" 2>&1 || { cat "$log" >&2; fail "[headless] the --fixture --snap run failed"; }
  [[ -s "$work/listing.png" ]] || fail "[headless] --snap wrote no PNG"
  evidence "[headless] --fixture demo --scene listing --snap: $(wc -c <"$work/listing.png" | tr -d ' ') bytes of PNG"

  # b. Real mode with an empty ssh config: starts, an empty sidebar, no panic.
  log="$work/real-tree.log"
  "${app_env[@]}" "$bin" --tree "$work/tree.json" >"$log" 2>&1 || { cat "$log" >&2; fail "[headless] the real-mode --tree run failed"; }
  grep -qi "panicked" "$log" && { cat "$log" >&2; fail "[headless] the app panicked"; }
  [[ -s "$work/tree.json" ]] || fail "[headless] --tree wrote nothing"
  grep -q '"key": *"sidebar"' "$work/tree.json" || fail "[headless] the tree has no sidebar"
  local shown system extra
  shown="$(tree_hosts "$work/tree.json")"
  system="$(system_aliases)"
  extra="$(comm -23 <(echo "$shown" | grep -v '^$' || true) <(echo "$system"))"
  [[ -z "$extra" ]] || fail "[headless] the sidebar shows hosts no config defines: $(echo $extra)"
  evidence "[headless] real mode, empty ~/.ssh/config: started; sidebar has no user hosts$([[ -n "$shown" ]] && echo " (system ssh_config: $(echo $shown))")"
}

# run_window NAME SECONDS [ENV=VALUE…] -- [APP ARGS…]: the binary in a real window, under
# `app_env` and DROMON_TRACE=1, for SECONDS, then stopped; it must still be running by then. On
# Linux without a display, under Xvfb with lavapipe.
run_window() {
  local name="$1" secs="$2"
  shift 2
  local extra_env=(DROMON_TRACE=1)
  while [[ $# -gt 0 && "$1" != -- ]]; do extra_env+=("$1"); shift; done
  [[ $# -gt 0 ]] && shift
  local wrap=(env) how=""
  if [[ "$os" == Linux && -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    command -v xvfb-run >/dev/null || fail "[$name] no display and xvfb-run is not installed"
    [[ -f "$icd" ]] || fail "[$name] no Vulkan ICD at $icd (install mesa-vulkan-drivers)"
    wrap=(xvfb-run -a -s "-screen 0 1280x800x24")
    extra_env+=(VK_ICD_FILENAMES="$icd")
    how=" under Xvfb"
  fi
  local names=() kv
  for kv in "${extra_env[@]}"; do names+=("${kv%%=*}"); done
  note "[$name] dromon${1+ $*} in a window for $secs s$how (setting ${names[*]})"
  set +e
  # The wrapper sends its own stderr (and so the app's) to $name.err itself: some xvfb-run
  # versions (Ubuntu 24.04's) run the command with 2>&1, which would mix it into stdout.
  : >"$work/$name.err"
  "${wrap[@]}" "${app_env[@]}" "${extra_env[@]}" \
    sh -c 'bin="$1" secs="$2" err="$3"; shift 3; exec 2>>"$err"; "$bin" "$@" & pid=$!; sleep "$secs"
      if kill -0 "$pid" 2>/dev/null; then kill "$pid"; wait "$pid"; echo "app: still running after $secs s, stopped (exit $?)" >&2
      else wait "$pid"; echo "app: exited early (exit $?)" >&2; exit 3; fi' \
    sh "$bin" "$secs" "$work/$name.err" ${1+"$@"} >"$work/$name.out" 2>"$work/$name.wrap"
  local status=$?
  set -e
  [[ $status -eq 0 ]] || { cat "$work/$name.out" "$work/$name.wrap" "$work/$name.err" >&2; fail "[$name] the app exited early or its wrapper failed (status $status)"; }
  if grep -qiE "panicked|couldn't start the service thread|Library .* could not be loaded" "$work/$name.err"; then
    cat "$work/$name.err" >&2
    fail "[$name] the app reported an error"
  fi
}

# trace NAME PATTERN: the run's trace lines matching PATTERN (after `dromon-trace: `).
trace() { grep -E "^dromon-trace: $2" "$work/$1.err" || true; }

window() {
  local secs="${DROMON_SMOKE_SECONDS:-8}" hosts
  use_home "$(new_home window)"
  run_window window "$secs" --
  hosts="$(trace window "Hosts count=")"
  [[ -n "$hosts" ]] || { cat "$work/window.err" >&2; fail "[window] no 'Hosts count=': the service thread never reported the hosts"; }
  evidence "[window] service thread discovered the hosts: $(echo "$hosts" | head -n1 | sed 's/^dromon-trace: //')"
  evidence "[window] $(grep -h '^app: ' "$work/window.err")"
}

connect() {
  local alias=fx-smoke secs="${DROMON_SMOKE_CONNECT_SECONDS:-10}"
  [[ "$os" == Linux ]] || fail "[connect] needs Linux (Xvfb and the Docker fixture)"
  docker info >/dev/null 2>&1 || fail "[connect] Docker is not reachable"
  note "[connect] starting the sshd fixture (DROMON_FIXTURE_KEEP=1)"
  # Bounded: a hang here (CI saw one) fails with the test's own log instead of stalling.
  DROMON_FIXTURE_KEEP=1 DROMON_REQUIRE_DOCKER=1 \
    timeout 300 cargo test -p dromon-sftp --quiet --test fixture -- --exact list_mixed_folder --nocapture >"$work/fixture.log" 2>&1 \
    || { cat "$work/fixture.log" >&2; fail "[connect] the fixture test failed or took over 5 min"; }
  # dromon-testkit: sshd fixture on 127.0.0.1:<port>, container <id>, …
  # dromon-testkit: …; clean up with `docker rm -f <id>; rm -r <key folder>`
  local port
  port="$(sed -n 's/^dromon-testkit: sshd fixture on 127\.0\.0\.1:\([0-9]*\),.*/\1/p' "$work/fixture.log" | head -n1)"
  container="$(sed -n 's/.*clean up with `docker rm -f \([0-9a-f]*\); rm -r \([^`]*\)`.*/\1/p' "$work/fixture.log" | head -n1)"
  keys="$(sed -n 's/.*clean up with `docker rm -f \([0-9a-f]*\); rm -r \([^`]*\)`.*/\2/p' "$work/fixture.log" | head -n1)"
  [[ -n "$port" && -n "$container" && -f "$keys/id_ed25519" ]] || { cat "$work/fixture.log" >&2; fail "[connect] couldn't read the fixture's port, container and keys"; }
  evidence "[connect] fixture on 127.0.0.1:$port, container $container"

  # The HOME: one alias for the fixture, its host key already known (so connecting asks
  # nothing), and the fixture's key.
  local home
  home="$(new_home connect)"
  cp "$keys/id_ed25519" "$home/.ssh/id_fixture"
  chmod 600 "$home/.ssh/id_fixture"
  ssh-keyscan -t ed25519 -p "$port" 127.0.0.1 >"$home/.ssh/known_hosts" 2>/dev/null
  [[ -s "$home/.ssh/known_hosts" ]] || fail "[connect] ssh-keyscan got no host key"
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

  # ssh finds ~/.ssh/config through the password database, not $HOME, so on its own it would
  # read the developer's config. An `ssh` first on PATH (the app runs `ssh` from PATH) passes
  # `-F <our config>` to the real one: discovery's `ssh -G` and the connections read only it.
  mkdir -p "$work/bin"
  local real_ssh
  real_ssh="$(command -v ssh)" || fail "[connect] no ssh on PATH"
  cat >"$work/bin/ssh" <<EOF
#!/bin/sh
exec "$real_ssh" -F "$home/.ssh/config" "\$@"
EOF
  chmod 755 "$work/bin/ssh"

  # How many hosts discovery finds with this HOME: the alias, plus literal aliases of the
  # system's ssh_config. A headless `--tree` run reads the same config files synchronously.
  use_home "$home"
  PATH="$work/bin:$PATH" "${app_env[@]}" "$bin" --tree "$work/connect-tree.json" >/dev/null 2>&1 || fail "[connect] the headless --tree run failed"
  local tree_hosts_list expected
  mapfile -t tree_hosts_list < <(tree_hosts "$work/connect-tree.json")
  [[ " ${tree_hosts_list[*]} " == *" $alias "* ]] || fail "[connect] the headless sidebar lacks $alias: ${tree_hosts_list[*]}"
  expected=${#tree_hosts_list[@]}
  evidence "[connect] headless discovery (--tree) lists $expected hosts: ${tree_hosts_list[*]}"

  # Rune replays a click on the host, then a double click on its `fixture` folder, on the wall
  # clock from the window's first GPU frame. `--window-script` starts its clock only at a
  # GPU-drawn frame, and a window resting on its CPU-drawn first frame never draws another, so
  # this run starts on the GPU (RUNE_CPU_FIRST=0). The pointer move first makes sure the frame
  # with the discovered hosts is laid out before the click.
  cat >"$work/connect.json" <<EOF
{"events": [
  {"t": 2.0, "type": "move", "x": 400, "y": 300},
  {"t": 3.0, "type": "tap", "key": "host-$alias"},
  {"t": 6.0, "type": "tap", "key": "rows/fixture"},
  {"t": 6.08, "type": "tap", "key": "rows/fixture"}
]}
EOF
  run_window connect "$secs" PATH="$work/bin:$PATH" RUNE_CPU_FIRST=0 -- --window-script "$work/connect.json"
  local hosts
  hosts="$(trace connect "Hosts count=$expected$")"
  [[ -n "$hosts" ]] || { cat "$work/connect.err" >&2; fail "[connect] no 'Hosts count=$expected': the service thread never discovered the hosts"; }
  local both=("$work/connect.out" "$work/connect.err")
  grep -qh "^rune-script-window" "${both[@]}" || { cat "${both[@]}" >&2; fail "[connect] Rune never replayed the script in a window"; }
  evidence "[connect] real window: $(grep -h "^rune-script-window" "${both[@]}" | head -n1)"
  if grep -qh "^rune-script-error" "${both[@]}"; then
    cat "${both[@]}" >&2
    fail "[connect] the window script failed: $(grep -h "^rune-script-error" "${both[@]}")"
  fi
  evidence "[connect] $(grep -h "^rune-script-done" "${both[@]}" | head -n1)"
  local want line pages
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

  docker rm -f "$container" >/dev/null 2>&1 || true
  container=""
  rm -rf "$keys"
  keys=""
}

for mode in "${modes[@]}"; do
  "$mode"
done
echo "smoke: OK"
