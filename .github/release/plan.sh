#!/usr/bin/env bash
# plan.sh: decides what release.yml does, from the event and Cargo.toml alone.
#
#   release  a push of tag vX.Y.Z or vX.Y.Z-rc.N: builds, signs, verifies, and publishes a draft
#            GitHub release for the tag. The tag must equal v + [workspace.package].version (the
#            version is bumped in an ordinary commit before tagging, so --locked builds hold).
#   check    a workflow_dispatch with mode=check, from any branch or tag: builds, signs with
#            whatever credentials exist, verifies, and keeps everything as workflow artifacts.
#
# Outputs: mode, version, tag (release only), prerelease (true for -rc.N).
# Environment: EVENT, REF_TYPE, REF_NAME, INPUT_MODE, and Cargo.toml in the working directory
# (the job's sparse checkout).
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

version=$(awk '
  /^\[/ { section = $0 }
  section == "[workspace.package]" && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
' Cargo.toml)
semver='^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$'
[[ "$version" =~ $semver ]] || die "[workspace.package].version '$version' is not X.Y.Z or X.Y.Z-rc.N."

case "$EVENT" in
  push)
    [ "$REF_TYPE" = tag ] || die "Releases start from a tag push, not a push to $REF_TYPE $REF_NAME."
    [[ "$REF_NAME" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]] || die "Tag $REF_NAME is not vX.Y.Z or vX.Y.Z-rc.N."
    [ "$REF_NAME" = "v$version" ] ||
      die "Tag $REF_NAME does not match Cargo.toml's [workspace.package].version $version: bump the version in a commit, then tag that commit v$version."
    mode=release
    tag=$REF_NAME
    ;;
  workflow_dispatch)
    [ "$INPUT_MODE" = check ] || die "A dispatched run only checks (mode=check), not '$INPUT_MODE'."
    mode=check
    tag=""
    ;;
  *) die "release.yml does not run on $EVENT." ;;
esac

prerelease=false
[[ "$version" != *-rc.* ]] || prerelease=true
output mode "$mode"
output version "$version"
output tag "$tag"
output prerelease "$prerelease"
if [ "$mode" = release ]; then
  summary "### Release $tag"
  summary "Builds, signs, verifies and publishes Dromon $version as a draft GitHub release$([ "$prerelease" = false ] || echo ' (prerelease)')."
else
  summary "### Check run, Dromon $version"
  summary "Builds, signs with the credentials that exist, verifies, and publishes nothing: the files stay as this run's artifacts."
fi
