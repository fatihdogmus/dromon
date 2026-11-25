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

## The built-in demo

`crates/dromon-app/assets/fixtures/demo.json` is compiled into Dromon: nine hosts that connect at
once, a web site on `prod` (dotfiles, symlinked folders, releases, an empty `uploads`, a denied
`/root`), a NAS, and `/srv/media/photos` with 10,000 photos (one generator). Scenes, goldens
(`scripts/goldens.sh`), the in-crate tests and `DROMON_MOCK=1` run on it; flows and repros without
a fixture of their own do too. Changing it changes the goldens.
