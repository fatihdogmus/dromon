#!/usr/bin/env bash
# sign-minisign.sh <release dir> <out dir> <version> <public key>: signs the release's SHA256SUMS
# with the minisign release key, the key Dromon's updater trusts, and writes only the signature:
#
#   SHA256SUMS.minisig   pre-hashed minisign signature of SHA256SUMS, trusted comment
#                        "dromon <version>" (the updater refuses another release's signature)
#
# The secret key goes to a 0600 file in a 0700 folder under the job's temp folder, deleted on
# exit; it never reaches argv or the log. The new signature must verify against <public key>
# (packaging/update/release-key.pub, read as data: the key the app builds in), so a release the
# app would refuse never ships, and must be the pre-hashed kind (the app refuses the legacy one).
#
# Environment: MINISIGN_SECRET_KEY (an unencrypted minisign secret key, `minisign -G -W`).
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"
set +x

usage="usage: sign-minisign.sh <release dir> <out dir> <version> <public key>"
release=${1:?$usage}
out=${2:?$usage}
version=${3:?$usage}
public=${4:?$usage}
[ -n "${MINISIGN_SECRET_KEY:-}" ] || die "MINISIGN_SECRET_KEY is empty."
[ -f "$release/SHA256SUMS" ] || die "$release has no SHA256SUMS."
[ -f "$public" ] || die "No public key at $public."
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "'$version' is not a release version."
command -v minisign > /dev/null || die "minisign is not installed."

work=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/minisign.XXXXXX")
chmod 700 "$work"
trap 'rm -rf "$work"' EXIT
key="$work/release.key"
(umask 077 && printf '%s\n' "$MINISIGN_SECRET_KEY" > "$key")

mkdir -p "$out"
signature="$out/SHA256SUMS.minisig"
# stdin from /dev/null: a key that wants a password fails here instead of waiting for one.
minisign -S -s "$key" -m "$release/SHA256SUMS" -x "$signature" -t "dromon $version" < /dev/null > /dev/null ||
  die "minisign could not sign SHA256SUMS (is MINISIGN_SECRET_KEY an unencrypted minisign secret key?)."
rm -f "$key"

minisign -V -q -p "$public" -m "$release/SHA256SUMS" -x "$signature" ||
  die "The new signature does not verify with $public: MINISIGN_SECRET_KEY is not the key the app trusts."
# Line 2 is base64("ED" + key id + signature): "RU…" for pre-hashed, "RW…" for legacy ("Ed").
sed -n 2p "$signature" | grep -q '^RU' || die "The new signature is not pre-hashed; the app refuses legacy signatures."
grep -qxF "trusted comment: dromon $version" "$signature" || die "The new signature's trusted comment is not 'dromon $version'."
summary "- SHA256SUMS.minisig: minisign signature of SHA256SUMS ($(head -n 1 "$public" | sed 's/^untrusted comment: //'))"
