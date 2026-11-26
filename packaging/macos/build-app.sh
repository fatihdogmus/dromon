#!/usr/bin/env bash
# build-app.sh <out dir>: builds Dromon's release binary for Apple silicon (aarch64) and Intel
# (x86_64), joins them into one universal executable (lipo), and writes
#
#   <out>/Dromon.app                              ad-hoc signed (Apple silicon runs nothing
#                                                 unsigned)
#   <out>/Dromon.app.zip                          the app for the release signing job (ditto keeps
#                                                 permissions and signatures)
#   <out>/Dromon-<version>-macos-universal.dmg    unsigned DMG with an /Applications link (what
#                                                 ships when the release has no Apple credentials)
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
cp "$root/packaging/icon/dromon.icns" "$app/Contents/Resources/dromon.icns"
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

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
/usr/bin/ditto "$app" "$staging/Dromon.app"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname Dromon -fs HFS+ -format UDZO -srcfolder "$staging" "$dmg"
ls -l "$out"
