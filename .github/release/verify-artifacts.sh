#!/usr/bin/env bash
# verify-artifacts.sh <dir>: checks every artifact downloaded into <dir>/<name>. Its
# ARTIFACT-SHA256SUMS must have the digest the job that made it reported
# (DROMON_DIGESTS="name=digest ..."), every listed file must match, and it must hold nothing
# else. An artifact without a reported digest, or replaced after its job finished, is rejected.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

root=$(posix "$1")
# download-artifact unpacks a lone matching artifact straight into <dir>; put it back under its
# name, found by its manifest's reported digest.
if [ -f "$root/$MANIFEST" ]; then
  lone=$(sha256 "$root/$MANIFEST" | cut -c1-64)
  name=""
  for pair in ${DROMON_DIGESTS:-}; do
    if [ "${pair#*=}" = "$lone" ]; then name=${pair%%=*}; fi
  done
  [ -n "$name" ] || die "The downloaded artifact's $MANIFEST has no reported digest"
  staging=$(mktemp -d)
  find "$root" -mindepth 1 -maxdepth 1 -exec mv {} "$staging/" \;
  mv "$staging" "$root/$name"
fi
shopt -s nullglob
found=0
for dir in "$root"/*/; do
  name=$(basename "$dir")
  digest=""
  for pair in ${DROMON_DIGESTS:-}; do
    if [ "${pair%%=*}" = "$name" ]; then digest=${pair#*=}; fi
  done
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || die "No digest was reported for $name"
  (
    cd "$dir" || exit 1
    [ "$(sha256 "$MANIFEST" | cut -c1-64)" = "$digest" ] || die "$MANIFEST of $name does not have the reported digest"
    sha256 -c "$MANIFEST" > /dev/null || die "Checksum mismatch in $name"
    diff <(find . -type f ! -name "$MANIFEST" | sed 's|^\./||' | LC_ALL=C sort) <(cut -c67- "$MANIFEST" | LC_ALL=C sort) ||
      die "Unexpected files in $name"
  )
  echo "verified $name ($digest)"
  found=$((found + 1))
done
[ "$found" -gt 0 ] || die "No artifacts in $root"
