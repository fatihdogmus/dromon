#!/usr/bin/env bash
# Measures Dromon's performance gates (plan §9, §5 M5) and prints one table: gate, measured,
# target, result.
#
#   bash scripts/perf.sh                          # frames, first-rows, throughput
#   bash scripts/perf.sh --gate frames            # any of frames,first-rows,throughput,soak (or all)
#   bash scripts/perf.sh --gate soak --soak-gib 2
#   bash scripts/perf.sh --samples 9 --runs 5
#
# Gates and how they're measured (release builds; numbers in target/perf/*.log):
#
# - frames: `crates/dromon-app/examples/perf.rs` (with `--features perf`, Rune's frame-profile
#   phase clocks): the real app headless on a 100,000-entry folder, wheel-scrolled one event a
#   frame (24 pt and 280 pt a frame). The number is the UI thread's CPU time per frame, p99 over
#   240 frames, median over --samples runs. Headless frames build the frame (view, commit,
#   layout, text, paint) but don't encode, upload or present it, so the target is half of a
#   120 Hz frame (4.2 ms), leaving the other half to the GPU side. Keys, a click, typing in the
#   filter, the jump to the last row and streaming 100k rows in are listed as info (budget 8.3 ms
#   a frame, no exit code).
# - first-rows: the same example, from the first `ListingPage` of a 10,000-entry folder applied
#   to the end of the first frame with its rows mounted; and `crates/dromon-app/tests/perf_e2e.rs`
#   with the real service against the sshd fixture (Docker), from `Action::GoTo` (`Command::List`)
#   to that frame. Target 150 ms.
# - throughput: `crates/dromon-transfer/tests/perf.rs` against the sshd fixture on localhost: a
#   1 GiB file (DROMON_PERF_MIB) and 8,192 files of 128 KiB (DROMON_PERF_FILES) down and up with
#   the engine's I/O and with scp (-r), --runs times, the order alternating; the median of the
#   paired ratios. Target 80 % of scp. On localhost ssh's cipher is the limit for both, which is
#   not the gigabit LAN of the plan.
# - soak: the same file's `soak_large_file`: one --soak-gib file (default 10) down and up, SHA-1
#   checked both ways, the resident memory of the copying process and its ssh sampled every
#   200 ms. Needs that much free disk twice over (checked first). Not in the default set.
#
# Result: PASS; FAIL (missed the target); FAIL! (missed by the wide margin below, and the script
# exits 1); info; n/a (not measured: Docker not reachable, or the gate wasn't asked for).
# Wide margins: frames p99 over 8.3 ms (the whole 120 Hz frame); first rows over 300 ms (2x);
# throughput under 60 % of scp; soak memory growth over 256 MiB or a SHA-1 mismatch. A run that
# errors (a test panics) also exits 1. CI hardware varies: the table is informational there.
set -uo pipefail

# DROMON_PERF_ROOT measures another checkout of the workspace (a baseline) with this script;
# CARGO_TARGET_DIR is honoured.
root="${DROMON_PERF_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
root="$(cd "$root" && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"
gates="frames,first-rows,throughput"
samples=5
runs=3
soak_gib=10
while [[ $# -gt 0 ]]; do
  case "$1" in
    --gate) gates="$2"; shift 2 ;;
    --samples) samples="$2"; shift 2 ;;
    --runs) runs="$2"; shift 2 ;;
    --soak-gib) soak_gib="$2"; shift 2 ;;
    -h|--help) sed -n '2,/^set -uo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "usage: perf.sh [--gate frames,first-rows,throughput,soak|all] [--samples N] [--runs N] [--soak-gib N]" >&2; exit 2 ;;
  esac
done
[[ $gates == all ]] && gates="frames,first-rows,throughput,soak"
want() { [[ ",$gates," == *",$1,"* ]]; }
for g in ${gates//,/ }; do
  case "$g" in frames|first-rows|throughput|soak) ;; *) echo "perf.sh: unknown gate $g" >&2; exit 2 ;; esac
done

cd "$root"
out="$root/target/perf"
mkdir -p "$out"
metrics="$out/metrics.txt"
: >"$metrics"
errors=()

# Runs a command, keeps its output in target/perf/<name>.log and its METRIC lines.
run() {
  local name="$1"; shift
  echo "perf.sh: $name: $*" >&2
  if "$@" >"$out/$name.log" 2>&1; then
    :
  else
    errors+=("$name (exit $?, see target/perf/$name.log)")
  fi
  grep '^METRIC ' "$out/$name.log" >>"$metrics" || true
}

if want frames || want first-rows; then
  cargo build --release --quiet -p dromon-app --example perf --features perf || exit 1
  list=""
  want frames && list="scroll,stream,refresh"
  want first-rows && list="${list:+$list,}first-rows"
  run frames "$target/release/examples/perf" --samples "$samples" --gate "$list"
fi
if want first-rows; then
  DROMON_PERF_RUNS=$((runs > 5 ? runs : 5)) run first-rows-sshd \
    cargo test --release --quiet -p dromon-app --test perf_e2e -- --ignored --nocapture
fi
if want throughput; then
  DROMON_PERF_RUNS="$runs" run throughput \
    cargo test --release --quiet -p dromon-transfer --test perf -- --ignored --nocapture --test-threads 1 perf_
fi
if want soak; then
  DROMON_SOAK_GIB="$soak_gib" run soak \
    cargo test --release --quiet -p dromon-transfer --test perf -- --ignored --nocapture --exact soak_large_file
fi

# The value of `gate`'s `metric`, or empty.
get() { awk -v g="gate=$1" -v m="metric=$2" '$2 == g && $3 == m { sub("value=", "", $4); print $4; exit }' "$metrics"; }

hard=0
rows=()
# row <asked gate> <label> <value> <unit> <target text> <kind> <target> [<hard limit>]
#   kind: max (value <= target), min (value >= target), info-max (shown against target only),
#   info (shown only).
row() {
  local asked="$1" label="$2" value="$3" unit="$4" target_text="$5" kind="$6" target="$7" limit="${8:-}"
  local result shown
  if [[ -z $value ]]; then
    want "$asked" && result="n/a" || return 0
    rows+=("$label|n/a|$target_text|$result")
    return 0
  fi
  shown="$(printf '%.1f %s' "$value" "$unit")"
  case "$kind" in
    max|info-max) ok=$(awk -v v="$value" -v t="$target" 'BEGIN { print (v <= t) }') ;;
    min) ok=$(awk -v v="$value" -v t="$target" 'BEGIN { print (v >= t) }') ;;
    info) ok=1 ;;
  esac
  if [[ $kind == info ]]; then
    result="info"
  elif [[ $kind == info-max ]]; then
    [[ $ok == 1 ]] && result="info (within)" || result="info (over)"
  elif [[ $ok == 1 ]]; then
    result="PASS"
  else
    result="FAIL"
    if [[ -n $limit ]]; then
      case "$kind" in
        max) wide=$(awk -v v="$value" -v l="$limit" 'BEGIN { print (v > l) }') ;;
        min) wide=$(awk -v v="$value" -v l="$limit" 'BEGIN { print (v < l) }') ;;
      esac
      if [[ $wide == 1 ]]; then result="FAIL!"; hard=1; fi
    fi
  fi
  rows+=("$label|$shown|$target_text|$result")
}

row frames "Scroll 100k rows, 24 pt/frame: CPU/frame p99" "$(get scroll cpu_p99_ms)" ms "<= 4.2 ms" max 4.17 8.33
row frames "Scroll 100k rows, 280 pt/frame: CPU/frame p99" "$(get fling cpu_p99_ms)" ms "<= 4.2 ms" max 4.17 8.33
row frames "Scroll 100k rows, 24 pt/frame: CPU/frame max" "$(get scroll cpu_max_ms)" ms "<= 8.3 ms" info-max 8.33
row frames "Scroll 100k rows, 280 pt/frame: CPU/frame max" "$(get fling cpu_max_ms)" ms "<= 8.3 ms" info-max 8.33
row frames "Scroll 100k rows, 280 pt/frame: allocations/frame" "$(get fling allocs_per_frame)" "" "-" info 0
row frames "100k: Down key frame" "$(get key-down cpu_ms)" ms "<= 8.3 ms" info-max 8.33
row frames "100k: End key frame" "$(get key-end cpu_ms)" ms "<= 8.3 ms" info-max 8.33
row frames "100k: click a row frame" "$(get click cpu_ms)" ms "<= 8.3 ms" info-max 8.33
row frames "100k: filter keystroke frame" "$(get filter-type cpu_ms)" ms "<= 8.3 ms" info-max 8.33
row frames "100k: jump to last row, worst frame" "$(get jump-end cpu_max_ms)" ms "<= 8.3 ms" info-max 8.33
row frames "100k streaming in: worst page (apply + frame)" "$(get stream-100k worst_page_ms)" ms "<= 8.3 ms" info-max 8.33
row frames "100k streaming in: all 196 pages" "$(get stream-100k total_ms)" ms "-" info 0
row frames "100k refresh, one file added: frame" "$(get refresh-100k cpu_ms)" ms "<= 8.3 ms" info-max 8.33
row first-rows "First rows of 10k, headless (first page -> frame)" "$(get first-rows-10k wall_ms)" ms "<= 150 ms" max 150 300
row first-rows "First rows of 10k, sshd (List -> first page)" "$(get first-rows-10k-sshd first_page_ms)" ms "<= 150 ms" info-max 150
row first-rows "First rows of 10k, sshd (List -> rows on screen)" "$(get first-rows-10k-sshd first_rows_ms)" ms "<= 150 ms" max 150 300
row first-rows "Whole 10k folder, sshd" "$(get first-rows-10k-sshd whole_ms)" ms "-" info 0
row throughput "1 GiB file download vs scp" "$(get large-down ratio_pct)" % ">= 80 %" min 80 60
row throughput "1 GiB file upload vs scp" "$(get large-up ratio_pct)" % ">= 80 %" min 80 60
row throughput "8,192 x 128 KiB download vs scp -r" "$(get many-down ratio_pct)" % ">= 80 %" min 80 60
row throughput "8,192 x 128 KiB upload vs scp -r" "$(get many-up ratio_pct)" % ">= 80 %" min 80 60
row throughput "1 GiB file download, dromon" "$(get large-down dromon_mbps)" MB/s "-" info 0
row throughput "1 GiB file upload, dromon" "$(get large-up dromon_mbps)" MB/s "-" info 0
row soak "Soak: SHA-1 both ways ($(get soak size_gib | cut -d. -f1) GiB)" "$(get soak sha1_ok)" "" "= 1" min 1 1
row soak "Soak download: RSS growth" "$(get soak-down rss_growth_mib)" MiB "<= 64 MiB" max 64 256
row soak "Soak upload: RSS growth" "$(get soak-up rss_growth_mib)" MiB "<= 64 MiB" max 64 256
row soak "Soak download: ssh RSS growth" "$(get soak-down ssh_rss_growth_mib)" MiB "<= 64 MiB" max 64 256
row soak "Soak upload: ssh RSS growth" "$(get soak-up ssh_rss_growth_mib)" MiB "<= 64 MiB" max 64 256
row soak "Soak download: RSS drift (last - first quarter)" "$(get soak-down rss_drift_mib)" MiB "-" info 0
row soak "Soak upload: RSS drift (last - first quarter)" "$(get soak-up rss_drift_mib)" MiB "-" info 0
row soak "Soak download, dromon" "$(get soak-down dromon_mbps)" MB/s "-" info 0
row soak "Soak upload, dromon" "$(get soak-up dromon_mbps)" MB/s "-" info 0

{
  echo "Gate|Measured|Target|Result"
  echo "---|---|---|---"
  printf '%s\n' "${rows[@]}"
} | column -t -s '|'

if [[ ${#errors[@]} -gt 0 ]]; then
  printf 'perf.sh: error: %s\n' "${errors[@]}" >&2
  exit 1
fi
if [[ $hard == 1 ]]; then
  echo "perf.sh: a gate failed by a wide margin (FAIL!)" >&2
  exit 1
fi
