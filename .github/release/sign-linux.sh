#!/usr/bin/env bash
# sign-linux.sh <release dir> <out dir>: GPG-signs the release with the dedicated release key and
# writes only signatures (the release's bytes stay as verified):
#
#   <name>.deb.asc           armored detached signature of each Dromon-*.deb
#   <name>.pkg.tar.zst.sig   binary detached signature of each Dromon-*.pkg.tar.zst (pacman's)
#   SHA256SUMS.asc           armored detached signature of the release-wide SHA256SUMS
#   dromon-release-key.asc   the public key, so later jobs and users can check them
#
# The key goes into a temporary GNUPGHOME, deleted on exit (its agent stopped first); the
# imported key's fingerprint must be RELEASE_GPG_FINGERPRINT, and every signature is verified
# before the script ends. The passphrase reaches gpg on a file descriptor, never argv.
#
# Environment: RELEASE_GPG_PRIVATE_KEY (armored secret key), RELEASE_GPG_PASSPHRASE (empty for
# a key without one), RELEASE_GPG_FINGERPRINT (40 hex digits).
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"
set +x

release=${1:?usage: sign-linux.sh <release dir> <out dir>}
out=${2:?usage: sign-linux.sh <release dir> <out dir>}
expected=$(printf '%s' "${RELEASE_GPG_FINGERPRINT:?}" | tr -d ' ' | tr '[:lower:]' '[:upper:]')
[[ "$expected" =~ ^[0-9A-F]{40}$ ]] || die "RELEASE_GPG_FINGERPRINT is not 40 hex digits."
[ -n "${RELEASE_GPG_PRIVATE_KEY:-}" ] || die "RELEASE_GPG_PRIVATE_KEY is empty."
[ -f "$release/SHA256SUMS" ] || die "$release has no SHA256SUMS."

GNUPGHOME=$(mktemp -d)
export GNUPGHOME
chmod 700 "$GNUPGHOME"
cleanup() {
  gpgconf --kill gpg-agent > /dev/null 2>&1 || true
  rm -rf "$GNUPGHOME"
}
trap cleanup EXIT

printf '%s\n' "$RELEASE_GPG_PRIVATE_KEY" | gpg --batch --quiet --import 2> "$GNUPGHOME/import.log" ||
  { grep -v -i 'secret' "$GNUPGHOME/import.log" >&2 || true; die "RELEASE_GPG_PRIVATE_KEY did not import."; }
secret=$(gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "sec" { primary = 1; next } $1 == "fpr" && primary { print $10; primary = 0 }')
[ "$(printf '%s\n' "$secret" | grep -c .)" = 1 ] || die "RELEASE_GPG_PRIVATE_KEY must hold exactly one secret key, not $(printf '%s\n' "$secret" | grep -c .)."
[ "$secret" = "$expected" ] || die "RELEASE_GPG_PRIVATE_KEY is key $secret, not RELEASE_GPG_FINGERPRINT $expected."

mkdir -p "$out"
# sign armored|binary <input> <output>
sign() {
  local input=$2 output=$3 format=()
  [ "$1" = binary ] || format=(--armor)
  gpg --batch --yes --quiet --pinentry-mode loopback --passphrase-fd 3 \
    --local-user "$expected" --digest-algo SHA512 --detach-sign "${format[@]}" \
    --output "$output" "$input" 3<<< "${RELEASE_GPG_PASSPHRASE:-}"
  gpg --batch --quiet --verify "$output" "$input" 2> /dev/null || die "The new signature $output does not verify."
  summary "- $(basename "$output"): GPG signature of $(basename "$input")"
}

shopt -s nullglob
count=0
for file in "$release"/Dromon-*.deb; do sign armored "$file" "$out/$(basename "$file").asc"; count=$((count + 1)); done
for file in "$release"/Dromon-*.pkg.tar.zst; do sign binary "$file" "$out/$(basename "$file").sig"; count=$((count + 1)); done
[ "$count" -gt 0 ] || die "$release has no Linux packages to sign."
sign armored "$release/SHA256SUMS" "$out/SHA256SUMS.asc"
gpg --batch --armor --export "$expected" > "$out/dromon-release-key.asc"
[ -s "$out/dromon-release-key.asc" ] || die "Could not export the public key."
echo "signed with $expected"
