# shellcheck shell=bash
# Shared helpers for the release scripts. Sourced, never executed.
#
# .github/release/ and .github/workflows/release.yml are the security boundary for releases
# (docs/releasing.md). Jobs that hold a secret or an OIDC token check out only this directory
# (plus data files they read, never run), run only these scripts and SHA-pinned actions, and
# treat artifacts as data whose SHA-256 the job that made them reported: they never build,
# install dependencies, or execute repository code or artifact contents.

set -euo pipefail

die() {
  echo "::error::$*" >&2
  exit 1
}

warn() {
  echo "::warning::$*" >&2
}

# A line for the job's summary page (and the log).
summary() {
  echo "$*"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then printf '%s\n' "$*" >> "$GITHUB_STEP_SUMMARY"; fi
}

output() {
  echo "$1=$2"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then printf '%s=%s\n' "$1" "$2" >> "$GITHUB_OUTPUT"; fi
}

sha256() {
  if command -v sha256sum > /dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

# Windows runners pass D:\a\_temp style paths; Git Bash tools want /d/a/_temp.
posix() {
  if command -v cygpath > /dev/null; then cygpath -u "$1"; else printf '%s' "$1"; fi
}

# The per-artifact transport manifest (write-checksums.sh). Not SHA256SUMS: that name is the
# release's own checksum file, which travels inside an artifact too.
# shellcheck disable=SC2034 # used by the scripts that source this file
MANIFEST=ARTIFACT-SHA256SUMS
