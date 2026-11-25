#!/usr/bin/env bash
# Regenerates the UI goldens: every scene of the dromon binary, rendered headless at 2x.
#
#   bash scripts/goldens.sh            # writes tests/goldens/<scene>-<theme>@2x.png
#   OUT=/tmp/snaps bash scripts/goldens.sh
#
# Scenes render with --theme dark; the `light` scene forces the light theme itself, so its
# file is light-light@2x.png. Builds once, then loops.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${OUT:-$root/tests/goldens}"
size="${SIZE:-1080x680}"
scale="${SCALE:-2}"

cd "$root"
cargo build -p dromon-app --quiet
bin="$root/target/debug/dromon"
mkdir -p "$out"

scenes=(welcome connecting listing listing-sorted-size big-folder empty-folder permission-denied reconnecting filter light
  auth-password auth-password-error host-key confirm connection-log)

for scene in "${scenes[@]}"; do
  theme=dark
  [[ "$scene" == light ]] && theme=light
  file="$out/$scene-$theme@${scale}x.png"
  # Rune prints frame timings on stderr; show them only when a render fails. DROMON_MOCK keeps
  # the binary off the network side and the user's ssh config (scenes are mock-built anyway).
  if ! log="$(DROMON_MOCK=1 "$bin" --snap "$file" --scene "$scene" --size "$size" --scale "$scale" --theme dark 2>&1)"; then
    echo "$log" >&2
    echo "failed: $scene" >&2
    exit 1
  fi
  echo "${file#"$root"/}"
done
