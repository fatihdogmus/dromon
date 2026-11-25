# shellcheck shell=bash disable=SC2034 # its variables are used by the scripts that source it
# Shared helpers for packaging/linux/*.sh and scripts/package-linux.sh. Sourced, never executed.

set -euo pipefail

PACKAGING_LINUX="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$PACKAGING_LINUX/../.." && pwd)"
# Who the packages name as maintainer (Debian Maintainer:, the PKGBUILDs' header).
MAINTAINER="Muhammed Fatih Doğmuş <fdogmus06@gmail.com>"
HOMEPAGE="https://github.com/fatihdogmus/dromon"
SUMMARY="Fast SFTP file browser"
DESCRIPTION="Dromon is a small, fast SFTP file browser that uses the system's OpenSSH client
for connections, so ~/.ssh/config, agents, ProxyJump and keys work as they do in a terminal."

die() {
  echo "error: $*" >&2
  exit 1
}

# [workspace.package].version from the root Cargo.toml.
workspace_version() {
  awk '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
  ' "$REPO_ROOT/Cargo.toml"
}

# x64 or arm64, from the ELF header's e_machine (offset 18, little-endian), so packaging an
# arm64 binary on an x64 host labels it right.
elf_arch() {
  local machine
  machine=$(od -An -tu2 -j18 -N2 "$1" | tr -d ' ')
  case "$machine" in
    62) echo x64 ;;
    183) echo arm64 ;;
    *) die "$1: unsupported ELF machine $machine" ;;
  esac
}

deb_arch() {
  case "$1" in x64) echo amd64 ;; arm64) echo arm64 ;; *) die "unknown arch $1" ;; esac
}

pacman_arch() {
  case "$1" in x64) echo x86_64 ;; arm64) echo aarch64 ;; *) die "unknown arch $1" ;; esac
}

# 0.1.0-rc.1 -> 0.1.0~rc.1 (sorts before 0.1.0 in dpkg).
deb_version() {
  printf '%s\n' "${1/-/\~}"
}

# 0.1.0-rc.1 -> 0.1.0rc.1 (pacman: 1.0rc < 1.0; pkgver may not hold '-').
pacman_version() {
  printf '%s\n' "${1//-/}"
}

# The vetted dlopen table, one "soname debian arch" line each.
dlopen_table() {
  grep -Ev '^[[:space:]]*(#|$)' "$PACKAGING_LINUX/dlopen-libs.txt"
}

# Fails when the binary names a lib*.so.N that it neither links (NEEDED) nor has in the table.
check_dlopen_table() {
  local bin=$1 needed named unknown
  needed=$(readelf -d "$bin" | sed -n 's/.*(NEEDED).*\[\(.*\)\].*/\1/p' | sort -u)
  named=$(grep -aoE 'lib[A-Za-z0-9_+-]+\.so\.[0-9]+' "$bin" | sort -u)
  unknown=$(comm -23 <(printf '%s\n' "$named") <(printf '%s\n%s\n' "$needed" "$(dlopen_table | awk '{ print $1 }')" | sort -u))
  [ -z "$unknown" ] || die "$bin loads libraries packaging/linux/dlopen-libs.txt doesn't map: $(echo "$unknown" | tr '\n' ' ')"
  local stale
  stale=$(comm -13 <(printf '%s\n' "$named") <(dlopen_table | awk '{ print $1 }' | sort -u))
  [ -z "$stale" ] || echo "warning: dlopen-libs.txt lists libraries $bin no longer names: $(echo "$stale" | tr '\n' ' ')" >&2
}
