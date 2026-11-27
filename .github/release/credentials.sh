#!/usr/bin/env bash
# credentials.sh: which signing credentials the release environment holds, and whether this run
# may go on without some of them. Reads only variables and, for secrets, whether each is set
# (release.yml passes `secrets.X != ''`, never a secret's value).
#
#   Linux (GPG)  required for a release (it costs nothing); a dry run without it warns.
#   Windows      WINDOWS_SIGNING = azure | sslcom | signpath; unset ships the MSI unsigned.
#   macOS        Developer ID + notary key; none ships the app ad-hoc signed and unnotarized.
#   Updates      MINISIGN_SECRET_KEY, required for a release (the app's updater installs only
#                a minisign-signed SHA256SUMS); a dry run without it warns and skips it.
#
# A platform whose credentials are set only in part fails the run (a half-configured backend is
# a mistake, not a choice). Repository variables DROMON_REQUIRE_SIGNED_WINDOWS and
# DROMON_REQUIRE_SIGNED_MACOS = true make a release fail when that platform would ship
# unsigned (a dry run warns).
#
# Outputs: windows (none|azure|sslcom|signpath), macos (true|false), apple-team-id, gpg
# (true|false), gpg-fingerprint, minisign (true|false).
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

release=false
[ "${MODE:?}" != release ] || release=true

# The names in "$@" that are unset or empty (a HAS_* flag counts when it is "true").
missing() {
  local name value
  for name in "$@"; do
    value=$(printenv "$name" || true)
    case "$name" in HAS_*) [ "$value" = true ] || printf ' %s' "${name#HAS_}" ;; *) [ -n "$value" ] || printf ' %s' "$name" ;; esac
  done
}
# How many of "$@" are set.
present() {
  local count=0 name value
  for name in "$@"; do
    value=$(printenv "$name" || true)
    case "$name" in HAS_*) [ "$value" != true ] || count=$((count + 1)) ;; *) [ -z "$value" ] || count=$((count + 1)) ;; esac
  done
  echo "$count"
}
# Fail a release, warn a dry run.
required() {
  if [ "$release" = true ]; then die "$1"; else warn "$1 (a release would fail)"; fi
}

summary "### Signing credentials"

# ---- Windows -----------------------------------------------------------------------------
windows=${WINDOWS_SIGNING:-none}
case "$windows" in
  azure) need=(AZURE_CLIENT_ID AZURE_TENANT_ID AZURE_SUBSCRIPTION_ID AZURE_ARTIFACT_SIGNING_ENDPOINT AZURE_ARTIFACT_SIGNING_ACCOUNT AZURE_ARTIFACT_SIGNING_PROFILE) ;;
  sslcom) need=(HAS_ESIGNER_USERNAME HAS_ESIGNER_PASSWORD HAS_ESIGNER_TOTP_SECRET HAS_ESIGNER_CREDENTIAL_ID) ;;
  signpath) need=(SIGNPATH_ORGANIZATION_ID SIGNPATH_PROJECT_SLUG SIGNPATH_SIGNING_POLICY_SLUG HAS_SIGNPATH_API_TOKEN) ;;
  none) need=() ;;
  *) die "WINDOWS_SIGNING is '$windows'; set it to azure, sslcom or signpath, or delete it to ship unsigned." ;;
esac
gaps=$(missing ${need[@]+"${need[@]}"})
[ -z "$gaps" ] || die "WINDOWS_SIGNING=$windows, but the release environment lacks:$gaps."
if [ "$windows" = none ]; then
  message="Windows: no WINDOWS_SIGNING; the MSI and dromon.exe ship unsigned (SmartScreen warns users)."
  if [ "${REQUIRE_SIGNED_WINDOWS:-}" = true ]; then required "$message DROMON_REQUIRE_SIGNED_WINDOWS is true."; else warn "$message"; fi
  summary "- :warning: $message"
else
  summary "- Windows: Authenticode through **$windows**$([ "$windows" != signpath ] || echo ' (SignPath Foundation; each release waits for approval in SignPath)')."
fi

# ---- macOS -------------------------------------------------------------------------------
apple=(HAS_APPLE_DEVELOPER_ID_P12 APPLE_TEAM_ID HAS_APPLE_NOTARY_KEY APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID)
if [ "$(present "${apple[@]}")" = 0 ]; then
  macos=false
  message="macOS: no Apple credentials; Dromon.app ships ad-hoc signed and not notarized (Gatekeeper blocks the first open)."
  if [ "${REQUIRE_SIGNED_MACOS:-}" = true ]; then required "$message DROMON_REQUIRE_SIGNED_MACOS is true."; else warn "$message"; fi
  summary "- :warning: $message"
else
  gaps=$(missing "${apple[@]}")
  [ -z "$gaps" ] || die "The release environment holds some Apple credentials but lacks:$gaps."
  [[ "$APPLE_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || die "APPLE_TEAM_ID '$APPLE_TEAM_ID' is not a 10-character team ID."
  macos=true
  summary "- macOS: Developer ID (team $APPLE_TEAM_ID), notarized and stapled."
fi

# ---- Linux (GPG) ---------------------------------------------------------------------------
gpg_names=(HAS_RELEASE_GPG_PRIVATE_KEY RELEASE_GPG_FINGERPRINT)
fingerprint=""
if [ "$(present "${gpg_names[@]}")" = 0 ]; then
  gpg=false
  required "Linux: no RELEASE_GPG_PRIVATE_KEY / RELEASE_GPG_FINGERPRINT; the packages and SHA256SUMS get no GPG signatures."
  summary "- :warning: Linux: no release GPG key; packages and SHA256SUMS unsigned."
else
  gaps=$(missing "${gpg_names[@]}")
  [ -z "$gaps" ] || die "The release environment holds part of the GPG key setup but lacks:$gaps."
  fingerprint=$(printf '%s' "$RELEASE_GPG_FINGERPRINT" | tr -d ' ' | tr '[:lower:]' '[:upper:]')
  [[ "$fingerprint" =~ ^[0-9A-F]{40}$ ]] || die "RELEASE_GPG_FINGERPRINT is not a 40-hex-digit fingerprint."
  gpg=true
  summary "- Linux: packages and SHA256SUMS GPG-signed by $fingerprint."
fi

# ---- Updates (minisign) --------------------------------------------------------------------
if [ "${HAS_MINISIGN_SECRET_KEY:-}" = true ]; then
  minisign=true
  summary "- Updates: SHA256SUMS minisign-signed with the key the app trusts (packaging/update/release-key.pub)."
else
  minisign=false
  required "Updates: no MINISIGN_SECRET_KEY; SHA256SUMS gets no minisign signature, so the app installs no update from this release."
  summary "- :warning: Updates: no minisign key; the app's updater would refuse this release."
fi
summary "- All files: Sigstore build-provenance attestations (published releases only)."

output windows "$windows"
output macos "$macos"
output apple-team-id "$([ "$macos" != true ] || echo "$APPLE_TEAM_ID")"
output gpg "$gpg"
output gpg-fingerprint "$fingerprint"
output minisign "$minisign"
