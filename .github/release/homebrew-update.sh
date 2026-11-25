#!/usr/bin/env bash
# homebrew-update.sh <out-file>: writes the Homebrew cask for the published release TAG of REPO
# to <out-file>, for homebrew.yml to push to the tap. Runs on macOS (hdiutil, spctl).
#
# Writes nothing, and sets the output `skip=true`, when TAG is not the repository's latest
# release (an older release republished, or a prerelease): the tap follows the latest only.
# Otherwise it downloads the release's DMG and SHA256SUMS and checks that
#   - the DMG's SHA-256 is the one SHA256SUMS lists,
#   - the DMG has a Sigstore build-provenance attestation from release.yml for this tag,
# then asks Gatekeeper whether Dromon.app is notarized, which decides whether the cask strips
# the quarantine flag (homebrew-cask.sh).
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

out=$1
: "${TAG:?}" "${REPO:?}" "${GH_TOKEN:?}"
[[ $TAG =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || die "$TAG is not a release tag (vX.Y.Z); prereleases get no cask."
version=${BASH_REMATCH[1]}

info=$(gh release view "$TAG" --repo "$REPO" --json isDraft,isPrerelease --jq '"\(.isDraft) \(.isPrerelease)"') \
  || die "No release $TAG in $REPO."
[ "$info" = "false false" ] || die "$TAG is a draft or a prerelease (draft, prerelease: $info)."
latest=$(gh release view --repo "$REPO" --json tagName --jq .tagName)
if [ "$latest" != "$TAG" ]; then
  summary "$TAG is not the latest release ($latest is); the tap keeps its cask."
  output skip true
  exit 0
fi

work=$(mktemp -d)
dmg="Dromon-$version-macos-arm64.dmg"
gh release download "$TAG" --repo "$REPO" --dir "$work" --pattern "$dmg" --pattern SHA256SUMS

sha=$(sha256 "$work/$dmg" | cut -c1-64)
listed=$(awk -v f="$dmg" '$2 == f || $2 == "*" f { print $1 }' "$work/SHA256SUMS")
[ -n "$listed" ] || die "SHA256SUMS of $TAG has no $dmg."
[ "$sha" = "$listed" ] || die "$dmg is $sha, but SHA256SUMS lists $listed."

gh attestation verify "$work/$dmg" --repo "$REPO" \
  --signer-workflow "$REPO/.github/workflows/release.yml" --source-ref "refs/tags/$TAG" > /dev/null \
  || die "$dmg has no build attestation from release.yml at $TAG."

mnt="$work/mnt"
hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$work/$dmg" > /dev/null
if spctl --assess --type execute "$mnt/Dromon.app" 2> /dev/null; then notarized=true; else notarized=false; fi
hdiutil detach "$mnt" > /dev/null

bash "$(dirname "$0")/homebrew-cask.sh" "$version" "$sha" "$notarized" "$REPO" > "$out"
rm -rf "$work"
summary "Cask for Dromon $version: sha256 $sha, notarized: $notarized (quarantine stripped: $([ "$notarized" = true ] && echo no || echo yes))."
output skip false
output version "$version"
