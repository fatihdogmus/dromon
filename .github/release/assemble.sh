#!/usr/bin/env bash
# assemble.sh <out dir> <version> <dir>...: gathers the release's files from the verified
# artifact folders into <out dir> and writes the release-wide SHA256SUMS. Exactly these files,
# each found once, or the run fails:
#
#   Dromon-<version>-windows-x64.msi
#   Dromon-<version>-macos-universal.dmg
#   Dromon-<version>-linux-x64.deb
#   Dromon-<version>-linux-x64.pkg.tar.zst
#
# Release targets: Windows x64, macOS (one universal app: Apple silicon and Intel), Linux
# x86_64. More architectures need their
# builds in release.yml and their names here (and in release-notes.sh).
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

out=${1:?usage: assemble.sh <out dir> <version> <dir>...}
version=${2:?usage: assemble.sh <out dir> <version> <dir>...}
shift 2
[ $# -gt 0 ] || die "assemble.sh needs the artifact folders to gather from."

expected=(
  "Dromon-$version-windows-x64.msi"
  "Dromon-$version-macos-universal.dmg"
  "Dromon-$version-linux-x64.deb"
  "Dromon-$version-linux-x64.pkg.tar.zst"
)

mkdir -p "$out"
for name in "${expected[@]}"; do
  matches=()
  while IFS= read -r -d '' file; do matches+=("$file"); done < <(find "$@" -type f -name "$name" -print0)
  [ "${#matches[@]}" = 1 ] || die "Expected one $name in the artifacts, found ${#matches[@]}."
  cp "${matches[0]}" "$out/$name"
done
(cd "$out" && for name in "${expected[@]}"; do sha256 "$name"; done) > "$out/SHA256SUMS"
(cd "$out" && sha256 -c SHA256SUMS > /dev/null) || die "SHA256SUMS does not check."
summary "### Release files (Dromon $version)"
summary '```'
summary "$(cat "$out/SHA256SUMS")"
summary '```'
