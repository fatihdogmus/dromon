#!/usr/bin/env bash
# build-arch.sh <Dromon .deb> <version> <out dir>: makes Dromon-<version>-linux-<arch>.pkg.tar.zst
# with makepkg in the archlinux:base-devel container, from packaging/arch/PKGBUILD, which installs
# the .deb's files (so both packages carry the same bytes).
#
# The container is x86_64 (Arch publishes no arm64 image); nothing is compiled or run from the
# package, so the arm64 package is made there too, with CARCH=aarch64 and no dependency check.
#
# Environment: ARCH_IMAGE (default archlinux:base-devel).
# shellcheck source-path=SCRIPTDIR source=common.sh
. "$(dirname "$0")/common.sh"

deb=${1:?usage: build-arch.sh <Dromon .deb> <version> <out dir>}
version=${2:?usage: build-arch.sh <Dromon .deb> <version> <out dir>}
out=${3:?usage: build-arch.sh <Dromon .deb> <version> <out dir>}
image=${ARCH_IMAGE:-archlinux:base-devel}
pkgbuild="$REPO_ROOT/packaging/arch/PKGBUILD"
command -v docker > /dev/null || die "docker is missing"

case "$(basename "$deb")" in
  "Dromon-$version-linux-x64.deb") arch=x64 ;;
  "Dromon-$version-linux-arm64.deb") arch=arm64 ;;
  *) die "$deb is not named Dromon-$version-linux-<x64|arm64>.deb" ;;
esac
carch=$(pacman_arch "$arch")

# The PKGBUILD's depends=() must hold every Arch package dlopen-libs.txt maps a library to.
depends=$(sed -n 's/^depends=(\(.*\))$/\1/p' "$pkgbuild" | tr ' ' '\n')
missing=$(comm -23 <(dlopen_table | awk '{ print $3 }' | sort -u) <(printf '%s\n' "$depends" | sort -u))
[ -z "$missing" ] || die "packaging/arch/PKGBUILD depends=() lacks: $(echo "$missing" | tr '\n' ' ')"

mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "$pkgbuild" "$work/PKGBUILD"
cp "$deb" "$work/"
sha=$(sha256sum "$deb" | cut -c1-64)
epoch=${SOURCE_DATE_EPOCH:-$(git -C "$REPO_ROOT" log -1 --format=%ct 2> /dev/null || date +%s)}

# makepkg refuses to run as root: build as a throwaway user, then hand the files back.
docker run --rm --network none \
  -e DROMON_VERSION="$version" -e DROMON_DEB="$(basename "$deb")" -e DROMON_DEB_SHA256="$sha" \
  -e CARCH_OVERRIDE="$carch" -e SOURCE_DATE_EPOCH="$epoch" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  -v "$work:/build" "$image" bash -euo pipefail -c '
    useradd -m builder
    chown -R builder /build
    if [ "$CARCH_OVERRIDE" != "$(uname -m)" ]; then
      install -d -o builder /home/builder/.config/pacman
      printf "CARCH=%s\nCHOST=%s-unknown-linux-gnu\n" "$CARCH_OVERRIDE" "$CARCH_OVERRIDE" > /home/builder/.config/pacman/makepkg.conf
    fi
    cd /build
    runuser -u builder -- env DROMON_VERSION="$DROMON_VERSION" DROMON_DEB="$DROMON_DEB" \
      DROMON_DEB_SHA256="$DROMON_DEB_SHA256" SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" PKGDEST=/build/out \
      makepkg --nodeps --noconfirm --force
    chown -R "$HOST_UID:$HOST_GID" /build
  '

built=("$work"/out/dromon-*-"$carch".pkg.tar.zst)
[ "${#built[@]}" = 1 ] && [ -f "${built[0]}" ] || die "makepkg made no single dromon package for $carch"
target="$out/Dromon-$version-linux-$arch.pkg.tar.zst"
mv "${built[0]}" "$target"

# What went in: the package's metadata and its binary is the .deb's.
info=$(tar --zstd -xOf "$target" .PKGINFO)
grep -Fxq "arch = $carch" <<< "$info" || die "$target is not for $carch"
grep -Fxq "pkgver = $(pacman_version "$version")-1" <<< "$info" || die "$target is not version $version"
grep -E '^(pkgname|pkgver|arch|depend) = ' <<< "$info"
cmp -s <(tar --zstd -xOf "$target" usr/bin/dromon) <(dpkg-deb --fsys-tarfile "$deb" | tar -xO ./usr/bin/dromon) ||
  die "$target's usr/bin/dromon differs from the .deb's"
echo "$target"
