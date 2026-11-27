#!/usr/bin/env bash
# verify-dmg.sh <dmg> <version> <signed: true|false> [<team id>] [--copy-to <dir>]: opens the
# DMG as a user would (hdiutil attach) and checks Dromon.app:
#
#   - the DMG holds Dromon.app, an /Applications link and its installer window (the layout's
#     .DS_Store and the background image)
#   - Info.plist: the bundle id of packaging/macos/Info.plist, the version, macOS 14 minimum,
#     and the icon its CFBundleIconFile names in Contents/Resources/
#   - the executable is universal (arm64 and x86_64) and prints "dromon <version>" for --version
#     (this Mac's half, and the Intel half too on Apple silicon with Rosetta)
#   - codesign --verify --deep --strict passes (ad-hoc or Developer ID)
#   - signed: Developer ID of <team id> with the hardened runtime, notarization tickets stapled
#     to the DMG and the app (xcrun stapler validate), and Gatekeeper accepts both (spctl)
#
# --copy-to copies Dromon.app out of the DMG (for a snapshot run after it is detached).
set -euo pipefail

dmg=${1:?usage: verify-dmg.sh <dmg> <version> <signed> [<team id>] [--copy-to <dir>]}
version=${2:?}
signed=${3:?}
team=""
copy_to=""
shift 3
while [ $# -gt 0 ]; do
  case "$1" in
    --copy-to) copy_to=${2:?}; shift 2 ;;
    *) team=$1; shift ;;
  esac
done
root=$(cd "$(dirname "$0")/../.." && pwd)
template="$root/packaging/macos/Info.plist"

fail() {
  echo "::error::$*" >&2
  exit 1
}
summary() {
  echo "$*"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then echo "- $*" >> "$GITHUB_STEP_SUMMARY"; fi
}

mount=$(mktemp -d)
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mount" "$dmg"
trap 'hdiutil detach -quiet "$mount" || hdiutil detach -force -quiet "$mount"' EXIT
app="$mount/Dromon.app"
[ -d "$app" ] || fail "$(basename "$dmg") has no Dromon.app"
[ "$(readlink "$mount/Applications")" = /Applications ] || fail "$(basename "$dmg") has no /Applications link"
# The installer window: Finder's layout (.DS_Store) and the background it points at.
[ -s "$mount/.DS_Store" ] || fail "$(basename "$dmg") has no window layout (.DS_Store)"
# dmgbuild writes it at the top of the volume as .background.tiff (a .background/ folder by hand).
compgen -G "$mount/.background*" > /dev/null || fail "$(basename "$dmg") has no background image"

plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$2"; }
[ "$(plist CFBundleIdentifier "$app/Contents/Info.plist")" = "$(plist CFBundleIdentifier "$template")" ] || fail "wrong bundle identifier"
[ "$(plist CFBundleShortVersionString "$app/Contents/Info.plist")" = "${version%%-*}" ] || fail "wrong CFBundleShortVersionString"
[ "$(plist LSMinimumSystemVersion "$app/Contents/Info.plist")" = "$(plist LSMinimumSystemVersion "$template")" ] || fail "wrong LSMinimumSystemVersion"
icon="$(plist CFBundleIconFile "$app/Contents/Info.plist").icns"
[ -s "$app/Contents/Resources/$icon" ] || fail "Dromon.app has no Contents/Resources/$icon (its CFBundleIconFile)"
archs=$(lipo -archs "$app/Contents/MacOS/dromon" | tr ' ' '\n' | sort | xargs)
[ "$archs" = "arm64 x86_64" ] || fail "dromon is '$archs', not universal (arm64 x86_64)"
ran=()
for arch in arm64 x86_64; do
  # This Mac's own half, and the other one when it can run it (Rosetta on Apple silicon).
  if [ "$arch" != "$(uname -m)" ] && ! arch "-$arch" /usr/bin/true 2> /dev/null; then continue; fi
  reported=$(arch "-$arch" "$app/Contents/MacOS/dromon" --version)
  [ "$reported" = "dromon $version" ] || fail "dromon --version ($arch) printed '$reported', not 'dromon $version'"
  ran+=("$arch")
done
reported="dromon $version (ran: ${ran[*]})"
codesign --verify --deep --strict --verbose=2 "$app"
summary "$(basename "$dmg") on $(uname -m): Dromon.app ($archs), \`dromon --version\` = $reported, codesign --verify --deep --strict passes"

if [ "$signed" = true ]; then
  [ -n "$team" ] || fail "a signed check needs the team id"
  signature=$(codesign -d --verbose=4 "$app" 2>&1)
  grep -Fq "TeamIdentifier=$team" <<< "$signature" || fail "Dromon.app is not signed by team $team"
  grep -Fq 'Authority=Developer ID Application:' <<< "$signature" || fail "Dromon.app is not signed for Developer ID distribution"
  grep -Eq 'flags=.*runtime' <<< "$signature" || fail "Dromon.app lacks the hardened runtime"
  xcrun stapler validate "$dmg"
  xcrun stapler validate "$app"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
  spctl --assess --type execute --verbose=2 "$app"
  summary "$(basename "$dmg"): Developer ID (team $team), hardened runtime, notarized and stapled, Gatekeeper accepts it"
else
  summary "$(basename "$dmg"): ad-hoc signed, not notarized (no Apple credentials)"
fi

if [ -n "$copy_to" ]; then
  mkdir -p "$copy_to"
  /usr/bin/ditto "$app" "$copy_to/Dromon.app"
fi
