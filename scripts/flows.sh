#!/usr/bin/env bash
# Renders the UI flows in tests/flows/<name>/ headless, as artifacts to look at: the real app on
# a fixture (`--fixture`), driven by Rune's `--script`. No network, no display, no ssh config.
#
#   bash scripts/flows.sh                      # every flow, into target/flows
#   bash scripts/flows.sh /tmp/flows           # every flow, into /tmp/flows
#   bash scripts/flows.sh /tmp/flows filter    # just these flows
#   bash scripts/flows.sh filter two-factor    # just these, into target/flows
#
# The first argument is the out dir when it contains a `/` or is an existing folder (`./out`,
# not `out`); otherwise every argument names a flow.
#
# A flow is a folder tests/flows/<name>/ with
#   script.json   Rune's script ({"description": "...", "events": [...]}, README "Agent tooling";
#                 end it with a {"type": "mark", "name": "end"} event), and optionally
#   fixture.json  the fixture to run on (docs/fixtures.md); without one, the built-in demo
#                 (crates/dromon-app/assets/fixtures/demo.json).
#
# For each flow it writes
#   <out>/<name>.png        a strip of frames, one every FLOWS_EVERY frames (60 fps) to the end mark
#   <out>/<name>.tree.json  Rune's element tree (`--tree`) at the end
#   <out>/<name>.mp4        a video, only when ffmpeg is on PATH
#   <out>/<name>.log        what the run printed (with DROMON_TRACE=1: one line per service event)
# and prints one line: `flow <name>: ok <outputs>` or `flow <name>: FAILED (...)`.
#
# Env: FLOWS_EVERY (30: a frame every 0.5 s), FLOWS_COLS (4), FLOWS_SCALE (1), FLOWS_SIZE
# (1080x680), FLOWS_THEME (dark). Builds once (debug: Rune's tooling is always on there).
# Exits 0 when every flow rendered, 1 when any failed, 2 on bad arguments.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
flows="$root/tests/flows"
demo="$root/crates/dromon-app/assets/fixtures/demo.json"

out="$root/target/flows"
if [[ $# -gt 0 && ( "$1" == */* || -d "$1" ) ]]; then
  # Relative to where the script was run from.
  mkdir -p "$1" && out="$(cd "$1" && pwd)" || { echo "flows: can't use $1 as the out dir" >&2; exit 2; }
  shift
fi
cd "$root" || exit 1
names=("$@")
if [[ ${#names[@]} -eq 0 ]]; then
  for d in "$flows"/*/; do names+=("$(basename "$d")"); done
fi
for name in "${names[@]}"; do
  [[ -f "$flows/$name/script.json" ]] || { echo "flows: no flow $name (tests/flows/$name/script.json; an out dir needs a /, e.g. ./$name)" >&2; exit 2; }
done

# DROMON_BIN: a dromon binary built already (scripts/smoke-app.sh says why).
if [[ -n ${DROMON_BIN:-} ]]; then
  bin=$DROMON_BIN
  [[ $bin == /* ]] || bin="$root/$bin"
else
  cargo build -p dromon-app --quiet || { echo "flows: the build failed" >&2; exit 1; }
  bin="$root/target/debug/dromon"
fi
mkdir -p "$out"
have_ffmpeg=0
command -v ffmpeg >/dev/null 2>&1 && have_ffmpeg=1

failed=0
for name in "${names[@]}"; do
  dir="$flows/$name"
  fixture="$dir/fixture.json"
  [[ -f "$fixture" ]] || fixture="$demo"
  png="$out/$name.png"
  tree="$out/$name.tree.json"
  log="$out/$name.log"
  rm -f "$png" "$tree" "$out/$name.mp4"
  video=()
  [[ $have_ffmpeg -eq 1 ]] && video=(--video "$out/$name.mp4")
  DROMON_TRACE=1 "$bin" --fixture "$fixture" --script "$dir/script.json" --duration auto \
    --every "${FLOWS_EVERY:-30}" --cols "${FLOWS_COLS:-4}" --size "${FLOWS_SIZE:-1080x680}" \
    --scale "${FLOWS_SCALE:-1}" --theme "${FLOWS_THEME:-dark}" \
    --strip "$png" --tree "$tree" "${video[@]}" >"$log" 2>&1
  status=$?
  if [[ $status -ne 0 ]]; then
    echo "flow $name: FAILED (exit $status; last lines of $log follow)"
    tail -n 5 "$log" | sed 's/^/  /'
    failed=1
    continue
  fi
  missing=()
  for f in "$png" "$tree" "${video[@]:1}"; do [[ -s "$f" ]] || missing+=("$f"); done
  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "flow $name: FAILED (no ${missing[*]}; see $log)"
    failed=1
    continue
  fi
  outputs="${png#"$root"/} ${tree#"$root"/}"
  [[ $have_ffmpeg -eq 1 ]] && outputs+=" ${out#"$root"/}/$name.mp4"
  echo "flow $name: ok $outputs"
done
exit "$failed"
