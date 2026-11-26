#!/usr/bin/env bash
# stage.sh <dromon binary> <root>: lays out the files every Linux package installs under <root>:
#
#   usr/bin/dromon
#   usr/share/applications/dromon.desktop
#   usr/share/icons/hicolor/{16..512}x{16..512}/apps/dromon.png, scalable/apps/dromon.svg
#   usr/share/doc/dromon/copyright              (machine-readable, with the MIT and OFL texts)
#   usr/share/doc/dromon/fonts/*.txt            (the embedded fonts' licenses, verbatim)
#
# build-deb.sh packs this tree; build-arch.sh's PKGBUILD installs the same files.
# shellcheck source-path=SCRIPTDIR source=common.sh
. "$(dirname "$0")/common.sh"

bin=${1:?usage: stage.sh <dromon binary> <root>}
root=${2:?usage: stage.sh <dromon binary> <root>}
[ -x "$bin" ] || die "$bin is not an executable"
[ ! -e "$root" ] || [ -z "$(ls -A "$root")" ] || die "$root is not empty"

install -Dm755 "$bin" "$root/usr/bin/dromon"
install -Dm644 "$PACKAGING_LINUX/dromon.desktop" "$root/usr/share/applications/dromon.desktop"
for size in 16 32 48 64 128 256 512; do
  install -Dm644 "$REPO_ROOT/packaging/icon/dromon-$size.png" "$root/usr/share/icons/hicolor/${size}x${size}/apps/dromon.png"
done
install -Dm644 "$REPO_ROOT/packaging/icon/dromon.svg" "$root/usr/share/icons/hicolor/scalable/apps/dromon.svg"

doc="$root/usr/share/doc/dromon"
fonts="$REPO_ROOT/crates/dromon-app/assets/fonts"
install -Dm644 "$fonts/Inter-LICENSE.txt" "$doc/fonts/Inter-LICENSE.txt"
install -Dm644 "$fonts/GeistMono-OFL.txt" "$doc/fonts/GeistMono-OFL.txt"

# A license text as a DEP-5 paragraph body: every line indented one space, blank lines " .".
dep5_text() {
  sed -e 's/[[:space:]]*$//' -e 's/^$/./' -e 's/^/ /' "$1"
}
{
  cat <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Dromon
Upstream-Contact: $MAINTAINER
Source: $HOMEPAGE

Files: *
Copyright: 2026 Muhammed Fatih Doğmuş
License: MIT

Files: crates/dromon-app/assets/fonts/InterVariable.ttf
Copyright: 2016 The Inter Project Authors (https://github.com/rsms/inter)
License: OFL-1.1
Comment: Embedded in the dromon binary. Full notice: /usr/share/doc/dromon/fonts/Inter-LICENSE.txt

Files: crates/dromon-app/assets/fonts/GeistMonoVariable.ttf
Copyright: 2026 The Geist Project Authors (https://github.com/vercel/geist-font)
License: OFL-1.1
Comment: Embedded in the dromon binary. Full notice: /usr/share/doc/dromon/fonts/GeistMono-OFL.txt

License: MIT
EOF
  # The MIT text without its title and copyright line (they are in the paragraph above).
  dep5_text <(sed '1,/^Copyright/d' "$REPO_ROOT/LICENSE" | sed '/./,$!d')
  echo
  echo "License: OFL-1.1"
  dep5_text <(sed -n '/^SIL OPEN FONT LICENSE/,$p' "$fonts/Inter-LICENSE.txt")
} > "$doc/copyright"
chmod 644 "$doc/copyright"
