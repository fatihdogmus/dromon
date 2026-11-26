#!/usr/bin/env bash
# Dromon with live editing (Rune's hot patching), compiled and run on this machine.
#
#   bash scripts/dev.sh                   # the real app: your ssh config, real connections
#   bash scripts/dev.sh --mock            # the built-in demo (DROMON_MOCK=1), no servers
#   bash scripts/dev.sh -- --fixture f.json --scene listing   # any app arguments after --
#
# Saved edits to function bodies in src/views/**, src/theme/** and src/settings_window.rs
# patch into the open window within a second or two, with the app's state kept (folder,
# selection, connections). Any other edit (state/, service/, types, signatures, constants,
# Cargo.toml) rebuilds and restarts the app; the first build is a full one. One exception: when
# an edit inside those patched files changes more than a function body (a signature, a const,
# a new item), the launcher prints "restart needed" and stops applying edits: Ctrl+C and rerun.
#
# Runs Rune's own launcher (scripts/rune-dev --local in the Rune checkout Cargo pinned) on
# dromon-app's `dromon-hot` binary. Needs python3; downloads Dioxus CLI 0.7.10 (`dx`, which does
# the patching) to target/tools on the first run, or uses RUNE_DX.
# Env: DROMON_DEV_PORT (18745) for the devserver, if that port is taken.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

app_args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mock) export DROMON_MOCK=1; shift ;;
    --) shift; app_args+=("$@"); break ;;
    -h|--help) sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "dev: unknown flag $1 (app arguments go after --)" >&2; exit 2 ;;
  esac
done

command -v python3 > /dev/null || { echo "dev: needs python3" >&2; exit 1; }

# Dioxus CLI 0.7.10 (Rune's launcher drives `dx serve --hot-patch`): RUNE_DX if set, else
# Dioxus's prebuilt release, checked against its pinned SHA-256, in target/tools.
host=$(rustc -vV | sed -n 's/^host: //p')
if [[ -z ${RUNE_DX:-} ]]; then
  export RUNE_DX="$root/target/tools/dx-0.7.10/dx"
  if ! [[ -x $RUNE_DX ]]; then
    case "$host" in
      aarch64-apple-darwin) sum=342c92bc33dca60dadd725ed5e249923f049ae2e1e50b7477f4968d56695b425 ;;
      x86_64-apple-darwin) sum=e3edbba285fa23a834aa7ceb558c9d8088cb3fbdcb48fb8c619532ed4d91cab7 ;;
      x86_64-unknown-linux-gnu) sum=4363e4ed2a3f1eb7f4d38d2d59aed59ce43271c44c16b425e92c89a64761fbe7 ;;
      aarch64-unknown-linux-gnu) sum=8f1a17d3218700ffbe15e6540d936a178b2556fc801121a31082e3ba4ab9ef55 ;;
      *) echo "dev: no prebuilt dx for $host; build it (cargo install dioxus-cli@0.7.10 --locked) and set RUNE_DX" >&2; exit 1 ;;
    esac
    dir=$(dirname "$RUNE_DX")
    mkdir -p "$dir"
    echo "dev: downloading Dioxus CLI 0.7.10 to $dir" >&2
    curl -fsSL -o "$dir/dx.tar.gz" "https://github.com/DioxusLabs/dioxus/releases/download/v0.7.10/dx-$host.tar.gz"
    if command -v sha256sum > /dev/null; then got=$(sha256sum "$dir/dx.tar.gz"); else got=$(shasum -a 256 "$dir/dx.tar.gz"); fi
    [[ ${got%% *} == "$sum" ]] || { rm -f "$dir/dx.tar.gz"; echo "dev: dx download doesn't match its SHA-256" >&2; exit 1; }
    tar -xzf "$dir/dx.tar.gz" -C "$dir" dx
    rm -f "$dir/dx.tar.gz"
  fi
fi
"$RUNE_DX" --version 2>/dev/null | grep -q '0\.7\.10' || { echo "dev: $RUNE_DX isn't Dioxus CLI 0.7.10" >&2; exit 1; }

# The pinned Rune's checkout: cargo fetches it on the first build, and it holds the launcher.
rune_manifest=$(cargo metadata --format-version 1 --locked --filter-platform "$host" |
  python3 -c 'import json, sys
d = json.load(sys.stdin)
print(next(p["manifest_path"] for p in d["packages"] if p["name"] == "rune" and (p.get("source") or "").startswith("git+")))')
launcher="$(dirname "$rune_manifest")/../../scripts/rune-dev"
[[ -f $launcher ]] || { echo "dev: no scripts/rune-dev next to $rune_manifest" >&2; exit 1; }

# `dromon-hot` is the app compiled as one crate (crates/dromon-app/src/hot.rs): hot patching
# reaches only the executable's own crate, and `dromon` links the views as a library.
exec python3 "$launcher" --local --manifest-path crates/dromon-app/Cargo.toml --bin dromon-hot \
  --patch 'src/views/**/*.rs' --patch 'src/theme/**/*.rs' --patch 'src/settings_window.rs' \
  --port "${DROMON_DEV_PORT:-18745}" -- ${app_args[@]+"${app_args[@]}"}
