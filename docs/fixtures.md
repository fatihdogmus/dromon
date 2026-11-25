# Fixtures

A fixture is a JSON file that stands in for the network: the sidebar's hosts, and for each server
where sessions start, how sign-in goes (host keys, passwords, passphrases, codes, questions,
notices) and what its folders hold, errors included. The real app runs on it:

```sh
dromon --fixture my-bug.json                        # a window, on the fixture
dromon --fixture my-bug.json --script s.json --strip strip.png --duration auto --tree tree.json
DROMON_MOCK=1 dromon                                # the built-in demo (crates/dromon-app/assets/fixtures/demo.json)
```

With `--fixture` Dromon starts no service thread, runs no ssh and reads no ssh config: the fixture
is the only file it reads. Every bug that isn't about real networking (the UI, sign-in sheets,
listings, sorting, errors, the connection log) can be reproduced this way, offline and headless,
with Rune's tooling (`--scene`, `--script`, `--snap`, `--strip`, `--tree`, `--video`; see Rune's
README, "Agent tooling"). `--scene NAME` starts that scene from the fixture's hosts (scenes
connect to `prod` and `nas`, so they're meant for the demo; the goldens are the demo's).

`scripts/flows.sh` renders the flows in `tests/flows/` this way, and `scripts/repro.sh` does it
for a bug report (`.github/ISSUE_TEMPLATE/bug.yml`).

An invalid fixture stops Dromon at once with exit status 2 and a message that names where in the
file it is wrong, e.g.

```text
dromon: invalid fixture my-bug.json: servers.prod.tree./var: entry 3: unknown kind "folder" (expected file, dir, symlink or other)
```

Object keys join with `.`, list items count from 1 (`host 2`, `step 1`, `entry 3`), and a field of
a list item follows a `:`. Unknown fields are errors too, so a typo never goes unnoticed.

## A complete example

```json
{
  "version": 1,
  "description": "prod asks to trust its host key, then for a password; its /var/log is locked.",
  "now": "2026-11-25T14:00:00Z",
  "hosts": [
    {"alias": "prod", "source": "ssh-config", "user": "deploy", "hostname": "prod.example.com"},
    {"alias": "lab", "source": "saved", "user": "kbd", "hostname": "10.0.0.7", "port": 2222},
    {"alias": "old-box", "source": "recent", "hostname": "192.0.2.1"}
  ],
  "servers": {
    "prod": {
      "home": "/var/www/site",
      "auth": [
        {"host-key": {"key_type": "ED25519", "fingerprint": "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU"}},
        {"password": {"accept": "correct horse"}}
      ],
      "tree": {
        "/var/www/site": [
          {"name": "assets", "kind": "dir", "mtime": "2026-11-25T13:58:00Z"},
          {"name": "current", "kind": "symlink", "target": "dir"},
          {"name": "broken", "kind": "symlink", "target": null},
          {"name": ".env", "kind": "file", "size": 1204, "mtime": "2026-11-25T13:48:00Z", "mode": "0600"},
          {"name": "index.html", "kind": "file", "size": 14812, "mtime": 1795528800},
          {"name": "deploy.sock", "kind": "other"}
        ],
        "/var/www/site/assets": [
          {"generate": {"pattern": "img-{03}.png", "count": 600, "size": [2000, 90000],
                        "mtime": {"end": "2026-11-25T14:00:00Z", "every": 3600, "jitter": 1800}}}
        ],
        "/var/log": {"error": "permission-denied"},
        "/var/www/site/current": {"error": "connection-lost"}
      }
    },
    "lab": {
      "home": "/home/kbd",
      "auth": [
        {"notice": {"text": "Confirm user presence for key ED25519-SK SHA256:abc"}},
        {"passphrase": {"key_path": "/home/me/.ssh/id_ed25519_sk", "accept": "open sesame"}},
        {"confirm": {"text": "Allow use of key /home/me/.ssh/id_ed25519_sk?"}},
        {"password": {"prompt": "(kbd@10.0.0.7) Password: ", "accept": "pw"}},
        {"code": {"prompt": "(kbd@10.0.0.7) Verification code: ", "accept": "123456"}}
      ],
      "fail": "Connection closed by 10.0.0.7 port 2222"
    }
  }
}
```

Here `old-box` has no server: connecting to it fails as an unknown host would ("Could not resolve
hostname 192.0.2.1").

## Top level

| Field | | |
|---|---|---|
| `version` | required | `1`. Other versions are refused with the version this build reads. |
| `description` | optional | Free text, for people. |
| `now` | optional | The clock: RFC 3339 (`"2026-11-25T14:00:00Z"`, any offset) or epoch seconds. Relative dates ("2 min ago") are measured from it, and the connection log stamps lines with it. Without it, the system clock. Set it, so runs are the same every time. |
| `hosts` | optional | The sidebar's hosts, in order (below). |
| `servers` | optional | By host alias: what each host's server does (below). A server for an alias that isn't in `hosts` is an error. |
| `transfers` | optional | Transfer jobs already in the transfers popover when the app starts, newest first (below, "Transfers"). |

## Hosts

`{"alias": "prod", "source": "ssh-config", "user": "deploy", "hostname": "prod.example.com", "port": 2222}`

| Field | | |
|---|---|---|
| `alias` | required | The name in the sidebar and the element key `host-<alias>`. Unique, no whitespace. |
| `source` | required | The sidebar section: `"ssh-config"`, `"saved"` or `"recent"`. |
| `user`, `hostname`, `port` | optional | Shown as the host's address, and used in prompt texts (`deploy@prod.example.com's password: `). |

A host without a server can't be resolved: connecting fails like an unknown host.

## Servers

| Field | | |
|---|---|---|
| `home` | required | An absolute path: where sessions open (the home folder, or the last folder used). |
| `auth` | optional | Sign-in steps, asked in order on every connection attempt (below). None: it connects at once. |
| `tree` | optional | Folders by absolute path (below). Paths are normalized (`/a//b/` is `/a/b`). |
| `fail` | optional | ssh's last words: the connection fails with this after the auth steps, as a refused or dropped connection would (`"Connection closed by 10.0.0.7 port 22"`, `"ssh: connect to host x port 22: Connection refused"`). |

## Sign-in steps

Each step is an object with one field, its kind. They're asked in order, through the same code
the real service uses to decide what to ask and with which error, so retries look exactly as they
do for real.

| Step | The sheet | Right answer | Wrong answer | Cancel |
|---|---|---|---|---|
| `{"host-key": {"key_type": "ED25519", "fingerprint": "SHA256:…"}}` | "First connection to …" with the fingerprint | Trust and connect (`yes`, or the fingerprint). Remembered: later attempts don't ask (as `known_hosts`) | `no` fails: "Host key verification failed." | fails: "Host key verification failed." |
| `{"password": {"prompt"?: "…", "accept": "…"}}` | Sign in, masked, "kept in memory" | `accept` | asked again with "That password didn't work. Try again."; the 3rd wrong answer fails: "Permission denied (publickey,password)." | fails: "Permission denied (publickey,password)." |
| `{"passphrase": {"key_path": "…", "accept": "…"}}` | Sign in, "Enter passphrase for key '…'" | `accept` (may be `""`) | asked again ("Bad passphrase, try again for …") with "That passphrase didn't work. Try again."; 3 wrong fail: "Permission denied (publickey)." | fails: "Permission denied (publickey)." |
| `{"code": {"prompt"?: "…", "accept": "…"}}` | Sign in, masked, never remembered (keyboard-interactive, 2FA) | `accept` | the round starts over: back to the nearest password step before it, asked with "That didn't work. Check your password and code, then try again."; without one, the code is asked again; 3 wrong fail: "Permission denied (keyboard-interactive)." | fails: "Permission denied (keyboard-interactive)." |
| `{"confirm": {"text": "…"}}` | "… asks", Yes / No | Yes | No fails: "Permission denied (publickey)." | fails the same |
| `{"notice": {"text": "…"}}` | A toast; nothing to answer | | | |

- Default prompts: `"<user>@<hostname>'s password: "` and `"(<user>@<hostname>) Verification code: "`
  (`user` defaults to `user`, `hostname` to the alias).
- Passwords and passphrases are remembered in memory as the real service does: a connection that
  fails for another reason (`fail`) answers them from memory on the next try ("answered from
  memory" in the log); a refusal forgets the password; Disconnect forgets everything.
- A notice stays up while the steps after it wait, and goes when the connection settles.
- Answers are test data, but they're treated like real secrets: they never reach the log, the
  trace (`DROMON_TRACE=1`) or `--tree` (masked fields have no text there).

The status bar and sidebar go Connecting → Signing in (a prompt is open) → Connecting → Connected,
and the connection log gets the lines the real service writes ("Running ssh …", "ssh asks “…”",
"Answered ssh's prompt.", "Permission denied, please try again.", "SFTP session started.",
"Connected.", "Couldn't connect: …", "Disconnected.").

## Folders

`tree` maps absolute paths to a list of entries, or to an error:

```json
"/var/www": [ {entry}, {entry}, {"generate": {…}} ],
"/var/log": {"error": "permission-denied"}
```

Errors: `"permission-denied"` and `"not-found"` show the folder's error state;
`"connection-lost"` drops the connection: it shows Reconnecting and the log says when it would
retry, then it stays that way (a fixture has no clock to retry on).

Listing a path:

1. A listed path gives its entries (in order; the app sorts them) or its error.
2. A path on the way to a listed folder or to `home` lists just those subfolders (no dates), so
   breadcrumbs and Go up always work: with only `/srv/data` listed, `/` lists `srv` (and the way
   home).
3. A folder (or a symlink to one) that a listing names but that isn't listed itself is empty. A
   symlinked folder doesn't list its target's entries: list its path too.
4. Anything else doesn't exist ("not found").

Listings arrive as the real service streams them: pages of 512 entries, then a last empty page.

### Entries

`{"name": "index.html", "kind": "file", "size": 14812, "mtime": "2026-11-24T18:00:00Z", "mode": "0644"}`

| Field | | |
|---|---|---|
| `name` | required | A file name: no `/`, not `.` or `..`, unique in its folder. Names starting with `.` are hidden files. |
| `kind` | required | `"file"`, `"dir"`, `"symlink"` or `"other"` (sockets, devices). |
| `target` | symlinks | What the link points at: `"file"`, `"dir"`, `"other"`, or `null` (or absent) for a dangling link. A link to a dir opens like a folder. |
| `size` | optional | Bytes. Default 4096 for folders, else 0. |
| `mtime` | optional | RFC 3339 or epoch seconds. None: no date shown. |
| `mode` | optional | A number (`33188`) or an octal string (`"0644"`, `"100644"`). Bare permissions get the kind's type bits. Default: 0755 folders, 0644 files, 0777 links. |

### Generators

Many similar entries from one line, the same on every run:

```json
{"generate": {"pattern": "IMG_{05}.jpg", "count": 10000, "size": [1500000, 6299999],
              "mtime": {"end": "2026-11-25T14:00:00Z", "every": 3120, "jitter": 2340}}}
```

| Field | | |
|---|---|---|
| `pattern` | required | The name, with one `{}` (the number) or `{0N}` (zero-padded to N digits). |
| `count` | required | How many (at most 1,000,000). |
| `start` | optional | The first number (default 1). |
| `kind` | optional | Default `"file"`. |
| `size` | optional | Bytes, or `[min, max]`: drawn for each entry. |
| `mtime` | optional | A date, or `{"end", "every", "jitter"?}`: entry `i` (from 0) of `count` is `every × (count − i)` seconds before `end`, and up to `jitter` seconds earlier, in whole minutes. |
| `mode` | optional | As for entries. |
| `seed` | optional | Random draws come from this string; default: the folder's path (`<path>#<n>` for its n-th generator after the first). |

## Transfers

Downloads (⌘D, double-click a file) and uploads (files dropped on the list or a folder row, ⌘V
with files copied in Finder or Explorer) run on the fixture through the real transfer queue
(`dromon_transfer::queue`) with fake, instant I/O, the same on every run:

- A job starts when its host is connected; until then it says "Waiting for connection".
- A download takes a free Chrome-style name (`index (1).html`) when it would land on a name an
  earlier download of the session took. A folder download walks the folder tree as listed here;
  links to folders inside it and special files are left out (the job lists them as skipped).
- An upload asks the server's tree whether its name exists. If it does, the conflict sheet asks:
  Replace, Keep both (a free name like `index (1).html`) or Skip, with "Apply to all". Each
  uploaded path counts as one file of 1,200,000 bytes (nothing on your disk is read; a folder is
  one file too). An upload into a folder that doesn't exist fails with why.
- Files are done the moment they start: jobs finish at once, so the ring never shows on its own.
- A finished upload is added to its folder: the folder on screen lists again at once (as the
  real app's polling would notice), and later listings show it.
- Transfers and file operations (new folder, rename, delete) change the same folders: an upload
  into a new folder lands there, a deleted folder can't be downloaded, a renamed one downloads
  under its new name, and a delete takes uploaded files with it.

`transfers` adds jobs the session starts with, for pictures of states the instant engine never
holds still in (a transfer half done, paused, failed). They stay listed after the session's own
jobs; their buttons move them along the task state machine (pause, resume, cancel, retry,
answering a clash) and Clear finished removes the finished ones, but they never progress.

```json
"transfers": [
  {"kind": "download", "host": "prod", "title": "releases", "state": "running", "done": 12400000, "total": 80000000,
   "files": [14, 128], "speed": 8100000, "eta": 9},
  {"kind": "download", "host": "prod", "title": "node_modules", "state": "preparing", "found": 1204},
  {"kind": "download", "host": "nas", "title": "photos", "state": "running", "done": 410000000, "total": 1020000000, "waiting": true},
  {"kind": "upload", "host": "prod", "title": "2 items", "state": "preparing",
   "conflict": {"name": "index.html", "folder": "/var/www/site", "more": 1}},
  {"kind": "download", "host": "prod", "title": "syslog", "state": "failed", "error": "1 file failed",
   "errors": ["syslog: You don't have access to /var/log/syslog"]},
  {"kind": "upload", "host": "prod", "title": "robots.txt", "state": "done", "done": 68, "total": 68,
   "reveal": "/var/www/site/robots.txt"}
]
```

| Field | | |
|---|---|---|
| `kind` | required | `"download"` (the Downloads tab) or `"upload"` (Uploads). |
| `host` | required | A host alias from `hosts`. |
| `title` | required | The row's name: `"report.pdf"`, `"photos"`, `"3 items"`. |
| `state` | required | `"queued"`, `"preparing"`, `"running"`, `"paused"`, `"done"`, `"failed"` or `"cancelled"`. |
| `found` | preparing | Files found so far ("Preparing… 1,204 files"). Default 0. |
| `error` | failed, required | Why it failed ("Failed: …"). |
| `done`, `total` | optional | Bytes copied and in all (`done` at most `total`). Default 0: the ring and bars sum them. |
| `files` | optional | `[done, total]` files. |
| `speed` | optional | Bytes per second, shown while running ("8.1 MB/s"). |
| `eta` | optional | Seconds left, shown while running ("about 9 s"). |
| `waiting` | optional | `true`: paused because the connection is away ("Waiting for connection"). |
| `conflict` | optional | `{"name", "folder", "dir"?, "more"?}`: the job waits on a name clash (`dir`: the existing item is a folder; `more`: how many clashes follow, for "Apply to all N conflicts"). The conflict sheet asks about the oldest job with one. |
| `reveal` | optional | Where a click on the finished row goes: a local path (downloads; the file manager is never opened from headless runs) or an absolute server path (uploads). |
| `errors`, `skipped` | optional | Lines for the row's tooltip ("a.txt: Permission denied", "cache: a link to a folder"). |

Ids are given in order, newest first, above any id the session's own jobs get.

## The built-in demo

`crates/dromon-app/assets/fixtures/demo.json` is compiled into Dromon: nine hosts that connect at
once, a web site on `prod` (dotfiles, symlinked folders, releases, an empty `uploads`, a denied
`/root`), a NAS, and `/srv/media/photos` with 10,000 photos (one generator). Scenes, goldens
(`scripts/goldens.sh`), the in-crate tests and `DROMON_MOCK=1` run on it; flows and repros without
a fixture of their own do too. Changing it changes the goldens.
