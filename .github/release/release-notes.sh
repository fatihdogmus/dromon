#!/usr/bin/env bash
# release-notes.sh <release dir>: prints the release notes' files part (Markdown): every file,
# its SHA-256, whether and how it is signed, and how a user checks it, with the steps to open an
# unsigned build. publish-github-release.sh puts the changes since the last release above it.
#
# Environment: VERSION, MODE (release|check), GITHUB_REPOSITORY, GITHUB_SERVER_URL,
# WINDOWS_SIGNING (none|azure|sslcom|signpath), MACOS_SIGNED, GPG_SIGNED (true|false),
# GPG_FINGERPRINT.
# shellcheck source-path=SCRIPTDIR source=lib.sh
. "$(dirname "$0")/lib.sh"

dir=${1:?usage: release-notes.sh <release dir>}
[ -f "$dir/SHA256SUMS" ] || die "$dir has no SHA256SUMS."
repo=${GITHUB_REPOSITORY:?}
server=${GITHUB_SERVER_URL:-https://github.com}
v=${VERSION:?}
windows=${WINDOWS_SIGNING:-none}
macos=${MACOS_SIGNED:-false}
gpg=${GPG_SIGNED:-false}
download="$server/$repo/releases/download/$v"

case "$windows" in
  none) windows_status=":warning: **Unsigned**: Windows SmartScreen warns before it runs" ;;
  signpath) windows_status="Authenticode-signed (installer and \`dromon.exe\`); publisher **SignPath Foundation**" ;;
  *) windows_status="Authenticode-signed (installer and \`dromon.exe\`), timestamped" ;;
esac
if [ "$macos" = true ]; then
  macos_status="Developer ID-signed, notarized by Apple, ticket stapled"
else
  macos_status=":warning: **Not notarized** (ad-hoc signed): macOS blocks the first open"
fi
if [ "$gpg" = true ]; then deb_status="GPG-signed (\`.asc\`)" pkg_status="GPG-signed (\`.sig\`)"; else deb_status="No GPG signature" pkg_status="No GPG signature"; fi

row() {
  local file=$1 platform=$2 status=$3 sum
  sum=$(awk -v f="$file" '$2 == f || $2 == "*" f { print $1 }' "$dir/SHA256SUMS")
  [ -n "$sum" ] || die "SHA256SUMS has no $file."
  echo "| [\`$file\`]($download/$file) | $platform | $status | \`${sum:0:16}…\` |"
}

rows=$(
  row "Dromon-$v-windows-x64.msi" "Windows 11, x64 (Arm runs it emulated)" "$windows_status"
  row "Dromon-$v-macos-arm64.dmg" "macOS 14+, Apple silicon (not Intel Macs)" "$macos_status"
  row "Dromon-$v-linux-x64.deb" "Debian 12+, Ubuntu 22.04+, x64" "$deb_status"
  row "Dromon-$v-linux-x64.pkg.tar.zst" "Arch Linux, x86_64" "$pkg_status"
)

cat <<EOF
## Downloads

| File | For | Signature | SHA-256 |
|---|---|---|---|
$rows

Every file has a [build provenance attestation]($server/$repo/attestations) (Sigstore): it was built by this repository's release workflow, run from \`main\`, at tag \`$v\`. \`SHA256SUMS\` lists every file's SHA-256$([ "$gpg" != true ] || echo "; \`SHA256SUMS.asc\` is its GPG signature").

## Check a download

Any file, with the [GitHub CLI](https://cli.github.com):

\`\`\`sh
gh attestation verify Dromon-$v-<platform>.<ext> --repo $repo
\`\`\`
EOF

if [ "$gpg" = true ]; then
  cat <<EOF

Any file, with GPG and the release key (fingerprint \`${GPG_FINGERPRINT:-}\`; check it against the project README, not only this page):

\`\`\`sh
curl -LO $download/dromon-release-key.asc
gpg --show-keys dromon-release-key.asc     # the fingerprint above
gpg --import dromon-release-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
\`\`\`
EOF
else
  cat <<EOF

Any file, against \`SHA256SUMS\` (not GPG-signed in this build; the attestation above is the signed proof):

\`\`\`sh
sha256sum --check --ignore-missing SHA256SUMS
\`\`\`
EOF
fi

echo
echo "### Windows"
echo
if [ "$windows" = none ]; then
  cat <<'EOF'
This build is **not code-signed**. When you open the installer, Windows SmartScreen says "Windows protected your PC": click **More info**, check the file name, then **Run anyway**. Check the file first with `gh attestation verify` or `SHA256SUMS` above.
EOF
else
  cat <<EOF
In PowerShell, \`Status\` must be \`Valid\`$([ "$windows" != signpath ] || echo " and the signer \`SignPath Foundation\`"):

\`\`\`powershell
Get-AuthenticodeSignature .\\Dromon-$v-windows-x64.msi | Format-List Status, SignerCertificate
\`\`\`
EOF
fi
echo
echo "Installs per machine to \`C:\\Program Files\\Dromon\`; uninstall from Settings → Apps."

echo
echo "### macOS"
echo
echo "With Homebrew (the cask follows each published release): \`brew install --cask ${repo%%/*}/tap/dromon\`"
echo
if [ "$macos" = true ]; then
  cat <<EOF
Open the DMG and drag Dromon to Applications. To check it:

\`\`\`sh
spctl --assess --type open --context context:primary-signature -v Dromon-$v-macos-arm64.dmg
codesign --verify --deep --strict -v /Applications/Dromon.app
spctl --assess --type execute -vv /Applications/Dromon.app    # "accepted, source=Notarized Developer ID"
\`\`\`
EOF
else
  cat <<'EOF'
This build is **not notarized** (no Apple Developer ID yet), so macOS refuses the first open ("Apple could not verify…"). Check the file first (above), drag Dromon to Applications, then either:

- open Dromon once, then **System Settings → Privacy & Security**, scroll to Security, click **Open Anyway** next to Dromon, and confirm (macOS 15 and later have no right-click → Open bypass), or
- in Terminal: `xattr -dr com.apple.quarantine /Applications/Dromon.app`
EOF
fi

echo
echo "### Linux"
echo
if [ "$gpg" = true ]; then
  cat <<EOF
Debian, Ubuntu (pulls in openssh-client):

\`\`\`sh
gpg --verify Dromon-$v-linux-x64.deb.asc Dromon-$v-linux-x64.deb
sudo apt install ./Dromon-$v-linux-x64.deb
\`\`\`

Arch: \`pacman -U\` checks the \`.sig\` beside the package against pacman's own keyring, so trust the release key there once:

\`\`\`sh
sudo pacman-key --add dromon-release-key.asc
sudo pacman-key --lsign-key ${GPG_FINGERPRINT:-}
sudo pacman -U Dromon-$v-linux-x64.pkg.tar.zst
\`\`\`
EOF
else
  cat <<EOF
\`\`\`sh
sudo apt install ./Dromon-$v-linux-x64.deb          # Debian, Ubuntu (pulls in openssh-client)
sudo pacman -U Dromon-$v-linux-x64.pkg.tar.zst      # Arch
\`\`\`
EOF
fi
if [ "${MODE:-release}" != release ]; then
  echo
  echo "_Dry run: nothing was published or attested._"
fi
