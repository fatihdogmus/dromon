# Dromon: product & technical plan

*A small, fast, beautiful SFTP file browser for macOS, Windows and Linux, built in Rust on [Rune](https://github.com/anomalyco/rune).*

Status: planning, v1 scope locked · Last updated: 2026-11-25 · License: MIT

> **Name.** A *dromon* was the Byzantine fast war galley; the word comes from the Greek for "runner". No software product with this name turned up, and `dromon` is free on crates.io. Before public release, run a proper trademark search (USPTO, EUIPO, Turkish Patent Office), check the domains, and claim the GitHub org.

---

## 1. Goals and non-goals

### Goals
1. **Browse a remote machine over SSH** the way you browse a local folder: fast, keyboard-friendly, no friction.
2. **Move files both ways with no ceremony.** Double-click downloads to Downloads. Drag in from Finder or Explorer to upload to the open folder.
3. **Authenticate once per connection.** Use the user's existing SSH setup (config, keys, agent, jump hosts). Ask for a password only when there's no key, keep it in memory only, and never ask again during the session, reconnects included.
4. **Look absolutely beautiful.** A bold, crisp, Linear-like design language that feels native in behaviour on every platform, and shows what Rune can do: springs, GPU-crisp text, materials.
5. **Stay small.** Not Cyberduck. One protocol (SFTP over SSH), one pane, done extremely well.

### Non-goals for v1
- Split or dual panes
- Editing remote files in place (a quick preview exists: Space shows an image, or a text file's start or a log's end, without downloading it)
- Permission (chmod/chown) editing
- Dragging files *out* of the app into Finder or Explorer (needs OS file promises, which Rune doesn't support)
- FTP, WebDAV, S3, cloud storage
- A built-in terminal
- Sync or mirroring
- Pinned or bookmarked remote folders (v1.1)
- Instant "live mode" via inotify (v1.1; v1 uses adaptive polling)
- Windows 10
- Auto-update, code signing, installers (decided at release time)
- Localization (English only in v1, with strings kept in one place for later)

---

## 2. Platforms

| Platform | Minimum | Notes |
|---|---|---|
| macOS | 14 (Sonoma) | Primary development target. Liquid Glass on macOS 26+, sidebar vibrancy below that. Dock badge shows the active transfer count (the Dock has no progress bar). |
| Windows | 11 | Uses the built-in OpenSSH client (`C:\Windows\System32\OpenSSH\ssh.exe`). Taskbar progress bar and badge. Snap layouts kept with the custom title bar. |
| Linux | Wayland and X11. Reference distros: Ubuntu 24.04, Fedora 40 | Rune-drawn window decorations that respect the desktop's button layout. Rune has no native context menus or taskbar badge here, so it falls back to rune-kit menus and no badge. |

---

## 3. User flows

### 3.1 First launch
1. The window opens. The sidebar lists hosts from the user's SSH config. The main area shows a welcome state: "Pick a host, or press ⌘K".
2. Nothing connects automatically.

### 3.2 Connecting to a host with a key
1. Click a host. The file list shows skeleton rows right away, and the path bar reads "Connecting to prod…".
2. ssh authenticates with the key or agent. If the key has a passphrase that the agent doesn't hold, a sheet asks for it once.
3. The listing of the remote home folder (or the last folder used on this host) springs in. The host gets a live mint dot in the **Connected** section.

### 3.3 Connecting to a host with a password
1. Same as above, but a sheet attached to the window shows the server's own prompt text ("deploy@10.0.0.4's password:"), with the line *"Kept in memory until you quit or disconnect. Never saved to disk."*
2. The password is used for this connection and for every silent reconnect until you quit or press Disconnect.
3. A wrong password clears the stored copy and asks again with an inline error.

### 3.4 First-time host (unknown host key)
A sheet shows the host, the key type and the SHA256 fingerprint in mono, with **Trust and connect** / **Cancel**. A *changed* host key is refused (ssh's own safety check) and shown as an error that explains why, with a "Show log" link and the `known_hosts` file and line to inspect.

### 3.5 Download
- Double-click a file, or select it and press ⌘D, or use the context menu's Download. It's saved to Downloads, or the folder set in Settings.
- Name clashes are renamed Chrome-style: `report.pdf` → `report (1).pdf`.
- Folders download recursively and keep their structure.
- The transfers button starts its progress ring. Clicking a finished row reveals the file in Finder or Explorer.

### 3.6 Upload
- Drag files or folders from Finder or Explorer onto the window.
  - Over empty space or file rows: the whole list area gets a mint outline and a floating pill: "Upload 3 items to /var/www".
  - Over a folder row: that folder highlights and the pill names it.
- Or copy files in Finder or Explorer and press ⌘V in the app.
- A name clash opens a sheet: **Replace / Keep both / Skip**, with an "Apply to all" checkbox. Keep both gives `name (1).ext`.

### 3.7 Copy, cut, paste and drag to move
| Action | Result |
|---|---|
| Copy → paste, same host | Server-side copy (`copy-data` extension), falling back to streaming through the app |
| Cut → paste, same host | Instant move (`posix-rename`) |
| Copy → paste, different host | Streamed through the app; shows as a transfer |
| Cut → paste, different host | Copy, verify the size, then delete the source |
| Finder/Explorer copy → paste in the app | Upload |
| Drag rows onto a folder or breadcrumb | Move (same host) |
| Drag rows onto another host in the sidebar | Copy to that host's current folder |
| ⌥-drag (Ctrl-drag on Windows/Linux) | Copy instead of move |

Shortcuts: ⌘C / ⌘X / ⌘V, plus ⌥⌘V ("Move here", as in Finder). Ctrl replaces ⌘ on Windows and Linux.

### 3.8 Quick connect, saving, and adding to the SSH config
1. Press **+** in the sidebar, or ⌘K → "Quick connect…". A popover with **Host**, **Username** and **Port** (placeholder `22`). Pasting `user@host:port` into Host splits it across all three fields.
2. **More** expands: alias name, identity file (with a file picker), jump host.
3. **Connect** adds the host to **Recent**. **Save** adds it to **Saved**.
4. On a saved host: context menu → **Add to SSH config…** opens a sheet with the exact `Host` block in mono, the target file path, and *"A backup will be saved to `~/.ssh/config.dromon-backup-20261125-1342`"*. Confirming appends the block at the end of the file, and the host moves from Saved to SSH Config.

### 3.9 The network drops (Wi-Fi change, sleep)
1. The ssh process exits, or SFTP hits an error. The host's dot turns amber and a slim banner reads "Reconnecting…".
2. Transfers pause and keep their offsets.
3. Silent reconnect with backoff (1s, 2s, 5s, 10s, then every 30s). A stored password is supplied automatically.
4. Once reconnected, the banner animates away, the listing refreshes, and transfers resume from where they stopped.
5. If reconnecting needs input (a 2FA code, a rotated password), the banner offers **Sign in**.

---

## 4. Architecture

### 4.1 Overview

```
┌──────────────────────────── dromon (one binary) ───────────────────────────────┐
│                                                                                 │
│  UI thread (Rune)                         Service thread (tokio runtime)        │
│  ┌──────────────────────────┐  commands   ┌──────────────────────────────────┐  │
│  │ AppState (plain structs) │ ──────────▶ │ ConnectionManager                │  │
│  │ view fns → El tree       │             │  ├─ Connection(host) × N         │  │
│  │ sheets, popovers, ⌘K     │ ◀────────── │  │   ├─ ssh child process        │  │
│  └──────────────────────────┘  events via │  │   ├─ SftpSession (russh-sftp) │  │
│                                AppHandle   │  │   └─ Poller (visible folder)  │  │
│                                + Waker     │  ├─ TransferEngine               │  │
│                                            │  └─ AskpassServer (IPC)          │  │
│                                            └──────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────────┘
        ▲                                                   │ spawns
        │ IPC (Unix socket / named pipe, per-launch token)   ▼
┌───────┴────────────────────┐                ┌──────────────────────────────────┐
│ dromon (askpass mode)      │ ◀── invoked ── │ ssh -s <host> sftp               │
│ prints answer to stdout    │    by ssh      │ (system OpenSSH client)          │
└────────────────────────────┘                └──────────────────────────────────┘
```

- **The UI thread** owns all app state and runs Rune's view functions. Nothing on it blocks.
- **A dedicated service thread** started with `rune::task::serve` runs a multi-threaded tokio runtime. The UI sends it commands over a channel. It sends events back through `AppHandle::update_if` and a `Waker`, so the UI redraws only when something changed.
- **One ssh child process per connected host.** Every listing, operation and transfer for that host is multiplexed over its single SFTP channel. That's why there is exactly one authentication per connection.

### 4.2 Workspace layout

```
dromon/
├─ Cargo.toml                 # workspace; Rune pinned to one `dev` rev + its [patch.crates-io] block
├─ rust-toolchain.toml        # matches Rune's toolchain
├─ crates/
│  ├─ dromon-app/             # the binary: Rune UI and askpass helper mode
│  │  └─ src/{main.rs, state/, views/, theme/, icons/, palette/, settings/}
│  ├─ dromon-core/            # shared types: HostId, RemotePath, Entry, Event, Error
│  ├─ dromon-ssh/             # config discovery, ssh process, askpass IPC server, password store
│  ├─ dromon-sftp/            # session wrapper, extensions (copy-data, posix-rename, limits), listing diff
│  └─ dromon-transfer/        # queue, scheduler, resume, conflict naming, progress aggregation
├─ assets/
│  ├─ fonts/                  # Inter (variable) + Geist Mono, both SIL OFL
│  └─ icons/                  # source SVGs on a 16px grid
├─ themes/dromon/             # theme.json (dark + light), modelled on Rune's themes/opencode
├─ tests/
│  ├─ docker/                 # sshd fixtures (compose file)
│  └─ goldens/                # headless Rune snapshots per screen × theme × scale
└─ docs/
```

### 4.3 SSH layer (`dromon-ssh`)

**Host discovery**
- Config files read:
  - macOS/Linux: `~/.ssh/config`, `/etc/ssh/ssh_config`
  - Windows: `%USERPROFILE%\.ssh\config`, `%ProgramData%\ssh\ssh_config`
- A small parser that reads only `Host` and `Include` lines:
  - Follows `Include` globs, relative to `~/.ssh` as ssh does.
  - Lists only literal aliases and skips patterns containing `*`, `?` or `!`.
  - Ignores `Match` blocks for listing purposes.
- Effective settings (HostName, User, Port, ProxyJump) come from `ssh -G <alias>`, run lazily and cached, so whatever ssh would do, we display.
- Config files are watched for changes and the sidebar updates live.

**Saved and recent hosts**
- `hosts.conf` lives in the app's config folder (Rune `dirs`), in ssh_config syntax, so "Add to SSH config" is a straight copy of a block.
- Recent hosts are stored in app state (last 10; hostname, user and port only, never secrets).
- Saved hosts are connected with explicit options, so the user's global defaults (IdentityFile, agent, etc.) still apply:
  `ssh -o HostName=… -o User=… -o Port=… [-o IdentityFile=…] [-o ProxyJump=…] <alias> …`

**Process launch**
```
ssh -T -x -e none
    -o BatchMode=no
    -o ServerAliveInterval=15 -o ServerAliveCountMax=3
    -o ConnectTimeout=15
    -o ControlMaster=no            # never leave a background master behind; still reuses an existing one
    -s <host> sftp
```
- stdin and stdout are piped and handed to `russh_sftp::client::SftpSession::new(stream)`, which accepts any `AsyncRead + AsyncWrite` stream.
- stderr is captured line by line into the connection log.
- Windows: spawned with `CREATE_NO_WINDOW`. Absolute path to the inbox `ssh.exe`, with a fallback to `PATH`.

**Askpass**
- Environment set on the child process:
  - `SSH_ASKPASS=<path to dromon exe>`
  - `SSH_ASKPASS_REQUIRE=force` (OpenSSH 8.4+)
  - `DISPLAY` set if empty (for older clients' `prefer` logic)
  - `DROMON_ASKPASS_ENDPOINT=<socket or pipe>`
  - `DROMON_ASKPASS_TOKEN=<random 32 bytes, hex>`
- `main()` checks for `DROMON_ASKPASS_ENDPOINT` before any GUI setup. If it's set, the process acts as the helper:
  1. It connects to the endpoint and sends `{token, prompt: argv[1], hint: $SSH_ASKPASS_PROMPT}`.
  2. It blocks until the reply arrives.
  3. It prints the answer plus a newline and exits 0, or exits 1 on cancel.
  It must start in well under 50 ms.
- IPC: a Unix domain socket in a 0700 per-user runtime folder (macOS/Linux), or a named pipe with an ACL for the current user only (Windows). Every request must carry the token issued for that connection.
- How the app answers each kind of prompt:

| Prompt kind (from hint or prompt text) | Behaviour |
|---|---|
| Host key confirmation (`continue connecting (yes/no/[fingerprint])`) | Fingerprint sheet → `yes` / cancel |
| Password (`…password:`) | First attempt of a connection: answer from memory if present; otherwise sheet. A second password prompt in the same attempt means it was wrong: clear memory, sheet with error |
| Key passphrase (`Enter passphrase for key…`) | Sheet; kept in memory like passwords |
| Anything else (keyboard-interactive, 2FA, OTP) | Generic sheet with the server's text; **never** remembered |

**Password store (memory only)**
- `HashMap<HostId, SecretString>` (`secrecy` + `zeroize`), owned by the service thread.
- Cleared on Disconnect, on quit, and when the password is rejected.
- Never logged or serialized, never shown in the connection log, never in crash reports.

**Reconnect**
- A dead connection is detected when the child exits, on an SFTP I/O error, or when ServerAlive times out.
- Backoff: 1, 2, 5, 10, then 30 seconds. An attempt also runs right away when the OS reports a network change or system wake, if Rune exposes it (spike S6); otherwise a cheap `stat .` probe runs on window focus.
- During reconnect the UI keeps the last listing (dimmed, read-only) and queues user actions, or shows them as disabled.

### 4.4 SFTP layer (`dromon-sftp`)
- Thin async wrapper over `russh-sftp` 4.x: `list`, `stat`, `lstat`, `realpath`, `mkdir`, `rename`, `remove_file`, `remove_dir`, `read_at`, `write_at`, `setstat` (mtime only).
- Extensions:
  - `posix-rename@openssh.com` for atomic moves and replaces (already supported by russh-sftp).
  - `limits@openssh.com` to size reads and writes.
  - `copy-data` for server-side copies. russh-sftp doesn't implement it, so we send the extended request ourselves (spike S4), with a streaming fallback when the server doesn't advertise it.
- **Listing** returns `Entry { name, kind (file/dir/symlink→target kind), size, mtime, mode, hidden }`, sorted in the UI.
  - Large folders stream in pages of 512 entries, so the first rows paint immediately.
- **Listing diff**: compare by name, then by `(kind, size, mtime)`. Produces `inserted / removed / changed` sets for animation.
- Symlinks are shown with a small link glyph. Double-clicking a symlinked folder navigates to the resolved path. Symlinked folders are never followed during recursive operations (prevents loops).

### 4.5 Adaptive polling
- Polls **only the folder on screen**, and only while the window is visible (`Tx::window_visible`, plus occlusion on macOS).
- Base interval depends on folder size, doubling while the window is visible but unfocused (so a file your agent writes still shows up while you're in the terminal):

| Entries | Focused | Visible, unfocused |
|---|---|---|
| < 1,000 | 2 s | 4 s |
| 1,000–5,000 | 4 s | 8 s |
| 5,000–20,000 | 10 s | 20 s |
| > 20,000 | manual (⌘R) | manual |

- Also refreshes immediately on navigation, on window focus, after our own operations, and on ⌘R.
- Changes animate:
  - New rows fade and slide in with a mint highlight that decays over about 1.2 s.
  - Removed rows collapse out.
  - Changed rows update their size and date in place (tabular figures, so nothing jiggles).
- **Selection and scroll position never move** because of a poll. If the selected file disappears, the selection clears quietly.
- Backs off further if a poll takes longer than 25% of its interval.

### 4.6 Transfer engine (`dromon-transfer`)
- **Jobs**: Download, Upload, RemoteCopy (same host), CrossHostCopy, CrossHostMove. A folder job expands into file tasks after a scan phase that shows "Preparing… 1,204 files".
- **Concurrency**: up to *N* running file tasks per host (default 3, set in Settings). Each task pipelines several outstanding read and write requests, with chunk size set from `limits@openssh.com` (fallback 32 KiB).
  - Throughput target: at least 80% of `scp` on a gigabit LAN.
- **Atomic and resumable writes**:
  - Downloads are written to `<final name>.dromon-part` in the destination and renamed on completion. The final name is reserved when the transfer starts and re-checked when it finishes.
  - Uploads are written to `.<name>.dromon-part` in the remote folder, then `posix-rename`d into place. Agents and web servers never see half-written files.
  - Resume after reconnect: continue from the part file's verified length.
- **Preserved**: mtime both ways. Mode bits on upload are left to the server's umask in v1.
- **Conflicts**:
  - Download: automatic Chrome-style unique name.
  - Upload or remote paste: sheet (Replace / Keep both / Skip, with Apply to all).
- **States**: queued → preparing → running → (paused) → done | failed | cancelled. Failed tasks keep their error text and offer Retry.
- **Progress**: bytes done and total per job, speed as an exponential moving average over about 3 s, and time remaining shown only after 2 s of data, rounded ("about 2 min").
- **Session history only**, held in memory, with a "Clear finished" button.
- **Quitting during active transfers** asks: "3 transfers in progress. Quit anyway?"

### 4.7 App state (sketch)

```rust
struct App {
    hosts: HostDirectory,            // config + saved + recent, with resolved settings
    connections: Map<HostId, ConnView>,
    windows: Vec<WindowState>,       // one per window; each points at a host + path
    transfers: TransferList,         // mirror of engine state, keyed by job id
    clipboard: Option<RemoteClip>,   // in-app clipboard: host, paths, Copy | Cut
    settings: Settings,
    ui: UiState,                     // open sheet, popover, palette, toasts
}

struct ConnView { status: Status /* Connecting | Ready | Reconnecting | NeedsInput | Failed */,
                  cwd: RemotePath, listing: Listing, history: NavHistory, log: LogBuffer }
```

**Persisted state** uses Rune's `prefs` and `dirs` (window frames, sidebar width, last host, last folder per host, sort, settings). It never includes secrets.

---

## 5. Features by milestone

Every milestone ends with goldens updated and the test suite green on all three platforms.

### M0 · Proofs of concept
Each proof of concept is a small throwaway binary that settles a risk before UI work starts.

| # | Proof of concept | Pass criteria |
|---|---|---|
| S1 | Rune pinned rev and `[patch.crates-io]` building and running `hello` on macOS, Windows 11, Ubuntu (Wayland + X11) | Window opens on all four; headless `--snap` works in CI |
| S2 | `ssh -s host sftp` + `russh-sftp` over child stdio | List `/` on a key host on all three OSes |
| S3 | Askpass round trip through our IPC, **including Windows 11 inbox OpenSSH** | Password, passphrase, host-key and 2FA prompts all reach a test UI. If Windows fails: fall back to an in-process `russh` client on Windows only (decide here) |
| S4 | `copy-data` extended request through russh-sftp's API | Server-side copy of a 1 GB file on OpenSSH 9.x |
| S5 | Rune features: native popup menu (macOS/Windows), clipboard file paste (`ClipboardContent::files`), file drop on the list, bundled fonts via `add_font`, taskbar/Dock badge, `rune-notify` | Each shown working in a scratch app |
| S6 | System wake / network change signal reachable from Rune or the platform crates | A documented approach, or a decision to rely on probing |

### M1 · Core browsing
- Host discovery (config + Include), sidebar with SSH Config / Recent sections
- Connect flow with all auth sheets and host-key sheet; connection log
- File list: virtualized rows (`El::virtual_items`), sortable Name / Size / Modified columns, folders first, natural sort, hidden files shown and dimmed (⌘⇧. toggles)
- Navigation: double-click / Return, ⌘↑, back/forward, breadcrumbs (click and type ⌘L), type-to-select, ⌘F filter
- Empty, loading, permission-denied and folder-gone states
- Persistence of window, sidebar, last host and folder
- Dark theme first pass

### M2 · Transfers
- Transfer engine with resume, conflicts and folder recursion
- Download (double-click, ⌘D, context menu)
- Upload (drop on window or folder row, paste from Finder/Explorer)
- Transfers popover with Downloads and Uploads tabs; toolbar progress ring
- Dock badge (macOS), taskbar progress and badge (Windows), notifications when the app is unfocused and the job took more than 10 s

### M3 · File operations
- New folder (inline row, name field focused), rename (inline, F2), delete (confirmation, recursive, progress toast), copy path
- In-app clipboard: copy, cut, paste, ⌥⌘V; same-host copy-data and cross-host streaming
- Drag to move onto folders and breadcrumbs, drag to copy onto sidebar hosts
- Native context menus (macOS/Windows), rune-kit menu on Linux

### M4 · Resilience, polish, design completion
- Silent reconnect with the in-memory password, transfer pause and resume across reconnects
- Adaptive polling with diff animations
- Connected section, multiple live connections, ⌘N new window, ⌘1–9 host switching
- Quick connect (3 fields + More), Save, Add to SSH config with backup
- ⌘K command palette
- Settings page
- Light theme, accent finalization (3 mint/teal variants → pick), contrast audit, reduce-motion and reduce-transparency paths
- Icon set complete and verified at 1× and 2×

### M5 · Hardening
- Server matrix (see §9), 100k-entry folder performance, 10 GB transfer soak, network-drop chaos tests
- Accessibility pass (AccessKit tree, full keyboard reachability)
- Packaging dry run (unsigned `.app`, `.msi` or `.zip`, AppImage or Flatpak) so release work later is mechanical

---

## 6. Design spec

### 6.1 Principles
1. **Bold and quiet.** Dark-first neutral surfaces. One vivid mint/teal accent, used only for selection, focus, progress, the connected dot and drop targets. Rarity gives the accent its force.
2. **Native behaviour, our own look.** Window controls, context menus, scrolling momentum, text input, shortcuts and accessibility behave natively. Surfaces, type, motion and icons are ours and identical on every platform.
3. **Respond on press.** Every control gives feedback on pointer-down in the same frame. Nothing waits for release to show it noticed.
4. **Everything interruptible.** All motion is springs that retarget from their current value. Nothing locks input while animating.
5. **Crisp or it ships later.** Every icon and hairline is pixel-aligned at 1× and 2×. Numbers use tabular figures. Text is never blurred by a transform.
6. **Confirm only what's destructive.** Delete, Replace and Quit-with-transfers ask. Everything else just happens.

### 6.2 Layout

```
┌────────────────────────────────────────────────────────────────────────────────┐
│ ● ● ●   │  ‹  ›   prod  ›  var  ›  www  ›  site            ⌕ Filter   ⤓◔  ⌘K     │  toolbar 44px
│─────────┼──────────────────────────────────────────────────────────────────────│
│ CONNECTED│  Name ▲                                  Size        Modified        │  header 28px
│ ● prod   │ ──────────────────────────────────────────────────────────────────── │
│ ● nas    │  ▸ assets                                  —         2 min ago        │  rows 28px
│          │  ▸ node_modules                            —         Nov 3, 2026      │
│ SSH CONFIG│    .env                                  1.2 KB      12 min ago       │  (dimmed)
│   staging│    index.html                            14.8 KB     Yesterday        │
│   pi     │    favicon.ico                           4.2 KB      Oct 14, 2026     │
│          │                                                                       │
│ SAVED    │                                                                       │
│   box    │                                                                       │
│ RECENT   │                                                                       │
│   10.0.… │                                                                       │
│      ＋   │  42 items · 3 selected, 18.2 MB                     ● Connected       │  status 24px
└────────────────────────────────────────────────────────────────────────────────┘
  sidebar 240px (resizable 200–320)
```

- **macOS**: the traffic lights sit inside the vibrant sidebar, and the toolbar is one piece with the title bar.
- **Windows 11**: Rune-drawn caption buttons on the right of the toolbar (snap layouts intact). The sidebar is a solid surface (no Mica in Rune).
- **Linux**: Rune-drawn decorations that follow the desktop's button order and side.

### 6.3 Color tokens

The final accent is chosen from three headless-rendered mockups in M4. Ramps are defined in OKLCH and exported as sRGB hex. Light and dark are composed separately, never inverted.

**Accent candidates**

| Variant | Dark theme | Light theme (darker for text and icon contrast) |
|---|---|---|
| A · Spearmint | `#3DDC97` | `#008051` (was `#0E9F6E`; darkened in the M4 contrast audit) |
| B · Teal | `#2DD4BF` | `#007E74` (was `#0F8F85`) |
| C · Seafoam | `#6EE7C8` | `#007F66` (was `#13896F`) |

**Semantic roles**

| Token | Dark | Light | Use |
|---|---|---|---|
| `canvas` | `#0C0E0E` | `#FFFFFF` | File list background |
| `sidebar` | `#101313` (macOS: Sidebar material + tint) | `#F5F7F6` | Sidebar |
| `surface` | `#141717` | `#FAFBFB` | Toolbar, status bar |
| `raised` | `#1A1E1E` | `#FFFFFF` + shadow | Popovers, menus (Linux) |
| `sheet` | `#1F2424` | `#FFFFFF` + shadow | Sheets, dialogs, palette |
| `border-subtle` | `rgba(255,255,255,.06)` | `rgba(0,0,0,.06)` | Row and column separators |
| `border` | `rgba(255,255,255,.09)` | `rgba(0,0,0,.09)` | Control outlines |
| `border-strong` | `rgba(255,255,255,.14)` | `rgba(0,0,0,.14)` | Focused fields |
| `text` | `#ECEFEE` | `#111515` | Primary text |
| `text-2` | `#A9B1AF` | `#545E5C` | Secondary (size, date) |
| `text-3` | `#87908E` | `#717A78` | Tertiary, hidden files, placeholders |
| `hover` | `rgba(255,255,255,.04)` | `rgba(0,0,0,.035)` | Row and button hover |
| `press` | `rgba(255,255,255,.07)` | `rgba(0,0,0,.06)` | Pointer-down |
| `selection` | accent @ 16% | accent @ 12% | Selected rows |
| `focus` | accent, 2px | accent, 2px | Keyboard focus ring |
| `danger` | `#FF6F6F` | `#C23034` | Delete, errors |
| `warning` | `#F5B94A` | `#A66900` | Reconnecting dot, conflicts |

Contrast rules: body text 4.5:1, icons and focus rings 3:1, checked for every pair in both themes and every state in M4. Hidden files at `text-3` must still pass 3:1 against `canvas` and `selection`. The values above are the ones `theme/contrast_tests.rs` passes for all three accents (M4 audit: the `text-2`, `text-3`, `danger`, `warning` and light accent values were nudged in OKLCH lightness only). Known exceptions it lists for a design decision: the idle transfers glyph (quiet on purpose), a danger label on its own pressed danger wash, accent text on its own accent hover wash, and the folder icon on accent fills in light.

### 6.4 Typography
- **Interface font: Inter** (variable, bundled with `add_font`), tabular figures (`tnum`) for every number column, the transfer stats and the status bar.
- **Mono: Geist Mono** for paths in sheets, fingerprints, the quick-connect host field, the SSH config preview and the connection log.

| Role | Size / line height | Weight | Tracking |
|---|---|---|---|
| Caption (section labels, `CONNECTED`) | 11 / 14 | 600 | +0.04em, uppercase |
| Meta (size, date, status bar) | 12 / 16 | 400 | +0.005em |
| Body (rows, menus, fields) | 13 / 18 | 400 (500 selected) | 0 |
| Title (sheet titles) | 15 / 20 | 600 | −0.005em |
| Display (empty-state headline) | 20 / 24 | 600 | −0.015em |

### 6.5 Spacing, radii, elevation
- 4px grid. Common steps: 4, 8, 12, 16, 24, 32.
- Radii: 6 for rows, buttons and fields; 10 for popovers and menus; 12 for sheets, dialogs and the palette.
- Elevation (dark): popovers use a 1px `border` plus a soft shadow (`0 8 24 rgba(0,0,0,.45)`); sheets are deeper (`0 16 48 rgba(0,0,0,.55)`). In light mode the shadows carry the separation and borders drop to `border-subtle`.
- Materials: on macOS, the sidebar uses `SystemMaterial::Sidebar` and popovers may use `Popover`. Never stack one translucent surface on another. Reduce-transparency switches to solid tokens.

### 6.6 Motion
Rune springs are parameterized by duration and bounce (SwiftUI's model).

| Token | Spring | Used for |
|---|---|---|
| `snap` | 0.18 s, bounce 0 | Hover/press fills, selection, checkbox, tab indicator |
| `move` | 0.30 s, bounce 0 | Popover and sheet enter/exit, row insert/remove, sidebar section changes, list ⇄ empty state |
| `settle` | 0.45 s, bounce 0.15 | Only after momentum: a released drag landing, the transfer "done" flourish, the connected dot coming on |

Rules:
- Buttons scale to 0.97 on pointer-down (`snap`). Rows don't scale, they only fill.
- Popovers and menus grow from their trigger (scale 0.96 → 1 plus opacity) and leave the same way. Sheets drop from the toolbar edge. The palette scales from centre.
- Navigating into a folder: the new listing slides 12px in from the right with a cross-fade (`move`); going up reverses it.
- The progress ring sweeps with `move` retargeting, so bursts of progress stay smooth.
- **Done flourish**: the row's progress bar fills, collapses into a mint check (`settle`), and the toolbar ring briefly pulses once. No confetti.
- Reduce motion: every slide and scale becomes a 150 ms opacity cross-fade; `settle` loses its bounce; the poll highlight stays (it's colour, not motion).

### 6.7 Icons
- Our own set, drawn on a 16px grid, each icon in two bodies (`IconPair`, picked by device scale). 1× bodies: 1 pt strokes on half-pixels; 2× bodies: 1.5 pt strokes on quarter-pixels; straight edges pixel-exact at both scales, verified by a pixel test.
- Set: folder, folder-open, file, code, text, image, archive, audio, video, PDF, binary/executable, symlink badge, lock badge (no permission), server, connected dot, download, upload, pause, resume, cancel, retry, check, chevrons, search, plus, more, gear, log.
- **Tint**: icons use `text-2`. Folders use the accent at 70%. File categories get a very desaturated hue (about 35% chroma) to add texture without noise. Selected rows lift icons to `text`.
- App icon (later): a stylised dromon bow or oar bank inside a rounded square, in mint on near-black.

### 6.8 Screens and states

| Screen / state | Spec |
|---|---|
| **Welcome** (no host) | Centred display text "Pick a host to start", sub-text "or press ⌘K", plus a quick-connect button. Subtle, no illustration. |
| **Connecting** | 12 skeleton rows pulsing gently in opacity (no shimmer sweep); path bar shows "Connecting to prod…" with a small spinner. |
| **Auth sheet** | Title "Sign in to prod", the server prompt in `text-2`, password field (autofocus), memory-only note in `text-3`, **Connect** / Cancel. Errors appear inline under the field and the field shakes once (`settle`). |
| **Host key sheet** | Title "First connection to prod", the fingerprint in mono on a `raised` block, a plain-language explanation, **Trust and connect** / Cancel. |
| **Listing** | As in §6.2. Hover = `hover` fill; selection = `selection` fill with a 6px radius inset 4px from the edges; keyboard focus = 2px ring on the focused row. |
| **Drop target** | List area: 1.5px accent inner outline, 6% accent fill, floating pill bottom-centre "Upload 3 items to /var/www". Folder row target: the row lifts with an accent fill and the pill names the folder. Springs in with `snap`, out with `move`. |
| **Empty folder** | "Empty folder" (display), "Drop files here to upload" (`text-2`). |
| **Permission denied** | Lock glyph, "You don't have access to this folder", **Go up** button. |
| **Folder vanished** | Navigate to the nearest existing parent and show a toast: "/var/www/old was removed". |
| **Reconnecting** | Slim banner under the toolbar ("Reconnecting to prod…", amber dot), listing dimmed to 60% and read-only. **Sign in** appears when input is needed. |
| **Transfers popover** | 380 × up to 480px, anchored to the toolbar button. Segmented Downloads / Uploads tabs with counts; rows show icon, name, a 3px progress bar, then "12.4 / 80 MB · 8.1 MB/s · about 9 s" in tabular `text-2`, plus pause/cancel/retry on hover. Footer "Clear finished". Empty tab: "No downloads yet". |
| **Toolbar transfers button** | Arrow glyph inside a 2px progress ring (aggregate bytes across all jobs); a count badge while anything runs. Idle = plain glyph. |
| **Conflict sheet** | "'index.html' already exists in /var/www", sizes and dates of both side by side, **Replace** (danger style) / **Keep both** / **Skip**, "Apply to all 4 conflicts". |
| **Delete confirm** | "Delete 3 items permanently?", list of the first 5 names, "This can't be undone on the server.", **Delete** (danger) / Cancel. |
| **⌘K palette** | 560px wide, top third of the window. Fuzzy search; groups: Hosts, Go to folder…, Actions. Each row shows its shortcut. `move` scale from centre. |
| **Quick connect** | Popover from sidebar **+**: Host (mono), Username, Port; **More** expands alias, identity file, jump host. **Connect** / **Save**. |
| **Add to SSH config** | Sheet: target path, the `Host` block in mono on `raised`, backup note, **Add to config** / Cancel. |
| **Settings** | A page in the main window, in place of the toolbar, list and status bar, the sidebar beside it; opened by ⌘, (Ctrl+,), the palette, the macOS app menu and the sidebar's gear; Back or Escape returns to the folder view as it was. Sections in a list on the left: Appearance (System/Dark/Light, density), Transfers (download folder, transfers at once), Browsing (show hidden files by default, on by default). |
| **Connection log** | Sheet or window per host: mono, timestamped, copy-all button, "Verbose logging" toggle (reconnects with `-v`). |

### 6.9 Accessibility
- Every control has an AccessKit role and label. The file list is exposed as a grid with column headers.
- Fully keyboard-operable, with a visible focus ring everywhere (§6.3).
- Respects reduce motion, reduce transparency and increased contrast (stronger borders, solid surfaces).
- Status and errors always use text or shape as well as colour.

---

## 7. Keyboard shortcuts

⌘ on macOS. Ctrl on Windows and Linux unless noted.

| Action | Shortcut |
|---|---|
| Command palette | ⌘K |
| New window | ⌘N |
| Quick connect | ⌘⇧K |
| Switch to host 1–9 in the sidebar | ⌘1 … ⌘9 |
| Open folder / download file | Return, ⌘↓, double-click |
| Parent folder | ⌘↑ (Alt+↑ on Windows) |
| Back / Forward | ⌘[ / ⌘] (Alt+← / Alt+→ on Windows and Linux) |
| Go to path | ⌘L |
| Filter current folder | ⌘F |
| Search in subfolders | ⌘⇧F |
| Refresh | ⌘R (also F5 on Windows and Linux) |
| Toggle hidden files | ⌘⇧. (Ctrl+H on Linux) |
| Download selection | ⌘D |
| Show transfers | ⌘⇧J |
| New folder | ⌘⇧N |
| Rename | F2 |
| Delete | ⌘⌫ (Delete on Windows and Linux) |
| Copy / Cut / Paste | ⌘C / ⌘X / ⌘V |
| Move here | ⌥⌘V |
| Copy path | ⌥⌘C |
| Select all | ⌘A |
| Settings | ⌘, |
| Close window | ⌘W |

---

## 8. Settings (v1)

| Setting | Default |
|---|---|
| Theme | System |
| Density | Comfortable (28px rows); Compact = 24px |
| Download folder | OS Downloads folder |
| Transfers at once (per host) | 3 |
| Show hidden files by default | **On** |

---

## 9. Testing strategy

- **Unit**: config parser (Include globs, patterns, quoting), conflict naming, listing diff, polling interval logic, prompt classifier, progress maths (EMA, time remaining).
- **Integration** against Docker sshd fixtures (`tests/docker/compose.yml`):
  - OpenSSH latest, with users for key, encrypted key, password and keyboard-interactive 2FA
  - OpenSSH 8.x without `copy-data`
  - A server that's slow and lossy behind `toxiproxy` (disconnects, latency, bandwidth caps)
  - Folders of 100, 10k and 100k entries; a 10 GB file; symlink loops; unreadable folders
- **UI goldens** with Rune's headless mode:
  - `--snap` for every screen and state in §6.8 × {dark, light} × {1×, 2×}
  - `--script` flows: connect → navigate → drop upload → conflict → done
  - Goldens are reviewed when they change; CI fails on an unreviewed diff
- **Crispness checks**: icon goldens at 1× compared pixel-exact; a lint that every hairline and icon origin lands on the device-pixel grid.
- **Performance gates** using Rune's perf counters:
  - Scrolling a 100k-row folder at 120 Hz with zero dropped frames on an M-series Mac
  - First rows of a 10k folder painted under 150 ms after the listing starts arriving
  - Transfer throughput at least 80% of `scp` on a gigabit LAN
- **Manual matrix** before each milestone sign-off:
  - Clients: macOS 14 + 26, Windows 11, Ubuntu 24.04 (GNOME Wayland), Fedora 40 (KDE X11)
  - Servers: Ubuntu, Debian, Alpine, a Synology NAS, a Windows OpenSSH server (`/C:/` paths)

---

## 10. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Windows 11 inbox OpenSSH handles `SSH_ASKPASS` differently | Password hosts broken on Windows | Proof of concept S3 first. Fallback: an in-process `russh` client on Windows only, with the same UI |
| Rune is pre-release (fast churn; issues only, no pull requests) | Breakage on upgrade | Pin one `dev` rev; bump deliberately about monthly behind the golden suite; report bugs as issues with headless repro scripts |
| Linux gaps in Rune (no native context menu, no taskbar badge) | Slightly less native on Linux | rune-kit menu styled to match; skip the badge |
| `russh-sftp` doesn't support `copy-data` | Slow same-host copies | Proof of concept S4: send the raw extended request; streaming fallback |
| Rune text input / IME maturity | Rename and path fields misbehave with non-Latin input | Test Turkish, Japanese and emoji input in M1; report issues early |
| Server diversity (old OpenSSH, BusyBox, Windows sshd) | Odd paths, missing extensions | Feature-detect every extension; server matrix in M5 |
| Very large folders | Jank, memory | Paged listing, virtualized rows, polling switches off above 20k entries |
| Name or trademark conflict found late | Forced rename | Full search before the public release; renaming is mechanical until then |

---

## 11. Later (post-v1 backlog)

- Release: signing and notarization (macOS), MSI/winget (Windows), Flatpak/AppImage (Linux), auto-update
- **Live mode**: `inotifywait -m` over a second ssh session on Linux servers with inotify-tools, for instant updates
- Pinned folders per host
- Drag-out to Finder/Explorer with file promises (needs Rune support or a platform shim)
- Quick Look–style previews: done for images, text and logs (Space); PDF, audio and video still to come
- Windows 10 support
- Localization
- Editing a remote file in a local editor with automatic re-upload on save

---

## 12. Decision log

| Topic | Decision |
|---|---|
| Name | Dromon (pending trademark search before release) |
| License | MIT |
| Stack | Rust + Rune (pinned), tokio, russh-sftp over the system `ssh` |
| Window model | Sidebar of hosts; many live connections; ⌘N for more windows |
| Hosts | SSH Config (read-only) / Saved (ours, editable) / Recent; Connected section on top |
| Quick connect | Separate Host / Username / Port fields + More; paste-splitting |
| Saved hosts storage | Our `hosts.conf` in ssh_config syntax; "Add to SSH config" = preview + backup + append |
| Passwords | Memory only, until quit or Disconnect; silent reconnects reuse them |
| Reconnect | Silent with backoff; transfers pause and resume |
| Panes | Single pane, list view only |
| Download | Double-click → Downloads; Chrome-style renaming |
| Upload | Drop or paste from the OS file manager; Replace / Keep both / Skip |
| Folders | Recursive both ways |
| Operations | New folder, rename, delete (confirm), copy path, copy/cut/paste incl. cross-host, drag to move |
| Freshness | Adaptive polling of the visible folder; animated diffs; live mode later |
| Sorting | Clickable columns, folders first, natural order, remembered globally |
| Hidden files | Shown by default, dimmed; ⌘⇧. toggles |
| Transfers UI | Toolbar ring → popover with Downloads / Uploads tabs; session history; 3 at once |
| Look | Bold, dark-first, Linear-like, with a real light mode; neutral + mint/teal accent |
| Type | Inter + Geist Mono, bundled; tabular figures |
| Density | 28px rows |
| Chrome | Custom single-piece title bar on all platforms |
| Motion | No-bounce springs; bounce only after momentum; reduce-motion respected |
| Context menus | Native on macOS/Windows; rune-kit on Linux |
| Extras | ⌘K palette, per-connection log, settings page |
| Launch | Restore window/sidebar/last host; never auto-connect |

---

## Sources and references
- Rune: https://github.com/anomalyco/rune (README, PLAN.md, ARCHITECTURE.md, examples index; `popup_menu`, `clipboard`, `drag_out`, `file_dialog`, `taskbar`, `system_material` modules)
- russh-sftp: https://github.com/AspectUnk/russh-sftp (`SftpSession::new` over any async stream; supported extensions)
- OpenSSH `SSH_ASKPASS_REQUIRE` (8.4+): https://lists.mindrot.org/pipermail/openssh-bugs/2020-July/021789.html
- OpenSSH `copy-data` extension: https://bugzilla.mindrot.org/show_bug.cgi?id=2948
- Emil Kowalski, design-engineering skills (apple-design, UI cheatsheet): https://github.com/emilkowalski/skill
- Impeccable design skill (color & contrast, native adaptation): https://github.com/pbakaus/impeccable
