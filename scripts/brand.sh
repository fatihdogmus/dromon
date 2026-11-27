#!/usr/bin/env bash
# Rebuilds the brand kit in assets/brand/ (the app icons for every platform, the marks, the
# lockups, the social card) from its generator, assets/brand/tools/build_logo.py, with the
# repository's own Inter:
#
#   bash scripts/brand.sh                                       # rewrites assets/brand/
#   bash scripts/brand.sh --out DIR                             # writes DIR instead (to compare)
#   bash scripts/brand.sh --accent '#2DD4BF' --deep '#0F8F85'   # anything else goes to build_logo.py
#
# The wordmark is Inter SemiBold and the social card's tagline Inter Regular. The repository
# carries Inter only as a variable font (crates/dromon-app/assets/fonts/InterVariable.ttf), so
# this first cuts static instances of it, at wght 600 and 400 in the text optical size (opsz 14),
# with fontTools' instancer into target/brand/.
#
# Needs python3 with Pillow and fontTools (pip install pillow fonttools), and rsvg-convert
# (librsvg: apt install librsvg2-bin, brew install librsvg). Never edit the outputs by hand:
# change build_logo.py, run this, look at every file that changed and commit them together
# (docs/packaging.md).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

out="$root/assets/brand"
args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) out=${2:?--out needs a folder}; shift 2 ;;
    --out=*) out=${1#--out=}; shift ;;
    -h | --help) sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
    *) args+=("$1"); shift ;;
  esac
done

missing=()
command -v python3 > /dev/null || missing+=("python3")
if command -v python3 > /dev/null; then
  python3 -c 'import PIL' 2> /dev/null || missing+=("Pillow (pip install pillow)")
  python3 -c 'import fontTools' 2> /dev/null || missing+=("fontTools (pip install fonttools)")
fi
command -v rsvg-convert > /dev/null || missing+=("rsvg-convert (apt install librsvg2-bin, brew install librsvg)")
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "brand: missing:" >&2
  printf '  %s\n' "${missing[@]}" >&2
  exit 1
fi

# The output folder as an absolute path, so it means what it meant where this was run.
# build_logo.py copies itself into <out>/tools/ last without making that folder.
mkdir -p "$out/tools"
out="$(cd "$out" && pwd)"

fonts="$root/target/brand"
mkdir -p "$fonts"
variable="$root/crates/dromon-app/assets/fonts/InterVariable.ttf"
python3 -m fontTools.varLib.instancer --quiet "$variable" opsz=14 wght=600 -o "$fonts/Inter-SemiBold.ttf"
python3 -m fontTools.varLib.instancer --quiet "$variable" opsz=14 wght=400 -o "$fonts/Inter-Regular.ttf"

python3 "$root/assets/brand/tools/build_logo.py" \
  --font "$fonts/Inter-SemiBold.ttf" --font-regular "$fonts/Inter-Regular.ttf" --out "$out" ${args[@]+"${args[@]}"}
echo "brand: wrote $out"
