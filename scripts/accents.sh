#!/usr/bin/env bash
# Renders the accent candidates side by side (§6.3, M4: pick one of three), headless at 2x:
# the same scenes for A Spearmint, B Teal and C Seafoam, in the dark and the light theme.
#
#   bash scripts/accents.sh            # writes target/accents/
#   OUT=/tmp/accents bash scripts/accents.sh
#
# Each image is target/accents/<accent>-<theme>-<scene>.png; with ImageMagick (`montage` or
# `magick montage`) or python3 with Pillow there is also one contact sheet per theme,
# target/accents/compare-<theme>.png: a row per scene, a column per accent. The app renders
# with an accent through `--accent a|b|c`; the default stays A until one is picked.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${OUT:-$root/target/accents}"
size="${SIZE:-1080x680}"
scale="${SCALE:-2}"

# The listing with a selection, a sheet with a primary button, the transfers popover with
# progress, the drop target. The light listing is the listing in the light column.
scenes=(listing auth-password transfers drop-list)
accents=(a b c)
themes=(dark light)

cd "$root"
cargo build -p dromon-app --quiet
bin="$root/target/debug/dromon"
rm -rf "$out"
mkdir -p "$out"

for theme in "${themes[@]}"; do
  for accent in "${accents[@]}"; do
    for scene in "${scenes[@]}"; do
      file="$out/$accent-$theme-$scene.png"
      if ! log="$(DROMON_MOCK=1 "$bin" --snap "$file" --scene "$scene" --size "$size" --scale "$scale" --theme "$theme" --accent "$accent" 2>&1)"; then
        echo "$log" >&2
        echo "failed: $accent $theme $scene" >&2
        exit 1
      fi
      echo "${file#"$root"/}"
    done
  done
done

# One contact sheet per theme, each tile at the window's size in points: rows are scenes,
# columns A, B, C.
montage_cmd=()
if command -v montage >/dev/null 2>&1; then
  montage_cmd=(montage)
elif command -v magick >/dev/null 2>&1 && magick montage -version >/dev/null 2>&1; then
  montage_cmd=(magick montage)
fi

for theme in "${themes[@]}"; do
  sheet="$out/compare-$theme.png"
  files=()
  for scene in "${scenes[@]}"; do
    for accent in "${accents[@]}"; do
      files+=("$out/$accent-$theme-$scene.png")
    done
  done
  if [[ ${#montage_cmd[@]} -gt 0 ]]; then
    labels=()
    for f in "${files[@]}"; do
      name="$(basename "$f" .png)"
      case "$name" in
        a-*) labels+=(-label "A Spearmint · ${name#a-}") ;;
        b-*) labels+=(-label "B Teal · ${name#b-}") ;;
        c-*) labels+=(-label "C Seafoam · ${name#c-}") ;;
      esac
      labels+=("$f")
    done
    # Each tile at the window's size in points (half of a 2x render), labelled underneath.
    "${montage_cmd[@]}" "${labels[@]}" -tile "${#accents[@]}x" -geometry "${size}+16+16" -background '#808080' -fill white -pointsize 24 "$sheet"
  elif python3 -c 'import PIL' >/dev/null 2>&1; then
    python3 - "$sheet" "${#accents[@]}" "$size" "${files[@]}" <<'PY'
import sys
from PIL import Image

sheet, cols, size, files = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4:]
images = [Image.open(f).convert("RGB") for f in files]
w, h = (int(v) for v in size.split("x"))
gap = 16
rows = (len(images) + cols - 1) // cols
out = Image.new("RGB", (cols * w + (cols + 1) * gap, rows * h + (rows + 1) * gap), (128, 128, 128))
for i, im in enumerate(images):
    r, c = divmod(i, cols)
    out.paste(im.resize((w, h), Image.LANCZOS), (gap + c * (w + gap), gap + r * (h + gap)))
out.save(sheet)
PY
  else
    echo "no ImageMagick montage or Pillow: skipped the contact sheets" >&2
    break
  fi
  echo "${sheet#"$root"/}"
done
