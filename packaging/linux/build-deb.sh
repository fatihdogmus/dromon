#!/usr/bin/env bash
# build-deb.sh <dromon binary> <version> <out dir>: packs Dromon-<version>-linux-<arch>.deb with
# dpkg-deb from the tree stage.sh lays out (arch from the binary's ELF header).
#
# Depends: the libraries the binary links, as dpkg-shlibdeps derives them (libc6, libgcc-s1), the
# libraries it dlopens (dlopen-libs.txt, checked against the binary), and openssh-client (the app
# runs the system ssh). Needs dpkg-dev, run where the binary's architecture is native: CI's
# ubuntu-22.04 runners, or the ubuntu:22.04 container scripts/package-linux.sh uses.
# shellcheck source-path=SCRIPTDIR source=common.sh
. "$(dirname "$0")/common.sh"

bin=${1:?usage: build-deb.sh <dromon binary> <version> <out dir>}
version=${2:?usage: build-deb.sh <dromon binary> <version> <out dir>}
out=${3:?usage: build-deb.sh <dromon binary> <version> <out dir>}
command -v dpkg-shlibdeps > /dev/null || die "dpkg-shlibdeps is missing (apt-get install dpkg-dev)"

arch=$(elf_arch "$bin")
mkdir -p "$out"
out=$(cd "$out" && pwd)
deb="$out/Dromon-$version-linux-$arch.deb"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

check_dlopen_table "$bin"
root="$work/root"
bash "$PACKAGING_LINUX/stage.sh" "$bin" "$root"

# dpkg-shlibdeps reads debian/control from the current directory.
mkdir -p "$work/debian"
printf 'Source: dromon\n\nPackage: dromon\nArchitecture: any\n' > "$work/debian/control"
shlibs=$(cd "$work" && dpkg-shlibdeps -O -e"$root/usr/bin/dromon" 2> "$work/shlibdeps.log") ||
  { cat "$work/shlibdeps.log" >&2; die "dpkg-shlibdeps failed"; }
shlibs=${shlibs#shlibs:Depends=}
[ -n "$shlibs" ] || die "dpkg-shlibdeps found no linked libraries"
rm -rf "$work/debian"

dlopen_debs=$(dlopen_table | awk '{ print $2 }' | sort -u | paste -sd, - | sed 's/,/, /g')
depends="$shlibs, openssh-client, $dlopen_debs"

# Reproducible: every file dated at the commit (or SOURCE_DATE_EPOCH), as dpkg-deb then packs it.
epoch=${SOURCE_DATE_EPOCH:-$(git -C "$REPO_ROOT" log -1 --format=%ct 2> /dev/null || date +%s)}
mkdir -p "$root/DEBIAN"
(cd "$root" && find usr -type f -print0 | LC_ALL=C sort -z | xargs -0 md5sum) > "$root/DEBIAN/md5sums"
{
  echo "Package: dromon"
  echo "Version: $(deb_version "$version")"
  echo "Architecture: $(deb_arch "$arch")"
  echo "Maintainer: $MAINTAINER"
  echo "Installed-Size: $(du -sk --exclude=DEBIAN "$root" | cut -f1)"
  echo "Depends: $depends"
  # A Vulkan driver for GPU drawing (any vendor's; Mesa's covers most machines).
  echo "Recommends: mesa-vulkan-drivers | vulkan-icd"
  echo "Section: net"
  echo "Priority: optional"
  echo "Homepage: $HOMEPAGE"
  echo "Description: $SUMMARY"
  printf '%s\n' "$DESCRIPTION" | sed 's/^/ /'
} > "$root/DEBIAN/control"
chmod 644 "$root/DEBIAN/control" "$root/DEBIAN/md5sums"
find "$root" -exec touch -h -d "@$epoch" {} +

rm -f "$deb"
SOURCE_DATE_EPOCH=$epoch dpkg-deb --root-owner-group -Zxz --build "$root" "$deb" > /dev/null

# What went in, and the packaged binary still runs (when it is this machine's architecture).
dpkg-deb --field "$deb" Package Version Architecture Depends
contents=$(dpkg-deb --contents "$deb")
grep -q '\./usr/bin/dromon$' <<< "$contents" || die "$deb has no /usr/bin/dromon"
if [ "$arch" = "$(elf_arch /bin/sh)" ]; then
  dpkg-deb --extract "$deb" "$work/check"
  reported=$("$work/check/usr/bin/dromon" --version)
  [ "$reported" = "dromon $version" ] || die "the packaged dromon --version printed '$reported', not 'dromon $version'"
fi
echo "$deb"
