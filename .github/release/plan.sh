#!/usr/bin/env bash
# plan.sh: decides what release.yml does, from the run's inputs and Cargo.toml.
#
#   release  BUMP=patch|minor|major from main: the next version (0.1.0 → 0.1.1 | 0.2.0 | 1.0.0),
#            committed (Cargo.toml and Cargo.lock, set-version.sh) on top of this run's commit and
#            pushed to the branch release/<version>. Every job builds that commit; publish tags
#            it <version> (plain semver, no "v"), publishes the release and moves main to it.
#   check    DRY_RUN=true, from any branch: the current version at this run's commit; builds,
#            signs with whatever credentials exist, verifies, publishes and pushes nothing.
#
# Outputs: mode, version, tag (release only), sha (the commit every job builds).
# Environment: BUMP, DRY_RUN, REF (github.ref), SHA (github.sha), GITHUB_REPOSITORY; the job's
# checkout (Cargo.toml, Cargo.lock, .github/release/) in the working directory, with push
# credentials for a release.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

current=$(awk '
  /^\[/ { section = $0 }
  section == "[workspace.package]" && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
' Cargo.toml)
[[ $current =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || die "[workspace.package].version '$current' is not X.Y.Z."
major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}

if [ "${DRY_RUN:?}" = true ]; then
  output mode check
  output version "$current"
  output tag ""
  output sha "${SHA:?}"
  summary "### Dry run, Dromon $current"
  summary "Builds, signs with the credentials that exist, verifies, and publishes nothing: the files stay as this run's artifacts."
  exit 0
fi

[ "${REF:?}" = refs/heads/main ] || die "Releases run from main, not $REF (a dry run works from any branch)."
case "${BUMP:?}" in
  patch) next="$major.$minor.$((patch + 1))" ;;
  minor) next="$major.$((minor + 1)).0" ;;
  major) next="$((major + 1)).0.0" ;;
  *) die "BUMP is patch, minor or major, not '$BUMP'." ;;
esac
tag=$next
if git ls-remote --exit-code --tags origin "refs/tags/$tag" > /dev/null; then
  die "Tag $tag exists already: Cargo.toml says $current, so a release was made without its version commit reaching main. Set main's version to $tag by hand, then run again."
fi

bash "$(dirname "$0")/set-version.sh" "$next" > /dev/null
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add Cargo.toml Cargo.lock
git commit -q -m "Dromon $next"
# release/<version>: the commit the jobs build. A failed earlier run may have left the branch.
git push -q --force origin "HEAD:refs/heads/release/$next"
sha=$(git rev-parse HEAD)

output mode release
output version "$next"
output tag "$tag"
output sha "$sha"
summary "### Release $next ($BUMP, from $current)"
summary "Builds commit \`${sha:0:7}\` (branch \`release/$next\`: the version bump on top of main), signs it once approved, then tags it \`$tag\`, publishes the release and moves main to it."
