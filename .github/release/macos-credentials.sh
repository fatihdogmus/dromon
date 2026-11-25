#!/usr/bin/env bash
# macos-credentials.sh <dir>: imports the Developer ID Application certificate into a new
# keychain in <dir> and writes the App Store Connect API key for notarytool there. Prints nothing
# secret. Appends DROMON_KEYCHAIN, DROMON_SIGNING_IDENTITY and DROMON_NOTARY_KEY to $GITHUB_ENV.
# release.yml deletes the keychain and <dir> in an always() step.
#
# Environment: APPLE_DEVELOPER_ID_P12 (the .p12, base64), APPLE_DEVELOPER_ID_P12_PASSWORD (may
# be empty), APPLE_NOTARY_KEY (the .p8, PEM or base64), APPLE_TEAM_ID.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"
set +x

dir=$1
mkdir -p "$dir"
chmod 700 "$dir"
certificate="$dir/certificate.p12"
keychain="$dir/signing.keychain-db"
key="$dir/notary-key.p8"

(umask 077 && printf '%s' "$APPLE_DEVELOPER_ID_P12" | tr -d '\r\n ' | base64 -D > "$certificate") ||
  die "APPLE_DEVELOPER_ID_P12 is not base64."
[ -s "$certificate" ] || die "APPLE_DEVELOPER_ID_P12 is empty."

password=$(uuidgen)
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
if ! security import "$certificate" -k "$keychain" -f pkcs12 -P "${APPLE_DEVELOPER_ID_P12_PASSWORD:-}" \
  -T /usr/bin/codesign -T /usr/bin/security > /dev/null 2>&1; then
  die "APPLE_DEVELOPER_ID_P12 did not import (wrong APPLE_DEVELOPER_ID_P12_PASSWORD, or not a .p12)."
fi
rm -f "$certificate"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" > /dev/null
# codesign finds identities only in the search list.
# shellcheck disable=SC2046
security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')
identity=$(security find-identity -v -p codesigning "$keychain" |
  sed -n "s/.*\"\(Developer ID Application:.*($APPLE_TEAM_ID)\)\"/\1/p" | head -1)
[ -n "$identity" ] || die "APPLE_DEVELOPER_ID_P12 holds no Developer ID Application identity for team $APPLE_TEAM_ID."
echo "Signing as $identity."

case "$APPLE_NOTARY_KEY" in
  -----BEGIN*) (umask 077 && printf '%s\n' "$APPLE_NOTARY_KEY" > "$key") ;;
  *) (umask 077 && printf '%s' "$APPLE_NOTARY_KEY" | tr -d '\r\n ' | base64 -D > "$key") || die "APPLE_NOTARY_KEY is neither PEM nor base64." ;;
esac
grep -q -- '-----BEGIN PRIVATE KEY-----' "$key" || die "APPLE_NOTARY_KEY is not an App Store Connect API key (.p8)."

{
  echo "DROMON_KEYCHAIN=$keychain"
  echo "DROMON_SIGNING_IDENTITY=$identity"
  echo "DROMON_NOTARY_KEY=$key"
} >> "$GITHUB_ENV"
