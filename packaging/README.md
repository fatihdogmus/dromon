# Packaging

How Dromon's release files are made. `.github/workflows/release.yml` runs the build scripts, and so
can you; `docs/releasing.md` covers releases, signing and the security boundary. The `verify-*`
scripts are for checking packages by hand (install, run, uninstall); the release workflow doesn't
run them.

```
packaging/
├─ icon/        the app icon
│  ├─ dromon.svg                 vector source (written from generate.py's shapes)
│  ├─ generate.py                renders everything below (Pillow); fails if dromon.svg drifted
│  ├─ dromon-{16…1024}.png       Linux hicolor icons, Windows/macOS sources
│  ├─ dromon.ico                 Windows (16–256 px): the exe, the MSI's shortcut and ARP entry
│  └─ dromon.icns                macOS (16–1024 px, @1x/@2x)
├─ linux/
│  ├─ common.sh                  shared helpers: version, ELF arch, maintainer, dlopen table check
│  ├─ stage.sh                   the /usr tree every Linux package installs
│  ├─ build-deb.sh               Dromon-<v>-linux-<arch>.deb with dpkg-deb (Depends: from
│  │                             dpkg-shlibdeps + dlopen-libs.txt + openssh-client)
│  ├─ build-arch.sh              Dromon-<v>-linux-<arch>.pkg.tar.zst: makepkg in archlinux:base-devel
│  ├─ verify-packages.sh         installs both in debian:12, ubuntu:24.04, archlinux:latest; GPG checks
│  ├─ dlopen-libs.txt            libraries the binary dlopens → Debian and Arch package names
│  └─ dromon.desktop             desktop entry (StartupWMClass = the app's app_id, "dromon")
├─ arch/
│  ├─ PKGBUILD                   the release's Arch package, from the .deb's files
│  └─ PKGBUILD.aur               template for a future dromon-bin AUR package (sha256 + validpgpkeys)
├─ macos/
│  ├─ Info.plist                 the bundle's Info.plist; the one place the bundle id lives
│  ├─ build-app.sh               universal Dromon.app (Apple silicon + Intel, ad-hoc signed), its zip, the unsigned DMG
│  ├─ dmg/                       the DMG's installer window: background (make-background.py), dmgbuild settings
│  └─ verify-dmg.sh              opens a DMG and checks the app as a user's Mac would
├─ windows/
│  ├─ build-exe.ps1              dromon.exe for x64/arm64, version resource + icon stamped (rcedit)
│  ├─ dromon.wxs                 WiX v5 installer source (fixed UpgradeCode)
│  ├─ build-msi.ps1              Dromon-<v>-windows-<arch>.msi
│  └─ verify-msi.ps1             silent install, --version, signatures, uninstall
└─ signpath/
   └─ dromon-msi.xml             SignPath artifact configuration (deep-signs exe-in-MSI, then the MSI)
```

`scripts/package-linux.sh` builds the Linux binary (in an `ubuntu:22.04` container by default)
and runs `build-deb.sh` and `build-arch.sh`; add `--verify` to install the results in clean
containers.

**The icon is a placeholder**: a simple galley (sail, hull, oars) in mint `#3DDC97` on near-black,
until the real app icon (plan §6.7) is drawn. Change the shapes in `icon/generate.py`, rewrite
`icon/dromon.svg` from `python3 packaging/icon/generate.py --svg`, then run
`python3 packaging/icon/generate.py` and commit the outputs (same Pillow version, same bytes).

Fixed identifiers, never to be changed:

- Windows MSI **UpgradeCode** `6C1DF9A7-7F49-4721-A013-EE31BC4D818E` (`windows/dromon.wxs`):
  every Dromon MSI upgrades any earlier one through it.
- macOS **bundle id** `dev.dromon.Dromon` (`macos/Info.plist`): macOS keys permissions and
  preferences on it.

Version numbers come from `[workspace.package].version` in the root `Cargo.toml`. Where a format
can't hold a pre-release suffix, the scripts convert it: `0.1.0-rc.1` is `0.1.0~rc.1` in the
`.deb`, `0.1.0rc.1` in the Arch package, and 0.1.0 in the MSI and in `CFBundleShortVersionString`.
