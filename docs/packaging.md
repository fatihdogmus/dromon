# Packaging Guide

> Source: `packaging.md`
> Purpose: How Dromon's installers are made: the `packaging/` tree, the icon, fixed identifiers and version numbers.

## Field contract

This guide uses typed sections with real fields, not title-only labels.

| Section type | Required fields | Use for |
| --- | --- | --- |
| Decision | Status, Scope, Context, Decision, Rules and examples, Consequences, Agent checklist | Durable choices and boundaries |
| Standard | Scope, Purpose, Rules, Agent checklist | Conventions and rules |
| Pattern | Scope, Use when, Pattern, Agent checklist | Repeatable implementation shapes |
| Recipe | Scope, Use when, Steps, Done when | Step-by-step workflows |
| Runbook | Scope, Use when, Procedure, Success signals | Troubleshooting and operations |
| Reference | Scope, Use for, Reference, Agent note | Tables, inventories, commands, and lookup material |

`.github/workflows/release.yml` runs these scripts, and so do `ci.yml`'s package jobs on every
push ([ci.md](ci.md#reference-workflows-and-jobs)). Releases, signing and the security boundary
are [releasing.md](releasing.md); its "Building packages locally" section has the commands.

## Index

- [Reference: The packaging Tree](#reference-the-packaging-tree)
- [Decision: Fixed Identifiers](#decision-fixed-identifiers)
- [Standard: Version Numbers](#standard-version-numbers)
- [Recipe: Change the Icon](#recipe-change-the-icon)
- [Recipe: Check a Package by Hand](#recipe-check-a-package-by-hand)

---

## Reference: The packaging Tree

**Scope:** `packaging/`, `scripts/package-linux.sh`

### Use for

Finding the script or source of an installer.

### Reference

```
packaging/
├─ icon/        dromon.svg (vector source), generate.py (renders the rest; fails if the SVG
│               drifted), dromon-{16…1024}.png, dromon.ico (Windows), dromon.icns (macOS)
├─ linux/       common.sh (version, ELF arch, maintainer, dlopen table check), stage.sh (the
│               /usr tree), build-deb.sh (dpkg-deb; Depends from dpkg-shlibdeps +
│               dlopen-libs.txt + openssh-client), build-arch.sh (makepkg in
│               archlinux:base-devel), verify-packages.sh, dlopen-libs.txt, dromon.desktop
├─ arch/        PKGBUILD (the release's Arch package, from the .deb's files), PKGBUILD.aur
│               (template for a future dromon-bin AUR package)
├─ macos/       Info.plist (the one place the bundle id lives), build-app.sh (universal
│               Dromon.app, ad-hoc signed, its zip and the unsigned DMG), dmg/ (the installer
│               window), verify-dmg.sh
├─ windows/     build-exe.ps1 (dromon.exe, version resource and icon stamped), dromon.wxs
│               (WiX v5 source), build-msi.ps1, verify-msi.ps1
├─ signpath/    dromon-msi.xml (SignPath artifact configuration: exe-in-MSI, then the MSI)
└─ update/      release-key.pub (the minisign key the app's updater trusts, built in)
```

- `scripts/package-linux.sh` (`cargo package-linux`) builds the Linux binary, in an
  `ubuntu:22.04` container by default so it needs only glibc 2.35, and runs `build-deb.sh` and
  `build-arch.sh`; `--verify` installs the results in clean containers.
- `dlopen-libs.txt` lists the libraries the binary loads at run time, with their Debian and Arch
  package names; `dpkg-shlibdeps` can't see them.
- `dromon.desktop`'s `StartupWMClass` is the app id, `dromon` (`rune::set_app_id`).
- The `verify-*` scripts check a package the way a user would (install, run, uninstall). CI's
  package jobs run them; the release workflow does not.
- The updater relies on the identifiers below: the MSI's UpgradeCode makes `msiexec /i` of a new
  release upgrade the installed one, and the macOS bundle id is part of the code requirement a
  new app must meet. It also relies on the release's file names (`assemble.sh`) and on
  `/usr/bin/dromon` and the package name `dromon` to tell a `.deb` or Arch install
  ([security.md](security.md#decision-updates-install-only-what-the-release-key-signed)).

### Agent note

Packaging scripts are repository code: they run in build jobs, never in a job with the
`release` environment's secrets.

## Decision: Fixed Identifiers

**Status:** Accepted
**Scope:** `packaging/windows/dromon.wxs`, `packaging/macos/Info.plist`

### Context

Operating systems key upgrades, permissions and preferences on identifiers. Changing one makes
the next release a different app.

### Decision

These never change:

- Windows MSI **UpgradeCode** `6C1DF9A7-7F49-4721-A013-EE31BC4D818E` (`windows/dromon.wxs`):
  every Dromon MSI upgrades any earlier one through it.
- macOS **bundle id** `dev.dromon.Dromon` (`macos/Info.plist`): macOS keys permissions and
  preferences on it.

### Rules and examples

- Copy these values from the files, never retype them elsewhere.
- A new platform's package gets its own fixed identifier, recorded here.

### Consequences

Users upgrade in place and keep their permissions and settings across releases.

### Agent checklist

- Does the diff touch the UpgradeCode or the bundle id? Then it is wrong.

## Standard: Version Numbers

**Scope:** every package

### Purpose

One version, written once, in every format's own spelling.

### Rules

- The version is `[workspace.package].version` in the root `Cargo.toml`; the release's `plan`
  job bumps it ([releasing.md](releasing.md)). Scripts read it from there.
- Where a format can't hold a pre-release suffix, the scripts convert it: `0.1.0-rc.1` is
  `0.1.0~rc.1` in the `.deb`, `0.1.0rc.1` in the Arch package, and `0.1.0` in the MSI and in
  `CFBundleShortVersionString`.
- Each build script runs the packaged `dromon --version` as its check.

### Agent checklist

- Does a new script read the version from `Cargo.toml` rather than take it as an argument?

## Recipe: Change the Icon

**Scope:** `packaging/icon/`

### Use when

Changing the app icon's shapes or colours.

### Steps

1. Change the shapes in `icon/generate.py`.
2. Rewrite the vector source: `python3 packaging/icon/generate.py --svg`.
3. Render everything else: `python3 packaging/icon/generate.py`.
4. Commit the source and every output together, rendered with the same Pillow version, so the
   bytes are reproducible.

### Done when

`generate.py` runs without reporting drift, and the PNGs, `.ico` and `.icns` in the commit are
its output.

## Recipe: Check a Package by Hand

**Scope:** local machines

### Use when

Changing packaging, or checking a release file before trusting it.

### Steps

1. Linux: `cargo package-linux --verify` (x64 `.deb` and `.pkg.tar.zst` in `target/packages`,
   installed in `debian:12`, `ubuntu:24.04` and `archlinux:latest`).
2. macOS: `packaging/macos/build-app.sh <out>`, then `packaging/macos/verify-dmg.sh <dmg>
   <version> false`.
3. Windows: `packaging/windows/build-exe.ps1`, `build-msi.ps1` (needs WiX v5), then
   `verify-msi.ps1` in an elevated shell.

### Done when

Each verify script finishes and the installed `dromon --version` prints the expected version.
