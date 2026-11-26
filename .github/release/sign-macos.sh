#!/usr/bin/env bash
# sign-macos.sh <Dromon.app.zip> <out dir> <version> <Info.plist> <unsigned DMG>: signs
# Dromon.app with the Developer ID identity (hardened runtime, secure timestamp, no
# entitlements), notarizes and staples it, then puts it into Dromon-<version>-macos-universal.dmg
# in place of the unsigned DMG's ad-hoc app (keeping that DMG's installer window: build-app.sh),
# and signs, notarizes and staples the DMG too.
#
# No entitlements: the hardened runtime limits JIT and unsigned executable memory, DYLD_*
# variables, loading libraries not signed by Apple or the team, debugging, and the camera,
# microphone, location, contacts, calendars, photos and Apple Events. Dromon uses none of these:
# it draws with Metal, links only system libraries, and starts /usr/bin/ssh and its own signed
# executable (the askpass helper), which the hardened runtime allows. Add one only for a feature
# that needs it.
#
# <Info.plist> is packaging/macos/Info.plist, read as data for the bundle identifier.
# Environment: DROMON_KEYCHAIN, DROMON_SIGNING_IDENTITY, DROMON_NOTARY_KEY
# (macos-credentials.sh), APPLE_NOTARY_KEY_ID, APPLE_NOTARY_ISSUER_ID, APPLE_TEAM_ID.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

input=${1:?usage: sign-macos.sh <Dromon.app.zip> <out dir> <version> <Info.plist> <unsigned DMG>}
out=${2:?}
version=${3:?}
plist=${4:?}
layout=${5:?usage: sign-macos.sh <Dromon.app.zip> <out dir> <version> <Info.plist> <unsigned DMG>}
bundle_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist")
dmg="$out/Dromon-$version-macos-universal.dmg"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"
/usr/bin/ditto -x -k "$input" "$work/unpacked"
app="$work/unpacked/Dromon.app"
if [ ! -d "$app" ] || [ "$(find "$work/unpacked" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" != 1 ]; then
  die "The macOS build archive must hold exactly Dromon.app."
fi
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" = "$bundle_id" ] ||
  die "Dromon.app's bundle identifier is not $bundle_id."
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" = "${version%%-*}" ] ||
  die "Dromon.app is not version ${version%%-*}."

# The bundle's only code is its executable; anything else that is Mach-O fails the release.
found=$(cd "$app" && find . -type f -exec sh -c 'file -b "$1" | grep -q Mach-O && printf "%s\n" "${1#./}"' _ {} \; | LC_ALL=C sort)
[ "$found" = Contents/MacOS/dromon ] || die "Dromon.app holds Mach-O code other than Contents/MacOS/dromon: $found"

notarize() {
  xcrun notarytool submit "$1" --key "$DROMON_NOTARY_KEY" --key-id "$APPLE_NOTARY_KEY_ID" \
    --issuer "$APPLE_NOTARY_ISSUER_ID" --wait --timeout 45m --output-format json > "$work/notary.json" ||
    { cat "$work/notary.json" >&2; die "Notarization of $(basename "$1") failed (xcrun notarytool log <id> shows why)."; }
  grep -q '"status" *: *"Accepted"' "$work/notary.json" || { cat "$work/notary.json" >&2; die "Notarization of $(basename "$1") was not accepted."; }
  echo "notarized $(basename "$1")"
}

# Inside out: the executable is the bundle's only nested code, signed with the bundle.
codesign --force --timestamp --options runtime --identifier "$bundle_id" \
  --keychain "$DROMON_KEYCHAIN" --sign "$DROMON_SIGNING_IDENTITY" "$app/Contents/MacOS/dromon"
codesign --force --timestamp --options runtime \
  --keychain "$DROMON_KEYCHAIN" --sign "$DROMON_SIGNING_IDENTITY" "$app"
codesign --verify --deep --strict --verbose=2 "$app"
signature=$(codesign -d --verbose=4 "$app" 2>&1)
grep -Fq "TeamIdentifier=$APPLE_TEAM_ID" <<< "$signature" || die "Dromon.app is not signed by team $APPLE_TEAM_ID."
grep -Eq 'flags=.*runtime' <<< "$signature" || die "Dromon.app is not signed with the hardened runtime."

/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$work/notarize.zip"
notarize "$work/notarize.zip"
xcrun stapler staple "$app"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app"

# The DMG: build-macos's installer window (background, icon positions: build-app.sh) with the
# signed app in place of its ad-hoc one. Apple's hdiutil only: a writable copy, grown for the
# signature and the ticket, the app swapped, compressed again. The layout stays byte for byte.
hdiutil convert -quiet "$layout" -format UDRW -o "$work/rw.dmg"
# -limits: minimum, current and maximum size in 512-byte sectors; 64 MB more than now.
current=$(hdiutil resize -limits "$work/rw.dmg" | awk 'NR == 1 { print $2 }')
hdiutil resize -quiet -sectors "$((current + 131072))" "$work/rw.dmg"
hdiutil attach -quiet -nobrowse -noautoopen -readwrite -mountpoint "$work/volume" "$work/rw.dmg"
[ -d "$work/volume/Dromon.app" ] && [ -f "$work/volume/.DS_Store" ] ||
  { hdiutil detach -quiet "$work/volume"; die "$(basename "$layout") has no Dromon.app or window layout."; }
rm -rf "$work/volume/Dromon.app"
/usr/bin/ditto "$app" "$work/volume/Dromon.app"
hdiutil detach -quiet "$work/volume" || hdiutil detach -quiet -force "$work/volume"
rm -f "$dmg"
hdiutil convert -quiet "$work/rw.dmg" -format UDZO -imagekey zlib-level=9 -o "$dmg"
codesign --force --timestamp --keychain "$DROMON_KEYCHAIN" --sign "$DROMON_SIGNING_IDENTITY" "$dmg"
notarize "$dmg"
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
summary "- macOS: $(basename "$dmg") and Dromon.app signed by $DROMON_SIGNING_IDENTITY, notarized and stapled."
ls -l "$out"
