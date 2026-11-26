#!/usr/bin/env bash
# changelog.sh <previous tag> <commit> [<new tag>]: prints the release notes' "Changes" part (Markdown): what
# reached <commit> since <previous tag>, one line per pull request. Pull requests are squash
# merged, so each is one commit; the commit's pull request gives the line its title, number and
# author. A commit pushed without one is listed by its subject. The release workflow's version
# commits ("Dromon X.Y.Z", "Merge Dromon X.Y.Z into main") are left out.
#
# Sections by label: New (feature, enhancement), Fixes (bug, fix), Faster (performance), then
# Other changes. A pull request labelled skip-changelog is left out. Authors other than the
# repository's owner and bots are thanked at the end.
#
#   bash .github/release/changelog.sh 0.1.0 main     # a preview, with gh logged in
#
# Environment: GITHUB_REPOSITORY (owner/name; default: the gh repository here), GH_TOKEN or gh's
# login, GITHUB_SERVER_URL (default https://github.com).
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

previous=${1:?usage: changelog.sh <previous tag> <commit> [<new tag>]}
target=${2:?usage: changelog.sh <previous tag> <commit> [<new tag>]}
# The "All changes" link ends at the new tag when given (it exists once the release does).
until=${3:-$target}
repo=${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}
server=${GITHUB_SERVER_URL:-https://github.com}
owner=${repo%%/*}

# One line per commit, oldest first: sha, then the subject.
commits=$(gh api "repos/$repo/compare/$previous...$target" --paginate \
  --jq '.commits[] | .sha + " " + (.commit.message | split("\n")[0])')

declare -A sections=()
order=(New Fixes Faster "Other changes")
thanks=()
seen_prs=" "
while IFS= read -r line; do
  [ -n "$line" ] || continue
  sha=${line%% *}
  subject=${line#* }
  [[ $subject =~ ^(Merge\ )?Dromon\ [0-9]+\.[0-9]+\.[0-9]+ ]] && continue
  # The pull request this commit came from (squash merge: its only commit), if any.
  pr=$(gh api "repos/$repo/commits/$sha/pulls" \
    --jq '[.[] | select(.merged_at != null)][0] // empty | [(.number | tostring), .user.login, ([.labels[].name] | join(",")), .title] | join("\u001f")')
  if [ -n "$pr" ]; then
    # Unit separators, not tabs: read folds runs of whitespace, so no labels would shift the title.
    IFS=$'\x1f' read -r number author labels title <<< "$pr"
    case " $seen_prs " in *" $number "*) continue ;; esac
    seen_prs+="$number "
    case ",$labels," in *,skip-changelog,*) continue ;; esac
    entry="- $title ([#$number]($server/$repo/pull/$number)) @$author"
    if [ "$author" != "$owner" ] && [[ $author != *"[bot]" ]] && [[ " ${thanks[*]} " != *" @$author "* ]]; then
      thanks+=("@$author")
    fi
    section="Other changes"
    case ",$labels," in
      *,feature,* | *,enhancement,*) section=New ;;
      *,bug,* | *,fix,*) section=Fixes ;;
      *,performance,*) section=Faster ;;
    esac
  else
    entry="- $subject ([\`${sha:0:7}\`]($server/$repo/commit/$sha))"
    section="Other changes"
  fi
  sections[$section]+="$entry"$'\n'
done <<< "$commits"

echo "## Changes"
echo
any=false
for section in "${order[@]}"; do
  [ -n "${sections[$section]:-}" ] || continue
  any=true
  echo "### $section"
  echo
  printf '%s' "${sections[$section]}"
  echo
done
[ "$any" = true ] || { echo "No changes besides the version."; echo; }
if [ "${#thanks[@]}" -gt 0 ]; then
  echo "Thanks to ${thanks[*]} for their contributions."
  echo
fi
echo "All changes: $server/$repo/compare/$previous...$until"
