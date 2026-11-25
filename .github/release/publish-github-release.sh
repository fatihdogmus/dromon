#!/usr/bin/env bash
# publish-github-release.sh <notes file> <dir>...: creates the draft GitHub release for TAG (or
# updates it while it is still a draft) and uploads every file in each <dir>, byte for byte,
# checking each upload by downloading it again. Never touches a published release: the
# maintainer publishes the draft by hand.
#
# Environment: TAG, VERSION, PRERELEASE (true|false), SHA, GITHUB_REPOSITORY, GH_TOKEN.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

notes=${1:?usage: publish-github-release.sh <notes file> <dir>...}
shift
[ $# -gt 0 ] || die "usage: publish-github-release.sh <notes file> <dir>..."
[ "$TAG" = "v$VERSION" ] || die "The tag is $TAG, not v$VERSION."
repo=$GITHUB_REPOSITORY

# The tag must still name this run's commit (an annotated tag names a tag object first).
ref=$(gh api "repos/$repo/git/ref/tags/$TAG" --jq '.object.type + " " + .object.sha')
if [ "${ref%% *}" = tag ]; then ref=$(gh api "repos/$repo/git/tags/${ref#* }" --jq '.object.type + " " + .object.sha'); fi
[ "$ref" = "commit $SHA" ] || die "Tag $TAG names $ref, not commit $SHA."

title="Dromon $VERSION"
if state=$(gh release view "$TAG" -R "$repo" --json isDraft --jq .isDraft 2> /dev/null); then
  [ "$state" = true ] || die "Release $TAG is already published; this workflow only writes drafts."
  gh release edit "$TAG" -R "$repo" --title "$title" --notes-file "$notes" --prerelease="$PRERELEASE"
  echo "updating draft $TAG"
else
  gh release create "$TAG" -R "$repo" --draft --verify-tag --title "$title" --notes-file "$notes" --prerelease="$PRERELEASE"
  echo "created draft $TAG"
fi

files=()
for dir in "$@"; do
  for file in "$dir"/*; do
    [ -f "$file" ] || continue
    [ "$(basename "$file")" != "$MANIFEST" ] || continue
    files+=("$file")
  done
done
[ "${#files[@]}" -gt 0 ] || die "Nothing to upload."
gh release upload "$TAG" -R "$repo" --clobber "${files[@]}"

check=$(mktemp -d)
trap 'rm -rf "$check"' EXIT
for file in "${files[@]}"; do
  name=$(basename "$file")
  gh release download "$TAG" -R "$repo" --pattern "$name" --dir "$check" --clobber
  cmp -s "$file" "$check/$name" || die "GitHub's $name differs from the release's bytes."
  rm -f "$check/$name"
  summary "- uploaded $name"
done
url=$(gh release view "$TAG" -R "$repo" --json url --jq .url)
summary ""
summary "Draft release: $url. Review it, write what changed at the top of the notes, then publish it."
