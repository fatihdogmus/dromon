#!/usr/bin/env bash
# verify-packages.sh <dir> <version> <x64|arm64> [--key <public key>]: installs the packages in
# <dir> the way a user would, each in a clean container, and runs `dromon --version`:
#
#   Dromon-<version>-linux-<arch>.deb          apt-get install ./… in debian:12 and ubuntu:24.04
#                                             (must pull openssh-client and the libraries)
#   Dromon-<version>-linux-<arch>.pkg.tar.zst  pacman -U in archlinux:latest (x64; Arch has no
#                                             arm64 image, so arm64 gets static checks only)
#
# Each install must also put the app icon in /usr/share/icons/hicolor/ at every size (the
# scalable SVG and each PNG), and the deb its desktop file and copyright.
#
# With --key, also checks each package's detached signature (.deb.asc, .pkg.tar.zst.sig) and
# SHA256SUMS.asc when present, against that key alone (a temporary GNUPGHOME).
# Runs containers for the host's architecture: verify arm64 on an arm64 machine.
# shellcheck source-path=SCRIPTDIR source=common.sh
. "$(dirname "$0")/common.sh"

dir=${1:?usage: verify-packages.sh <dir> <version> <x64|arm64> [--key <public key>]}
version=${2:?usage: verify-packages.sh <dir> <version> <x64|arm64> [--key <public key>]}
arch=${3:?usage: verify-packages.sh <dir> <version> <x64|arm64> [--key <public key>]}
key=""
if [ "${4:-}" = --key ]; then key=${5:?--key needs a file}; fi
dir=$(cd "$dir" && pwd)
deb="Dromon-$version-linux-$arch.deb"
pkg="Dromon-$version-linux-$arch.pkg.tar.zst"
expected="dromon $version"
[ -f "$dir/$deb" ] || die "$dir has no $deb"
[ -f "$dir/$pkg" ] || die "$dir has no $pkg"
host=$(elf_arch /bin/sh)
[ "$arch" = "$host" ] || die "can't install $arch packages on this $host machine"
# Every gpg call uses a throwaway keyring holding only --key, never the caller's ~/.gnupg.
GNUPGHOME=$(mktemp -d)
export GNUPGHOME
chmod 700 "$GNUPGHOME"
trap 'gpgconf --kill gpg-agent > /dev/null 2>&1 || true; rm -rf "$GNUPGHOME"' EXIT
if [ -n "$key" ]; then gpg --batch --quiet --import "$key"; fi

summary() {
  echo "$*"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then echo "- $*" >> "$GITHUB_STEP_SUMMARY"; fi
}

# The app icon at every size of the brand kit's hicolor tree (stage.sh), checked inside each
# container: `test -f` for each, so a missing size fails the install check.
icons="scalable/apps/dromon.svg"
for size in 16 24 32 48 64 128 256 512; do icons+=" ${size}x${size}/apps/dromon.png"; done

# Docker Hub's official images through Google's mirror: Docker Hub rate-limits anonymous pulls.
hub=${DROMON_IMAGE_MIRROR-mirror.gcr.io/library/}
for image in debian:12 ubuntu:24.04; do
  reported=$(docker run --rm -v "$dir:/pkg:ro" -e DEB="$deb" -e ICONS="$icons" "$hub$image" bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    # A mirror mid-sync serves index files of the wrong size for a moment ("Mirror sync in
    # progress?"): try again rather than fail the check.
    for try in 1 2 3 4; do
      apt-get update -qq -o Acquire::Retries=3 > /dev/null && break
      [ "$try" -lt 4 ] || exit 1
      echo "apt-get update failed (try $try); again in $((try * 15)) s" >&2
      sleep $((try * 15))
    done
    apt-get install -y -qq --no-install-recommends -o Acquire::Retries=3 "/pkg/$DEB" > /dev/null
    command -v ssh > /dev/null || { echo "openssh-client was not installed" >&2; exit 1; }
    test -f /usr/share/applications/dromon.desktop
    for icon in $ICONS; do
      test -f "/usr/share/icons/hicolor/$icon" || { echo "no icon /usr/share/icons/hicolor/$icon" >&2; exit 1; }
    done
    test -f /usr/share/doc/dromon/copyright
    dromon --version
  ') || die "$deb failed to install or run in $image"
  [ "$reported" = "$expected" ] || die "dromon --version in $image printed '$reported', not '$expected'"
  summary "$deb: installs in $image (with openssh-client), \`dromon --version\` = $reported"
done

if [ "$arch" = x64 ]; then
  # pacman -U checks a .sig lying beside the package against its own keyring, as it would for a
  # user who downloaded both: trust the release key there first, as the release notes say.
  key_mount=()
  fingerprint=""
  if [ -n "$key" ] && [ -f "$dir/$pkg.sig" ]; then
    key_mount=(-v "$(cd "$(dirname "$key")" && pwd)/$(basename "$key"):/release-key.asc:ro")
    fingerprint=$(gpg --batch --with-colons --list-keys | awk -F: '$1 == "fpr" { print $10; exit }')
  fi
  reported=$(docker run --rm -v "$dir:/pkg:ro" "${key_mount[@]}" -e PKG="$pkg" -e FINGERPRINT="$fingerprint" -e ICONS="$icons" "${hub}archlinux:latest" bash -euo pipefail -c '
    for try in 1 2 3 4; do
      pacman -Syu --noconfirm --noprogressbar > /dev/null && break
      [ "$try" -lt 4 ] || exit 1
      echo "pacman -Syu failed (try $try); again in $((try * 15)) s" >&2
      sleep $((try * 15))
    done
    if [ -n "$FINGERPRINT" ]; then
      pacman-key --init > /dev/null 2>&1
      pacman-key --add /release-key.asc > /dev/null 2>&1
      pacman-key --lsign-key "$FINGERPRINT" > /dev/null 2>&1
    fi
    pacman -U --noconfirm --noprogressbar "/pkg/$PKG" > /dev/null
    command -v ssh > /dev/null || { echo "openssh was not installed" >&2; exit 1; }
    for icon in $ICONS; do
      test -f "/usr/share/icons/hicolor/$icon" || { echo "no icon /usr/share/icons/hicolor/$icon" >&2; exit 1; }
    done
    test -f /usr/share/licenses/dromon/copyright
    dromon --version
  ') || die "$pkg failed to install or run in archlinux:latest"
  [ "$reported" = "$expected" ] || die "dromon --version in archlinux:latest printed '$reported', not '$expected'"
  summary "$pkg: installs in archlinux:latest (with openssh$([ -z "$fingerprint" ] || echo ", pacman checked its .sig against the release key")), \`dromon --version\` = $reported"
else
  info=$(tar --zstd -xOf "$dir/$pkg" .PKGINFO)
  grep -Fxq "arch = $(pacman_arch "$arch")" <<< "$info" || die "$pkg is not for $(pacman_arch "$arch")"
  cmp -s <(tar --zstd -xOf "$dir/$pkg" usr/bin/dromon) <(dpkg-deb --fsys-tarfile "$dir/$deb" | tar -xO ./usr/bin/dromon) ||
    die "$pkg's usr/bin/dromon differs from the .deb's"
  summary "$pkg: static checks only (no arm64 Arch image): arch = aarch64, binary identical to the .deb's"
fi

if [ -n "$key" ]; then
  for pair in "$deb.asc:$deb" "$pkg.sig:$pkg" "SHA256SUMS.asc:SHA256SUMS"; do
    signature=${pair%%:*} file=${pair#*:}
    if [ ! -f "$dir/$signature" ]; then
      [ "$signature" = SHA256SUMS.asc ] || die "$dir has no $signature"
      continue
    fi
    gpg --batch --verify "$dir/$signature" "$dir/$file" 2> "$GNUPGHOME/verify.log" || { cat "$GNUPGHOME/verify.log" >&2; die "$signature does not verify"; }
    summary "$signature: good signature ($(sed -n 's/.*using [A-Z]* key \([0-9A-F]*\).*/\1/p' "$GNUPGHOME/verify.log" | head -n1))"
  done
fi
