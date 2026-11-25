#!/bin/sh
# Dromon installer for macOS (Apple silicon) and Linux (x86_64: Debian/Ubuntu, Arch).
#
#   curl -fsSL https://raw.githubusercontent.com/fatihdogmus/dromon/main/install.sh | sh
#
# Finds the latest GitHub release, downloads the package for this machine, checks its SHA-256
# against the release's SHA256SUMS and installs it without prompts (Linux: .deb via apt-get or
# .pkg.tar.zst via pacman, using sudo when not root; macOS: Dromon.app into /Applications).
#
# Options, as environment variables (they work through a pipe):
#
#   DROMON_VERSION      install this release instead of the latest ("0.1.0" or "v0.1.0")
#   DROMON_FORCE=1      install even if that version is already installed
#   DROMON_DRY_RUN=1    do everything up to and including the download and the checksum check,
#                       then print the install command instead of running it
#   DROMON_RELEASE_URL  test hook: base URL that holds the release's files, used instead of
#                       github.com (needs DROMON_VERSION)
#
# Arguments (after "sh -s --"; they win over the environment):
#
#   --version <v>       same as DROMON_VERSION
#   -h, --help          print the usage
#
#   curl -fsSL https://raw.githubusercontent.com/fatihdogmus/dromon/main/install.sh | sh -s -- --version 0.1.0
#
# POSIX sh. Everything lives in functions and the only top-level command is the last line, so a
# truncated download runs nothing.

REPO=fatihdogmus/dromon
REPO_URL=https://github.com/$REPO
SOURCE_URL=$REPO_URL#build-from-source

REQ_VERSION=
TMP=
MNT=
DRY=0
USE_ROOT=0

say() { printf 'dromon-install: %s\n' "$*"; }
err() { printf 'dromon-install: error: %s\n' "$*" >&2; }
die() {
  err "$*"
  exit 1
}

cleanup() {
  status=$?
  trap - EXIT
  if [ -n "$MNT" ]; then
    hdiutil detach "$MNT" -quiet >/dev/null 2>&1 || hdiutil detach "$MNT" -force -quiet >/dev/null 2>&1 || true
  fi
  if [ -n "$TMP" ]; then
    rm -rf "$TMP"
  fi
  exit "$status"
}

# Runs a command as root: directly when we are root, else through sudo (it may prompt once).
as_root() {
  if [ "$(id -u)" = 0 ]; then
    "$@"
  else
    sudo "$@"
  fi
}

root_prefix() {
  if [ "$(id -u)" = 0 ]; then
    return 0
  fi
  printf 'sudo '
}

download() {
  # download <url> <file>
  curl -fsSL --retry 2 -o "$2" "$1" || die "could not download $1"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    die "need sha256sum or shasum to check the download"
  fi
}

# detect_platform sets OS (linux|macos), ASSET_SUFFIX (the part of the file name after the
# version) and, on Linux, PKG_MANAGER (apt|pacman).
detect_platform() {
  kernel=$(uname -s)
  machine=$(uname -m)
  case "$kernel" in
    Linux)
      case "$machine" in
        x86_64 | amd64) ;;
        *) die "no Dromon build for Linux $machine (x86_64 only). Build from source: $SOURCE_URL" ;;
      esac
      OS=linux
      if command -v apt-get >/dev/null 2>&1; then
        PKG_MANAGER=apt
        ASSET_SUFFIX=linux-x64.deb
      elif command -v pacman >/dev/null 2>&1; then
        PKG_MANAGER=pacman
        ASSET_SUFFIX=linux-x64.pkg.tar.zst
      else
        die "unsupported Linux distribution: packages exist for Debian/Ubuntu (apt-get) and Arch Linux (pacman). Build from source: $SOURCE_URL"
      fi
      USE_ROOT=1
      ;;
    Darwin)
      if [ "$machine" != arm64 ]; then
        # A terminal run under Rosetta reports x86_64 on an Apple silicon Mac.
        if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" != 1 ] &&
          [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" != 1 ]; then
          die "no Intel Mac build (Apple silicon only). Build from source: $SOURCE_URL"
        fi
      fi
      OS=macos
      ASSET_SUFFIX=macos-arm64.dmg
      # Writing to /Applications (or replacing an app there) needs sudo unless we may do it.
      if [ ! -w /Applications ] || { [ -e /Applications/Dromon.app ] && [ ! -w /Applications/Dromon.app ]; }; then
        USE_ROOT=1
      fi
      ;;
    MINGW* | MSYS* | CYGWIN*)
      die "this is a Windows shell ($kernel). Use PowerShell instead: irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex"
      ;;
    *)
      die "unsupported system $kernel $machine. On Windows use install.ps1; otherwise build from source: $SOURCE_URL"
      ;;
  esac
}

# If installing needs root, fail now (before downloading) when we can neither be root nor sudo.
check_root() {
  if [ "$USE_ROOT" != 1 ] || [ "$(id -u)" = 0 ] || [ "$DRY" = 1 ]; then
    return 0
  fi
  if command -v sudo >/dev/null 2>&1; then
    return 0
  fi
  err "installing needs root and sudo is not available."
  err "Run the installer as root, e.g.: su -c 'curl -fsSL $INSTALLER_URL | sh'"
  exit 1
}

# resolve_version sets VERSION (without the "v").
resolve_version() {
  if [ -n "$REQ_VERSION" ]; then
    VERSION=${REQ_VERSION#v}
  else
    if [ -n "${DROMON_RELEASE_URL:-}" ]; then
      die "DROMON_RELEASE_URL needs DROMON_VERSION (or --version)"
    fi
    # The redirect of /releases/latest names the newest published release (never a draft or
    # prerelease) and, unlike the API, is not rate limited.
    final=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$REPO_URL/releases/latest") ||
      die "could not reach $REPO_URL/releases/latest"
    case "$final" in
      */releases/tag/*) VERSION=${final##*/releases/tag/} ;;
      *) die "no Dromon release has been published yet ($REPO_URL/releases). Build from source: $SOURCE_URL" ;;
    esac
    VERSION=${VERSION#v}
  fi
  case "$VERSION" in
    '' | *[!0-9A-Za-z._+-]*) die "invalid version '$VERSION'" ;;
  esac
}

usage() {
  cat <<EOF
Usage: install.sh [--version <version>]

Installs Dromon from its latest GitHub release.

  --version <v>   install this release ("0.1.0" or "v0.1.0") instead of the latest
  -h, --help      show this help

Environment: DROMON_VERSION, DROMON_FORCE=1, DROMON_DRY_RUN=1, DROMON_RELEASE_URL (see the top of
the script).
EOF
}

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      -h | --help)
        usage
        exit 0
        ;;
      --version)
        [ $# -ge 2 ] && [ -n "$2" ] || {
          err "--version needs a value"
          usage >&2
          exit 1
        }
        REQ_VERSION=$2
        shift 2
        ;;
      --version=*)
        REQ_VERSION=${1#--version=}
        shift
        ;;
      *)
        err "unknown option: $1"
        usage >&2
        exit 1
        ;;
    esac
  done
}

# installed_version prints the version of the installed Dromon, or nothing.
installed_version() {
  case "$OS" in
    linux) out=$(dromon --version 2>/dev/null) || return 0 ;;
    macos) out=$(/Applications/Dromon.app/Contents/MacOS/dromon --version 2>/dev/null) || return 0 ;;
  esac
  # "dromon <version>"
  printf '%s\n' "$out" | awk 'NR == 1 && $1 == "dromon" { print $2 }'
}

# verify_checksum <file name>: the file in $TMP against its line in $TMP/SHA256SUMS.
verify_checksum() {
  name=$1
  expected=$(awk -v f="$name" '{ n = $2; sub(/^\*/, "", n); if (n == f) { print $1; exit } }' "$TMP/SHA256SUMS")
  if [ -z "$expected" ]; then
    die "SHA256SUMS has no line for $name; not installing"
  fi
  actual=$(sha256_of "$TMP/$name")
  expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
  if [ "$actual" != "$expected" ]; then
    err "SHA-256 mismatch for $name"
    err "  expected $expected"
    err "  actual   $actual"
    die "the download is corrupt or was tampered with; not installing"
  fi
  say "SHA-256 verified ($actual)"
}

install_linux() {
  name=$1
  # apt's sandbox user (_apt) must be able to read the file.
  chmod 755 "$TMP"
  cd "$TMP"
  case "$PKG_MANAGER" in
    apt)
      if [ "$DRY" = 1 ]; then
        say "dry run: would run: $(root_prefix)env DEBIAN_FRONTEND=noninteractive apt-get install -y ./$name"
        return 0
      fi
      say "installing with apt-get"
      # The ./ form makes apt install the local file and resolve its dependencies.
      as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "./$name" || die "apt-get failed"
      ;;
    pacman)
      if [ "$DRY" = 1 ]; then
        say "dry run: would run: $(root_prefix)pacman -U --noconfirm $name"
        return 0
      fi
      say "installing with pacman"
      as_root pacman -U --noconfirm "$TMP/$name" || die "pacman failed"
      ;;
  esac
  say "installed:"
  dromon --version || err "installed, but running 'dromon --version' failed"
}

install_macos() {
  name=$1
  dest=/Applications/Dromon.app
  MNT=$TMP/mnt
  if [ "$DRY" = 1 ]; then
    say "dry run: would mount $name and copy Dromon.app to $dest (replacing an existing copy)"
    MNT=
    return 0
  fi
  mkdir -p "$MNT"
  say "mounting the disk image"
  if ! hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$MNT" "$TMP/$name" >/dev/null; then
    MNT=
    die "could not mount $name"
  fi
  [ -d "$MNT/Dromon.app" ] || die "Dromon.app is not in the disk image"
  say "copying Dromon.app to /Applications"
  if [ "$USE_ROOT" = 1 ]; then
    as_root rm -rf "$dest" || die "could not remove the old $dest"
    as_root ditto "$MNT/Dromon.app" "$dest" || die "could not copy Dromon.app to /Applications"
  else
    rm -rf "$dest" || die "could not remove the old $dest"
    ditto "$MNT/Dromon.app" "$dest" || die "could not copy Dromon.app to /Applications"
  fi
  # No quarantine handling on purpose: files downloaded by curl carry no com.apple.quarantine
  # attribute, so Gatekeeper does not prompt even though the app is not notarized.
  say "installed:"
  "$dest/Contents/MacOS/dromon" --version || err "installed, but running dromon --version failed"
}

main() {
  set -eu
  INSTALLER_URL=https://raw.githubusercontent.com/$REPO/main/install.sh
  if [ "${DROMON_DRY_RUN:-}" = 1 ]; then
    DRY=1
  fi
  REQ_VERSION=${DROMON_VERSION:-}
  parse_args "$@"
  command -v curl >/dev/null 2>&1 || die "curl is required"

  detect_platform
  check_root
  resolve_version

  if [ "${DROMON_FORCE:-}" != 1 ]; then
    current=$(installed_version)
    if [ "$current" = "$VERSION" ]; then
      say "Dromon $VERSION is already installed (DROMON_FORCE=1 reinstalls)"
      return 0
    fi
  fi

  if [ -n "${DROMON_RELEASE_URL:-}" ]; then
    base=${DROMON_RELEASE_URL%/}
  else
    base=$REPO_URL/releases/download/v$VERSION
  fi
  name=Dromon-$VERSION-$ASSET_SUFFIX

  TMP=$(mktemp -d "${TMPDIR:-/tmp}/dromon-install.XXXXXX") || die "could not create a temporary folder"
  trap cleanup EXIT
  trap 'exit 1' HUP INT TERM

  say "installing Dromon $VERSION ($name)"
  say "downloading $name"
  download "$base/$name" "$TMP/$name"
  download "$base/SHA256SUMS" "$TMP/SHA256SUMS"
  verify_checksum "$name"

  case "$OS" in
    linux) install_linux "$name" ;;
    macos) install_macos "$name" ;;
  esac
}

main "$@"
