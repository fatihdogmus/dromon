<div align="center">

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/svg/dromon-lockup-on-dark.svg">
    <img alt="Dromon" src="assets/brand/svg/dromon-lockup-on-light.svg" width="360">
  </picture>
</p>

**Browse a remote machine over SSH the way you browse a local folder.**

A small, fast, beautiful SFTP file browser for macOS, Windows and Linux.<br>
Native-feeling, GPU-rendered, and built around the SSH you already use.

[![CI](https://github.com/fatihdogmus/dromon/actions/workflows/ci.yml/badge.svg)](https://github.com/fatihdogmus/dromon/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-3DDC97.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-0D211C.svg)

<picture>
  <source media="(prefers-color-scheme: light)" srcset="tests/goldens/light-light@2x.png">
  <img alt="Dromon showing a remote web site's folder: a sidebar of SSH hosts, breadcrumbs, and a crisp file list" src="tests/goldens/listing-dark@2x.png" width="860">
</picture>

[**Install**](#install) · [**Features**](#features) · [**Shortcuts**](#shortcuts)

</div>

---

> *A dromon was the Byzantine fast war galley; the word comes from the Greek for "runner".*

Dromon does one thing and does it extremely well: **SFTP over the SSH you already have**, in one clean pane. Open it, pick a host from your SSH config, and you're in. No accounts, no bookmarks to re-create, no second copy of your keys.

<a name="install"></a>

## 📦 Install

**Quick install**

```sh
# macOS (Apple silicon and Intel) and Linux (Debian/Ubuntu, Arch; x86_64)
curl -fsSL https://raw.githubusercontent.com/fatihdogmus/dromon/main/install.sh | sh
```

```powershell
# Windows
irm https://raw.githubusercontent.com/fatihdogmus/dromon/main/install.ps1 | iex
```

Each one finds the latest release, downloads the right package, checks its SHA-256 against the release's `SHA256SUMS` and installs it silently; set `DROMON_VERSION` (for example `0.1.0`) to pin a version.

On macOS, Homebrew works too, and `brew upgrade` keeps Dromon current:

```sh
brew install --cask fatihdogmus/tap/dromon
```

Or download a file yourself from the [Releases page](https://github.com/fatihdogmus/dromon/releases):

| Platform | File | Notes |
|---|---|---|
| macOS 14+, Apple silicon and Intel | `Dromon-<version>-macos-universal.dmg` | Drag Dromon to Applications |
| Windows 11 (x64; Arm runs it emulated) | `Dromon-<version>-windows-x64.msi` | If SmartScreen asks: **More info → Run anyway** |
| Debian 12+, Ubuntu 22.04+ (x86_64) | `Dromon-<version>-linux-x64.deb` | `sudo apt install ./Dromon-….deb` (pulls in `openssh-client`) |
| Arch Linux (x86_64) | `Dromon-<version>-linux-x64.pkg.tar.zst` | `sudo pacman -U Dromon-….pkg.tar.zst` |

Dromon uses your system `ssh` (OpenSSH 8.4 or newer; on Windows, the built-in OpenSSH client). On Arm Linux, [build it from source](#source).

<details>
<summary><b>Verifying a download</b></summary>

Every file has a build-provenance attestation from GitHub Actions, and the checksums are signed with the release key in [`KEYS`](KEYS):

```sh
gh attestation verify Dromon-<version>-linux-x64.deb --repo fatihdogmus/dromon

gpg --import KEYS          # 61AA 2003 0B81 70BE 3527  E4BA 1E54 8E51 E3AD 8C38
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
```

`SHA256SUMS.minisig` is the checksums' [minisign](https://jedisct1.github.io/minisign/) signature by the key Dromon's updater trusts, [`crates/dromon-app/assets/release-key.pub`](crates/dromon-app/assets/release-key.pub) (key ID `489CB3C37A12EC48`): `minisign -V -p crates/dromon-app/assets/release-key.pub -m SHA256SUMS -x SHA256SUMS.minisig`.

</details>

<a name="source"></a>

<details>
<summary><b>Build from source</b></summary>

```sh
git clone https://github.com/fatihdogmus/dromon && cd dromon
cargo run --release -p dromon-app      # the toolchain is pinned in rust-toolchain.toml
```

Linux needs `libxkbcommon-dev` and `libwayland-dev`. Want to look around without a server? `cargo demo` opens a built-in demo.

</details>

## ✨ Why Dromon

- **Your SSH setup, as is.** Hosts come straight from `~/.ssh/config`, `Include`s and all, and every connection runs through your own `ssh`: keys, agent, `ProxyJump`, `Match` blocks. Edit your config and the sidebar updates on its own.
- **Sign in once.** Passwords, key passphrases and 2FA codes are asked in a native sheet, kept **in memory only**, and reused when the connection comes back. Never written to disk, never logged.
- **Transfers that finish.** Drag files in from Finder or Explorer, or press ⌘D to download. Interrupted transfers resume where they stopped, copies on the same server happen server-side in seconds, and a move between servers deletes the original only after the copy checks out.
- **Survives your Wi-Fi.** Close the lid, switch networks: Dromon notices, reconnects in the background and puts you back exactly where you were.
- **Live.** The folder on screen keeps itself up to date. New files fade in, removed ones fold away, and your selection never jumps.
- **Fast with big folders.** Listings stream in and rows are virtualized: a 10,000-file folder shows its first rows in about 15 ms, and 100,000 files scroll smoothly.

<table>
  <tr>
    <td width="50%"><img src="tests/goldens/transfers-dark@2x.png" alt="The transfers popover: downloads with progress bars, speed and time left, a paused and a failed job"></td>
    <td width="50%"><img src="tests/goldens/drop-list-dark@2x.png" alt="Files dragged over the list: a mint outline and an 'Upload 3 items to /var/www/site' pill"></td>
  </tr>
  <tr>
    <td align="center"><sub>Every transfer in one place: progress, speed, time left, resume.</sub></td>
    <td align="center"><sub>Drop files from Finder or Explorer to upload them.</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="tests/goldens/poll-added-dark@2x.png" alt="Two new files fading in with a mint highlight while the selection stays"></td>
    <td width="50%"><img src="tests/goldens/palette-filtered-dark@2x.png" alt="The ⌘K palette filtered by 'st': a host and actions, each with its shortcut"></td>
  </tr>
  <tr>
    <td align="center"><sub>What changes on the server shows up by itself.</sub></td>
    <td align="center"><sub>⌘K: hosts, folders and every action, by a few letters.</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="tests/goldens/transfer-conflict-dark@2x.png" alt="Name clash sheet: the existing and incoming file's sizes and dates side by side, Replace, Skip, Keep both"></td>
    <td width="50%"><img src="tests/goldens/host-key-dark@2x.png" alt="First connection: the server's key fingerprint, with Trust and connect"></td>
  </tr>
  <tr>
    <td align="center"><sub>Name clashes show both files before you choose.</sub></td>
    <td align="center"><sub>First connection: check the fingerprint, trust it once.</sub></td>
  </tr>
</table>

<a name="features"></a>

## 🧰 Everything you need, nothing you don't

**Hosts.** Your SSH config, plus quick connect: paste `user@host:port` and go. Save the ones you use, find recent ones in the sidebar, and add any of them to your SSH config in one step, with a backup of the file.

**Signing in.** Host-key checks, passwords, key passphrases, keyboard-interactive 2FA and jump hosts, all in native sheets. Each host has a connection log with a *Verbose logging* switch for the day something goes wrong.

**Files.** Browse, sort, filter, search by name through every subfolder, new folder, rename, delete, copy path. Copy, cut and paste work across hosts and windows, and you can drag rows onto a folder, a breadcrumb or another host. *Open in Terminal* opens your own terminal app already ssh'd into the folder (pick the app under Settings → General). Right-click for everything else.

**Edit in your own editor.** ⌘E on a text file opens it in the app your system uses for it, and every save there goes back to the server. If someone changed the file on the server in the meantime, Dromon asks first: replace theirs, keep both, or take theirs. The transfers popover lists what you're editing, with *Stop editing*; the local copies stay private and are removed when you stop, disconnect or quit.

**Transfers.** Downloads and uploads of files and whole folders, with resume, Chrome-style `name (1)` renaming or a Replace / Keep both / Skip choice on clashes. Progress in the toolbar, a badge in the Dock or taskbar, and a notification when a long job finishes while you're elsewhere.

**Made to fit in.** Several windows, light and dark themes, compact or comfortable rows, your own download folder and number of parallel transfers. Respects reduce motion and increased contrast, and every control works with the keyboard and screen readers.

**Updates.** Dromon tells you when a new version is out. On macOS (the DMG) and Windows it installs it for you after your transfers are done, checking that the download is the release the project signed first; a Homebrew, `.deb` or Arch install gets the command to run instead. Settings › General has *Check for updates automatically* and *Check now*.

**Privacy.** Dromon talks only to the SSH servers you choose, through your own `ssh`, and to GitHub to check for updates: one anonymous request at launch and once a day (turn it off in Settings › General), nothing about you or your servers, no telemetry.

<a name="shortcuts"></a>

## ⌨️ Made for the keyboard

<table>
  <tr><td><kbd>⌘K</kbd></td><td>Command palette: hosts, folders, actions</td><td><kbd>⌘D</kbd> · <kbd>⌘U</kbd></td><td>Download the selection · upload files</td></tr>
  <tr><td><kbd>⌘L</kbd></td><td>Go to a path</td><td><kbd>⌘C</kbd> <kbd>⌘X</kbd> <kbd>⌘V</kbd></td><td>Copy, cut, paste, also between hosts</td></tr>
  <tr><td><kbd>⌘F</kbd> · <kbd>⌘⇧F</kbd></td><td>Filter the folder · search its subfolders</td><td><kbd>⌥⌘V</kbd></td><td>Move here</td></tr>
  <tr><td><kbd>⌘↑</kbd> <kbd>⌘[</kbd> <kbd>⌘]</kbd></td><td>Up, back, forward</td><td><kbd>⌘⇧N</kbd> · <kbd>F2</kbd></td><td>New folder · rename</td></tr>
  <tr><td><kbd>⌘1</kbd>…<kbd>⌘9</kbd></td><td>Switch host</td><td><kbd>⌘⌫</kbd></td><td>Delete</td></tr>
  <tr><td><kbd>⌘⇧K</kbd></td><td>Quick connect</td><td><kbd>⌘⇧J</kbd></td><td>Show transfers</td></tr>
  <tr><td><kbd>⌘⇧.</kbd></td><td>Show hidden files</td><td><kbd>⌘N</kbd> · <kbd>⌘,</kbd></td><td>New window · Settings</td></tr>
  <tr><td><kbd>Space</kbd></td><td>Quick preview: an image, a text file, a log's end</td><td><kbd>⌘⇧T</kbd></td><td>Open in Terminal: your terminal app, signed in, in this folder</td></tr>
  <tr><td><kbd>⌘E</kbd></td><td>Edit a text file in your editor; each save uploads it</td><td></td><td></td></tr>
</table>

On Windows and Linux, <kbd>Ctrl</kbd> takes the place of <kbd>⌘</kbd>; Delete deletes and <kbd>F5</kbd> refreshes. Or just start typing a file's name to jump to it.

## 🐛 Found a bug?

In Dromon, choose **Help › Report a Bug…** (on Windows and Linux: <kbd>Ctrl+K</kbd> → "Report a bug…"), or **Report** on an error. Dromon shows you the whole report first, then opens a new GitHub issue in your browser with it filled in; you add what happened and what you expected, and can edit anything before you submit. Nothing is sent until you do. No GitHub account? **Copy report** puts the same text on your clipboard.

The report says which Dromon you run and how it was installed, your OS, display and accessibility settings, what kind of screen was up, the error you reported from, and the last things you did and Dromon heard, by kind only. The names of your hosts, users, addresses, folders and files are replaced (`host-1`, `<dir>/file-2.txt`), and it never holds passwords or what your files contain.

Can you make it happen again? Choose **Record Steps…** (on the report, in the Help menu, or <kbd>Ctrl+K</kbd> → "Record steps for a bug report…"), do what makes the bug show, and press **Stop** on the "Recording" pill at the bottom of the sidebar. The report then also holds your steps and what your servers answered (folder listings, errors, which sign-in questions were asked), with every name replaced (`dir-3`, `file-7.log`) and sizes, dates and permissions kept, so the bug can be replayed without your servers. What you type to sign in is never recorded. You see all of it before anything is sent; if it's too long for the link, Open GitHub copies the rest for you to paste into the form.

Contributors: the [repro form](https://github.com/fatihdogmus/dromon/issues/new?template=repro.yml) takes a fixture and a script that `cargo repro <issue>` replays headless (see [`docs/fixtures.md`](docs/fixtures.md)).

## 🛠️ Contributing

```sh
cargo dev                 # the app with live editing: UI changes appear as you save
cargo demo                # the app on a built-in demo, no servers needed
cargo test --workspace    # unit, headless UI and end-to-end tests (Docker runs a real sshd)
cargo lint                # clippy, as CI runs it
cargo goldens             # re-render the screenshots above; they're also the visual tests
cargo flows               # render user flows to image strips in target/flows
```

`cargo xtask` lists every command.

How the code is organised, how to verify a change and how to triage an issue: [`AGENTS.md`](AGENTS.md). How releases are built and signed: [`docs/releasing.md`](docs/releasing.md).

## 🙏 Credits

Built in Rust on [Rune](https://github.com/anomalyco/rune) by Anomaly and [russh-sftp](https://github.com/AspectUnk/russh-sftp). Set in [Inter](https://rsms.me/inter/) and [Geist Mono](https://vercel.com/font) (both SIL Open Font License).

## License

[MIT](LICENSE) © Muhammed Fatih Doğmuş
