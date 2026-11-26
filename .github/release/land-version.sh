#!/usr/bin/env bash
# land-version.sh: after a release, brings its version commit SHA (branch BRANCH) to main and
# deletes BRANCH. A fast-forward when main hasn't moved since the run started; else a merge
# commit (the version commit touches only Cargo.toml and Cargo.lock). Fails without changing
# main when the merge conflicts, saying what to do.
#
# Environment: SHA, BRANCH, VERSION, GITHUB_REPOSITORY, GH_TOKEN.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

repo=$GITHUB_REPOSITORY
if gh api -X PATCH "repos/$repo/git/refs/heads/main" -f sha="$SHA" -F force=false > /dev/null 2>&1; then
  summary "main fast-forwarded to the version commit ${SHA:0:7}."
elif merge=$(gh api "repos/$repo/merges" -f base=main -f head="$SHA" \
  -f commit_message="Merge Dromon $VERSION into main" --jq .sha 2>&1); then
  summary "main moved during the release: merged the version commit (${merge:0:7})."
else
  die "Could not bring Dromon $VERSION's version commit (${SHA:0:7}, branch $BRANCH) to main: $merge. Merge $BRANCH into main by hand, then delete the branch."
fi
gh api -X DELETE "repos/$repo/git/refs/heads/$BRANCH" > /dev/null || warn "Could not delete $BRANCH."
