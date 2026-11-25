#!/usr/bin/env bash
# Regenerates or checks the UI goldens: every scene of the dromon binary, rendered headless at 2x.
#
#   bash scripts/goldens.sh            # writes tests/goldens/<scene>-<theme>@2x.png
#   bash scripts/goldens.sh --check    # renders into target/goldens/new and compares (CI)
#   OUT=/tmp/snaps bash scripts/goldens.sh
#
# Scenes render with --theme dark; the `light` scene forces the light theme itself, so its
# file is light-light@2x.png. Builds once, then loops.
#
# --check passes when every new image has the committed one's size and no pixel channel differs
# by more than GOLDEN_TOLERANCE (default 2 of 255). Different Mesa (software Vulkan) versions
# round a few pixels by one step; a real UI change moves far more than that. On a mismatch it
# writes the new image and a diff (changed pixels in red) to target/goldens/ and exits 1; look at
# them, then run without --check and commit the images you meant. Needs python3 with Pillow.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
check=0
case "${1:-}" in
  --check) check=1 ;;
  "") ;;
  *) echo "usage: goldens.sh [--check]" >&2; exit 2 ;;
esac
size="${SIZE:-1080x680}"
scale="${SCALE:-2}"
if [[ $check == 1 ]]; then
  out="$root/target/goldens/new"
  rm -rf "$root/target/goldens"
else
  out="${OUT:-$root/tests/goldens}"
fi

cd "$root"
cargo build -p dromon-app --quiet
bin="$root/target/debug/dromon"
mkdir -p "$out"

scenes=(welcome connecting listing listing-sorted-size big-folder empty-folder permission-denied reconnecting filter light
  auth-password auth-password-error host-key confirm connection-log
  transfers transfers-uploads transfer-conflict drop-list drop-folder toolbar-ring quit-confirm)

for scene in "${scenes[@]}"; do
  theme=dark
  [[ "$scene" == light ]] && theme=light
  file="$out/$scene-$theme@${scale}x.png"
  # Rune prints frame timings on stderr; show them only when a render fails. DROMON_MOCK runs
  # the binary on the built-in demo fixture (assets/fixtures/demo.json), off the network side
  # and the user's ssh config; scenes start from it.
  if ! log="$(DROMON_MOCK=1 "$bin" --snap "$file" --scene "$scene" --size "$size" --scale "$scale" --theme dark 2>&1)"; then
    echo "$log" >&2
    echo "failed: $scene" >&2
    exit 1
  fi
  echo "${file#"$root"/}"
done

[[ $check == 1 ]] || exit 0

python3 - "$root/tests/goldens" "$out" "$root/target/goldens/diff" "${GOLDEN_TOLERANCE:-2}" <<'PY'
import os, sys
from PIL import Image, ImageChops

golden_dir, new_dir, diff_dir, tolerance = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
failed = []
for name in sorted(os.listdir(new_dir)):
    new = Image.open(os.path.join(new_dir, name)).convert("RGBA")
    golden_path = os.path.join(golden_dir, name)
    if not os.path.exists(golden_path):
        failed.append(f"{name}: no committed golden")
        continue
    golden = Image.open(golden_path).convert("RGBA")
    if new.size != golden.size:
        failed.append(f"{name}: {new.size[0]}x{new.size[1]}, golden is {golden.size[0]}x{golden.size[1]}")
        continue
    # Per pixel, the largest channel difference; then the pixels over the tolerance.
    delta = ImageChops.difference(new, golden).split()
    worst = delta[0]
    for band in delta[1:]:
        worst = ImageChops.lighter(worst, band)
    over = worst.point(lambda v: 255 if v > tolerance else 0)
    count = over.histogram()[255]
    if count:
        os.makedirs(diff_dir, exist_ok=True)
        marked = golden.convert("RGB")
        marked.paste((255, 0, 0), mask=over)
        marked.save(os.path.join(diff_dir, name))
        new.save(os.path.join(diff_dir, name.replace(".png", ".new.png")))
        failed.append(f"{name}: {count} pixels differ by more than {tolerance}")
    else:
        print(f"ok {name}")
for line in failed:
    print(f"CHANGED {line}")
if failed:
    print(f"Goldens changed: see target/goldens/diff. If the change is meant, run scripts/goldens.sh and commit.")
    sys.exit(1)
PY
