#!/usr/bin/env bash
# Turns a bug report into headless output to look at: the real app on the issue's fixture,
# driven by the issue's script, rendered without a window or network.
#
#   bash scripts/repro.sh 123                    # issue #123 of this repo (gh issue view)
#   bash scripts/repro.sh 123 --repo owner/name  # of another repo
#   bash scripts/repro.sh path/to/issue.md       # an issue body saved as a file
#   bash scripts/repro.sh 123 /tmp/out           # out dir (default target/repro/<number or file name>)
#
# The issue (see .github/ISSUE_TEMPLATE/repro.yml and docs/examples/issue-repro.md) carries fenced
# blocks whose info string is exactly:
#   ```json fixture   optional: the fixture (docs/fixtures.md); without it, the built-in demo
#   ```json script    the Rune script to replay (README "Agent tooling")
#   ```text args      optional: extra flags, only these: --scene NAME, --size WxH, --scale N,
#                     --theme dark|light, --at T|mark:NAME[±S], --frames N, --every N
# At most one of each. Nothing from the issue is ever run as shell: the blocks are written to
# files and handed to dromon as arguments, and args outside the list are refused.
#
# Writes to the out dir: issue.md (the body), fixture.json, script.json, args.txt (as given),
# end.png (`--snap` at the script's end, or at --at), strip.png (`--strip`: a frame every
# --every frames, 30 by default), tree.json (`--tree` at the same moment as end.png), run.mp4
# (`--video`, only when ffmpeg is on PATH), run.log, and summary.md: the issue's title and URL,
# Dromon's revision, the exact commands and their exit codes, the texts visible at the end
# (from tree.json; needs jq or python3) and the outputs' paths.
#
# Exit status: 0 when Dromon ran (whether or not the bug shows: look at the outputs);
# 1 when Dromon failed to run (the build, an invalid fixture or script, a crash; summary.md
# says which); 2 on bad arguments, an unreadable issue or refused input (args off the list, a
# block given twice); 3 when the issue has no ```json script block (nothing to run).
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  sed -n '2,/^set -uo/p' "$0" | sed '$d; s/^# \{0,1\}//'
}
die() {
  echo "repro: $*" >&2
  exit 2
}

repo=""
pos=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) [[ $# -ge 2 ]] || die "--repo needs owner/name"; repo="$2"; shift 2 ;;
    --repo=*) repo="${1#--repo=}"; shift ;;
    -h | --help) usage; exit 0 ;;
    -*) die "unknown option $1 (see --help)" ;;
    *) pos+=("$1"); shift ;;
  esac
done
[[ ${#pos[@]} -ge 1 && ${#pos[@]} -le 2 ]] || { usage >&2; exit 2; }
source="${pos[0]}"

# --- The issue ---------------------------------------------------------------------------------
title=""
url=""
body=""
if [[ "$source" =~ ^[0-9]+$ ]]; then
  command -v gh >/dev/null || die "gh (GitHub CLI) is needed to fetch issue #$source"
  # shellcheck disable=SC2054 # one --json value: a comma-separated field list
  gh_args=(issue view "$source" --json title,body,url)
  [[ -n "$repo" ]] && gh_args+=(--repo "$repo")
  title="$(gh "${gh_args[@]}" --jq .title)" || die "couldn't fetch issue #$source${repo:+ of $repo}"
  url="$(gh "${gh_args[@]}" --jq .url)" || die "couldn't fetch issue #$source"
  body="$(gh "${gh_args[@]}" --jq .body)" || die "couldn't fetch issue #$source"
  name="$source"
elif [[ -f "$source" ]]; then
  [[ -z "$repo" ]] || die "--repo goes with an issue number, not a file"
  body="$(cat "$source")" || die "couldn't read $source"
  url="$source"
  name="$(basename "$source")"
  name="${name%.*}"
else
  die "$source is neither an issue number nor a file"
fi

out="${pos[1]:-$root/target/repro/$name}"
mkdir -p "$out" || die "couldn't create $out"
out="$(cd "$out" && pwd)"
rm -f "$out"/{issue.md,fixture.json,script.json,args.txt,end.png,strip.png,tree.json,run.mp4,run.log,summary.md}
printf '%s\n' "$body" >"$out/issue.md"

# A file has no title field: take the Summary section's first line (issue forms put each field
# under `### <label>`), else the file name.
if [[ -z "$title" ]]; then
  title="$(awk '{ sub(/\r$/, "") } /^### / { s = ($0 ~ /^### Summary/) ; next } s && NF && $0 != "_No response_" { print; exit }' "$out/issue.md")"
  [[ -n "$title" ]] || title="$name"
fi

# --- The blocks: CommonMark backtick fences, written to files, counted ------------------------
counts="$(awk -v dir="$out" '
  function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
  { sub(/\r$/, "") }
  {
    line = $0
    sub(/^ ? ? ?/, "", line)          # up to three spaces of indentation
    if (open == "") {
      if (line !~ /^```/) next
      fence = line; sub(/[^`].*$/, "", fence)
      info = trim(substr(line, length(fence) + 1))
      open = fence
      kind = ""
      if (info == "json fixture") kind = "fixture.json"
      else if (info == "json script") kind = "script.json"
      else if (info == "text args") kind = "args.txt"
      if (kind != "") { seen[kind]++; if (seen[kind] == 1) { file = dir "/" kind; printf "" > file } else file = "" }
      else file = ""
      next
    }
    # Inside a fence: a line of at least as many backticks (and nothing else) closes it.
    t = trim(line)
    if (t ~ /^```+$/ && length(t) >= length(open)) { if (file != "") close(file); open = ""; file = ""; next }
    if (file != "") print $0 > file
  }
  END { printf "fixture %d\nscript %d\nargs %d\n", seen["fixture.json"], seen["script.json"], seen["args.txt"] }
' "$out/issue.md")"
count() { echo "$counts" | sed -n "s/^$1 //p"; }

summary() { printf '%s\n' "$@" >>"$out/summary.md"; }
revision="$(git -C "$root" rev-parse --short HEAD 2>/dev/null || echo unknown)"
if [[ "$revision" != unknown && -n "$(git -C "$root" status --porcelain --untracked-files=no 2>/dev/null)" ]]; then
  revision="$revision (with local changes)"
fi
summary "# Repro: $title" "" "- Issue: $url" "- Dromon: \`$revision\`" "- Run: $(date -u +%Y-%m-%dT%H:%M:%SZ)"

# refuse CODE MESSAGE: says why nothing ran, in summary.md too.
refuse() {
  summary "- Result: not run: $2" ""
  echo "repro: $2" >&2
  echo "repro: summary in ${out#"$root"/}/summary.md" >&2
  exit "$1"
}

for k in fixture script args; do
  [[ "$(count $k)" -le 1 ]] || refuse 2 "the issue has $(count $k) \`\`\`$( [[ $k == args ]] && echo text || echo json ) $k blocks; give at most one"
done
[[ "$(count script)" -eq 1 && -s "$out/script.json" ]] \
  || refuse 3 "the issue has no \`\`\`json script block (or it's empty), so there is nothing to replay. See .github/ISSUE_TEMPLATE/repro.yml for the format."

fixture="$out/fixture.json"
fixture_note="fixture.json (from the issue)"
if [[ "$(count fixture)" -eq 0 ]] || ! grep -q '[^[:space:]]' "$fixture"; then
  rm -f "$fixture"
  fixture="$root/crates/dromon-app/assets/fixtures/demo.json"
  fixture_note="the built-in demo (no \`\`\`json fixture block)"
fi

# --- Args: an allowlist of flags with checked values ---------------------------------------------
extra=()
if [[ -f "$out/args.txt" ]]; then
  tokens=()
  set -f # split on whitespace only: no globbing of the issue's text
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%%#*}"
    line="${line//$'\r'/}"
    for tok in $line; do tokens+=("$tok"); done
  done <"$out/args.txt"
  set +f
  seen_flags=" "
  i=0
  while [[ $i -lt ${#tokens[@]} ]]; do
    flag="${tokens[$i]}"
    value="${tokens[$((i + 1))]:-}"
    case "$flag" in
      --scene) re='^[a-z0-9][a-z0-9-]*$' ;;
      --size) re='^[0-9]{2,5}x[0-9]{2,5}$' ;;
      --scale) re='^[1-4](\.[0-9]+)?$' ;;
      --theme) re='^(dark|light)$' ;;
      --at) re='^([0-9]+(\.[0-9]+)?|mark:[A-Za-z0-9_.-]+([+-][0-9]+(\.[0-9]+)?)?)$' ;;
      --frames) re='^[1-9][0-9]{0,4}$' ;;
      --every) re='^[1-9][0-9]{0,3}$' ;;
      *) refuse 2 "args: $flag isn't allowed (allowed: --scene NAME, --size WxH, --scale N, --theme dark|light, --at T|mark:NAME, --frames N, --every N)" ;;
    esac
    [[ -n "$value" && "$value" =~ $re ]] || refuse 2 "args: $flag needs a value like the allowlist says, not \"$value\""
    [[ "$seen_flags" != *" $flag "* ]] || refuse 2 "args: $flag is given twice"
    seen_flags+="$flag "
    extra+=("$flag" "$value")
    i=$((i + 2))
  done
fi

# --- Run ---------------------------------------------------------------------------------------
cd "$root" || exit 1
build=(cargo build -p dromon-app --quiet)
"${build[@]}" >"$out/build.log" 2>&1
build_status=$?
bin="$root/target/debug/dromon"
[[ -x "$bin.exe" ]] && bin="$bin.exe"
video=()
command -v ffmpeg >/dev/null 2>&1 && video=(--video "$out/run.mp4")
# Defaults first: the issue's args come later and win.
run=("$bin" --fixture "$fixture" --script "$out/script.json" --duration auto --size 1080x680 --scale 1 --theme dark
  --every 30 --cols 4 ${extra[@]+"${extra[@]}"} --snap "$out/end.png" --strip "$out/strip.png" --tree "$out/tree.json"
  ${video[@]+"${video[@]}"})
run_status="not run"
if [[ $build_status -eq 0 ]]; then
  DROMON_TRACE=1 "${run[@]}" >"$out/run.log" 2>&1
  run_status=$?
fi

quote() {
  local a s=""
  for a in "$@"; do s+="$(printf '%q' "${a#"$root"/}") "; done
  echo "${s% }"
}
summary "- Inputs: $fixture_note; script.json$( [[ ${#extra[@]} -gt 0 ]] && echo "; args \`${extra[*]}\`")"
if [[ "$build_status" -ne 0 ]]; then
  summary "- Result: **Dromon failed to build** (exit $build_status)"
elif [[ "$run_status" -ne 0 ]]; then
  summary "- Result: **Dromon failed to run** (exit $run_status); see the log below"
else
  summary "- Result: Dromon ran (exit 0). Whether the bug shows is for you to judge from the outputs."
fi
summary "" "## Commands" "" '```sh' "$(quote "${build[@]}")    # exit $build_status"
[[ $build_status -eq 0 ]] && summary "DROMON_TRACE=1 $(quote "${run[@]}")    # exit $run_status"
summary '```' ""

if [[ "$run_status" == 0 && -s "$out/tree.json" ]]; then
  # Elements with text, on screen at the end (secure fields have none), in tree order.
  texts=""
  if command -v jq >/dev/null; then
    texts="$(jq -r '.elements[] | select((.text // "") != "" and (.opacity // 1) > 0.05 and (.exiting // false) == false)
      | (if .key then "`\(.key)`: " else "" end) + (.text | gsub("\n"; " ⏎ "))' "$out/tree.json")"
  elif command -v python3 >/dev/null; then
    texts="$(python3 -c '
import json, sys
for e in json.load(open(sys.argv[1]))["elements"]:
    t = e.get("text") or ""
    if t and (e.get("opacity") or 1) > 0.05 and not e.get("exiting"):
        print(("`%s`: " % e["key"] if e.get("key") else "") + t.replace("\n", " ⏎ "))
' "$out/tree.json")"
  fi
  at="$(sed -n 's/^  "t": \([0-9.]*\),*$/\1/p' "$out/tree.json" | head -n1)"
  summary "## Visible at the end${at:+ (t = $at s)}" ""
  if [[ -n "$texts" ]]; then
    total="$(echo "$texts" | wc -l | tr -d ' ')"
    echo "$texts" | head -n 60 | sed 's/^/- /' >>"$out/summary.md"
    [[ "$total" -le 60 ]] || summary "- … $((total - 60)) more in tree.json"
  else
    summary "(install jq or python3 to list them; the tree is in tree.json)"
  fi
  summary ""
fi

summary "## Outputs" ""
for f in issue.md fixture.json script.json args.txt end.png strip.png tree.json run.mp4 run.log build.log; do
  [[ -s "$out/$f" ]] && summary "- \`${out#"$root"/}/$f\`"
done
if [[ "$build_status" -ne 0 || "$run_status" != 0 ]]; then
  log="$out/run.log"
  [[ "$build_status" -ne 0 ]] && log="$out/build.log"
  summary "" "## Log (last 20 lines)" "" '```text'
  grep -v '^rune: ' "$log" | tail -n 20 >>"$out/summary.md"
  summary '```'
fi

echo "repro: ${out#"$root"/}/summary.md"
if [[ "$build_status" -ne 0 || "$run_status" != 0 ]]; then
  echo "repro: Dromon failed to run (build exit $build_status, run exit $run_status)" >&2
  exit 1
fi
echo "repro: ran (exit 0): ${out#"$root"/}/end.png, strip.png, tree.json"
