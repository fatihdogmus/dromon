#!/usr/bin/env bash
# write-checksums.sh <dir>: writes <dir>/ARTIFACT-SHA256SUMS for every file under <dir> and
# prints that manifest's digest. Every artifact a release job uploads carries this manifest, and
# the job that made it reports the digest as a job output; jobs that download the artifact check
# both (verify-artifacts.sh), so an artifact replaced after its job finished is rejected.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

cd "$(posix "$1")" || die "No folder $1"
manifest=$(mktemp)
find . -type f ! -name "$MANIFEST" | sed 's|^\./||' | LC_ALL=C sort | while IFS= read -r file; do sha256 "$file"; done > "$manifest"
[ -s "$manifest" ] || die "Nothing to upload in $1"
mv "$manifest" "$MANIFEST"
cat "$MANIFEST" >&2
sha256 "$MANIFEST" | cut -c1-64
