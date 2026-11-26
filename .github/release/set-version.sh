#!/usr/bin/env bash
# set-version.sh <X.Y.Z> [<root>]: sets Dromon's version to X.Y.Z in <root> (default .):
# [workspace.package].version in Cargo.toml, and in Cargo.lock every workspace package's version
# (packages without a `source`: every crate here inherits the workspace version). The same edit
# `cargo` makes, without running cargo, so `--locked` builds of the result pass. Prints the old
# version.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

new=${1:?usage: set-version.sh <X.Y.Z> [<root>]}
root=${2:-.}
[[ $new =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$new is not X.Y.Z."
old=$(awk '
  /^\[/ { section = $0 }
  section == "[workspace.package]" && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
' "$root/Cargo.toml")
[ -n "$old" ] || die "No [workspace.package] version in $root/Cargo.toml."

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
awk -v new="$new" '
  /^\[/ { section = $0 }
  section == "[workspace.package]" && $1 == "version" && !done { sub(/"[^"]*"/, "\"" new "\""); done = 1 }
  { print }
' "$root/Cargo.toml" > "$tmp"
cat "$tmp" > "$root/Cargo.toml"

# Cargo.lock: one [[package]] block at a time; a block without `source =` is a workspace member.
awk -v old="$old" -v new="$new" '
  function flush() {
    if (n == 0) return
    for (i = 1; i <= n; i++) {
      if (!has_source && block[i] == "version = \"" old "\"") block[i] = "version = \"" new "\""
      print block[i]
    }
    n = 0; has_source = 0
  }
  /^\[\[package\]\]$/ || /^\[metadata\]$/ || /^\[\[patch\.unused\]\]$/ { flush() }
  { block[++n] = $0; if ($0 ~ /^source = /) has_source = 1 }
  END { flush() }
' "$root/Cargo.lock" > "$tmp"
cat "$tmp" > "$root/Cargo.lock"
echo "$old"
