#!/usr/bin/env bash
# publish-github-release.sh <notes file> <dir>...: publishes the GitHub release TAG at commit SHA
# with every file in each <dir>. It writes a draft first, uploads the files byte for byte and
# checks each by downloading it again, then publishes the draft, which creates the tag at SHA.
# The notes start with the commits since the previous release. Never touches a published
# release.
#
# Environment: TAG, VERSION, SHA, GITHUB_REPOSITORY, GH_TOKEN.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

notes=${1:?usage: publish-github-release.sh <notes file> <dir>...}
shift
[ $# -gt 0 ] || die "usage: publish-github-release.sh <notes file> <dir>..."
[ "$TAG" = "$VERSION" ] || die "The tag is $TAG, not $VERSION."
repo=$GITHUB_REPOSITORY

if gh api "repos/$repo/git/ref/tags/$TAG" > /dev/null 2>&1; then
  die "Tag $TAG exists already."
fi

# What changed: the commits since the previous release (its tag), oldest first, without the
# version commits themselves.
previous=$(gh release view -R "$repo" --json tagName --jq .tagName 2> /dev/null || true)
body=$(mktemp)
trap 'rm -f "$body"' EXIT
{
  echo "## Changes"
  echo
  if [ -n "$previous" ]; then
    gh api "repos/$repo/compare/$previous...$SHA" --paginate \
      --jq '.commits[] | "- " + (.commit.message | split("\n")[0]) + " (" + .sha[0:7] + ")"' |
      grep -vE '^- Dromon [0-9]+\.[0-9]+\.[0-9]+ \(' || echo "- No changes besides the version."
    echo
    echo "All changes: $GITHUB_SERVER_URL/$repo/compare/$previous...$TAG"
  else
    echo "The first release."
  fi
  echo
  cat "$notes"
} > "$body"

title="Dromon $VERSION"
if state=$(gh release view "$TAG" -R "$repo" --json isDraft --jq .isDraft 2> /dev/null); then
  [ "$state" = true ] || die "Release $TAG is already published."
  gh release edit "$TAG" -R "$repo" --title "$title" --notes-file "$body" --target "$SHA"
  echo "updating draft $TAG"
else
  gh release create "$TAG" -R "$repo" --draft --target "$SHA" --title "$title" --notes-file "$body"
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
trap 'rm -rf "$check" "$body"' EXIT
for file in "${files[@]}"; do
  name=$(basename "$file")
  gh release download "$TAG" -R "$repo" --pattern "$name" --dir "$check" --clobber
  cmp -s "$file" "$check/$name" || die "GitHub's $name differs from the release's bytes."
  rm -f "$check/$name"
  summary "- uploaded $name"
done

# Publishing creates the tag at SHA.
gh release edit "$TAG" -R "$repo" --draft=false --latest
ref=$(gh api "repos/$repo/git/ref/tags/$TAG" --jq '.object.type + " " + .object.sha')
if [ "${ref%% *}" = tag ]; then ref=$(gh api "repos/$repo/git/tags/${ref#* }" --jq '.object.type + " " + .object.sha'); fi
[ "$ref" = "commit $SHA" ] || die "Tag $TAG names $ref, not commit $SHA."
url=$(gh release view "$TAG" -R "$repo" --json url --jq .url)
summary ""
summary "Published: $url"
