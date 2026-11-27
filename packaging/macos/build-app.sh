#!/usr/bin/env bash
# build-app.sh <out dir>: builds Dromon's release binary for Apple silicon (aarch64) and Intel
# (x86_64), joins them into one universal executable (lipo), and writes
#
#   <out>/Dromon.app                              ad-hoc signed (Apple silicon runs nothing
#                                                 unsigned)
#   <out>/Dromon.app.zip                          the app for the release signing job (ditto keeps
#                                                 permissions and signatures)
#   <out>/Dromon-<version>-macos-universal.dmg    the installer window: the app, an Applications
#                                                 link and the background's arrow between them
#                                                 (dmgbuild, packaging/macos/dmg/). The release
#                                                 signs the app inside it (sign-macos.sh); without
#                                                 Apple credentials it ships as is.
#
# The DMG's layout needs dmgbuild (packaging/macos/dmg/requirements.txt, pinned by hash), which
# needs Python 3.11+: the first new enough python3 on PATH (macOS's /usr/bin/python3 is 3.9).
#
# One app for every Mac: each runs its own half natively. On an Apple silicon Mac with Rosetta
# the Intel half's `--version` is checked too.
#
# Runs on macOS with Xcode's command-line tools. The version is [workspace.package].version from
# Cargo.toml; the bundle's identity is packaging/macos/Info.plist. The release workflow signs,
# notarizes and staples the app and a new DMG in .github/release/sign-macos.sh.
set -euo pipefail

out=${1:?usage: build-app.sh <out dir>}
root=$(cd "$(dirname "$0")/../.." && pwd)
version=$(awk '/^\[/ { s = $0 } s == "[workspace.package]" && $1 == "version" { gsub(/"/, "", $3); print $3; exit }' "$root/Cargo.toml")
[ -n "$version" ] || { echo "no [workspace.package] version in Cargo.toml" >&2; exit 1; }
short=${version%%-*}
# The binary's minimum macOS (LC_BUILD_VERSION minos) matches Info.plist's LSMinimumSystemVersion.
minimum=$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$root/packaging/macos/Info.plist")
export MACOSX_DEPLOYMENT_TARGET=$minimum

targets=(aarch64-apple-darwin x86_64-apple-darwin)
rustup target add "${targets[@]}"
for target in "${targets[@]}"; do
  cargo build --release --locked -p dromon-app --bin dromon --target "$target" --manifest-path "$root/Cargo.toml"
done

mkdir -p "$out"
out=$(cd "$out" && pwd)
app="$out/Dromon.app"
dmg="$out/Dromon-$version-macos-universal.dmg"
rm -rf "$app" "$out/Dromon.app.zip" "$dmg"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/licenses"
built=${CARGO_TARGET_DIR:-$root/target}
lipo -create -output "$app/Contents/MacOS/dromon" "$built/aarch64-apple-darwin/release/dromon" "$built/x86_64-apple-darwin/release/dromon"
archs=$(lipo -archs "$app/Contents/MacOS/dromon" | tr ' ' '\n' | sort | xargs)
[ "$archs" = "arm64 x86_64" ] || { echo "dromon is '$archs', not 'arm64 x86_64'" >&2; exit 1; }
for arch in arm64 x86_64; do
  minos=$(otool -arch "$arch" -l "$app/Contents/MacOS/dromon" | awk '/LC_BUILD_VERSION/ { found = 1 } found && $1 == "minos" { print $2; exit }')
  [ "$minos" = "$minimum" ] || { echo "dromon ($arch) needs macOS $minos, not $minimum" >&2; exit 1; }
done

sed "s/@VERSION@/$short/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
cp "$root/assets/brand/macos/Dromon.icns" "$app/Contents/Resources/Dromon.icns"
cp "$root/LICENSE" "$app/Contents/Resources/licenses/LICENSE.txt"
cp "$root/crates/dromon-app/assets/fonts/Inter-LICENSE.txt" "$root/crates/dromon-app/assets/fonts/GeistMono-OFL.txt" "$app/Contents/Resources/licenses/"

codesign --force --sign - "$app"
codesign --verify --strict --verbose=2 "$app"
for arch in arm64 x86_64; do
  # This Mac's own half, and the other one when it can run it (Rosetta on Apple silicon).
  if [ "$arch" != "$(uname -m)" ] && ! arch "-$arch" /usr/bin/true 2> /dev/null; then
    echo "($arch half not run: this Mac can't)"
    continue
  fi
  reported=$(arch "-$arch" "$app/Contents/MacOS/dromon" --version)
  [ "$reported" = "dromon $version" ] || { echo "dromon --version ($arch) printed '$reported', not 'dromon $version'" >&2; exit 1; }
done

/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$out/Dromon.app.zip"

# The installer window (packaging/macos/dmg/): dmgbuild in a throwaway virtualenv.
python=""
for candidate in python3.13 python3.12 python3.11 python3; do
  if command -v "$candidate" > /dev/null &&
    "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 11))' 2> /dev/null; then
    python=$candidate
    break
  fi
done
[ -n "$python" ] || { echo "the DMG's layout needs Python 3.11+ for dmgbuild (brew install python)" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
dmgdir="$root/packaging/macos/dmg"
"$python" -m venv "$work/venv"
"$work/venv/bin/pip" install --quiet --disable-pip-version-check --no-deps --require-hashes -r "$dmgdir/requirements.txt"
# One TIFF with both resolutions, so Retina displays get the 2x background.
tiffutil -cathidpicheck "$dmgdir/background.png" "$dmgdir/background@2x.png" -out "$work/background.tiff" 2> /dev/null
"$work/venv/bin/dmgbuild" -s "$dmgdir/settings.py" -D app="$app" -D background="$work/background.tiff" Dromon "$dmg"
ls -l "$out"
