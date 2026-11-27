# UI Guide

> Source: `ui.md`
> Purpose: Views and theme tokens, sheets, menus, the palette and toasts, keyboard shortcuts, accessibility, icons, motion, scenes, goldens, headless tooling and flows.

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

The design itself (colours, type, spacing, motion tokens, every screen and state) is the plan,
`.plans/dromon-plan.md` §6, and the shortcut table is §7. This guide is how the code keeps them.
How a view gets its state and runs actions is [architecture.md](architecture.md#pattern-action-state-step-effect).

## Index

- [Standard: Views and Theme Tokens](#standard-views-and-theme-tokens)
- [Reference: Sheets, Popovers, the Palette and Toasts](#reference-sheets-popovers-the-palette-and-toasts)
- [Reference: The Filter and Search in Subfolders](#reference-the-filter-and-search-in-subfolders)
- [Decision: One Model for Native and Drawn Menus](#decision-one-model-for-native-and-drawn-menus)
- [Standard: Keyboard Shortcuts](#standard-keyboard-shortcuts)
- [Decision: Accessibility Is Tested](#decision-accessibility-is-tested)
- [Standard: Icons on the Pixel Grid](#standard-icons-on-the-pixel-grid)
- [Standard: Motion and Reduce Motion](#standard-motion-and-reduce-motion)
- [Reference: Scenes](#reference-scenes)
- [Recipe: Change Something Visible](#recipe-change-something-visible)
- [Reference: Headless Tooling](#reference-headless-tooling)
- [Recipe: Add a Flow](#recipe-add-a-flow)
- [Reference: Script Events and Element Keys](#reference-script-events-and-element-keys)

---

## Standard: Views and Theme Tokens

**Scope:** `crates/dromon-app/src/views/`, `crates/dromon-app/src/theme/`

### Purpose

Keep every colour, size and spring in one place, so the light theme, another accent and the
accessibility variants are one switch away.

### Rules

- A view is a function of `&App` and a `Paint` (`views/mod.rs`) returning an `El`. It reads
  colours from `Paint::t` (`theme::Tokens`) only, never from literals. `Paint::hair` is one
  device pixel, `Paint::scale` picks icon bodies.
- Type comes from `theme/type_.rs`; every style that shows numbers in columns, the status bar
  or transfer stats sets tabular figures.
- Spacing is on the 4 pt grid; radii are 6 (rows, buttons, fields), 10 (popovers, menus),
  12 (sheets, the palette).
- Every element a test, flow or golden needs to find has a stable `.key(...)`. Keys are part of
  the scripting surface ([Script Events and Element Keys](#reference-script-events-and-element-keys)):
  renaming one breaks flows and issue scripts.
- The contrast audit (`theme/contrast_tests.rs`) checks every colour pair the views draw, in
  both themes, all accent candidates, every preference and state. A new token or a new pair of
  existing ones goes into it. `cargo test -p dromon-app --lib contrast -- --nocapture` prints the
  worst pair per rule.
- `--accent a|b|c` renders with another accent candidate; `cargo accents` renders the comparison
  sheets into `target/accents/`.

### Agent checklist

- Are all colours tokens, and is any new pair in the contrast audit?
- Do new numbers use tabular figures?
- Do new interactive elements have keys, roles and names?

## Reference: Sheets, Popovers, the Palette and Toasts

**Scope:** `views/sheets.rs`, `state/sheets.rs`, `views/quick.rs`, `views/transfers.rs`, `views/palette.rs`, `views/toast.rs`

### Use for

Picking the right surface for new UI.

### Reference

| Surface | Behaviour |
| --- | --- |
| Sheet | Drops from the toolbar edge over a veil that makes the window inert. One at a time; prompts that arrive meanwhile wait in order. Escape cancels, Return presses the default button (`default_button`). Sign in, host key, yes/no, connection log, delete, Add to SSH config, conflict, edit conflict (`views/editing.rs`: a file being edited changed on the server), quit. |
| Popover | Grows from its trigger on `raised`, modal: Escape or a press outside closes it. Quick connect, transfers (headed by its Editing section while files are being edited in a local editor, `views/editing.rs`). |
| Palette | ⌘K, 560 pt, a third of the way down; groups Hosts, Go to folder…, Actions. Its phases are `state::palette::Phase`. Actions that can't run now are not listed (never greyed). |
| Toast | Bottom centre, blocks nothing. ssh's notices first, then the app's (delete progress, "folder was removed"). |
| Preview panel | Space on a file row (`views/preview.rs`, `state/preview.rs`): a card over the right 60 % of the list that leaves the rows on its left in reach, so ↑/↓, a click or typing move the cursor and the preview follows. Springs in on `snap`, out on `move`, at once under reduce motion. A dialog, but not a Rune modal (that would make the rows inert): it takes the keyboard on opening and its own `on_key` keeps Tab among its controls; Space or Escape closes it and hands the keyboard back to the rows. While it's open the list's other shortcuts wait (`App::preview_shortcut`). A text shows in the mono face as virtual rows of 16-line blocks (one text for the lines, one for their numbers, hidden from screen readers, inside a region named "Contents of <name>, N lines"), so only the blocks on screen are laid out; lines don't wrap, and the content is as wide as the widest line. A line over 2,000 characters is cut there with " …" (`state::preview::LONG_LINE`), and a note under the header says so beside the one for a text cut to 256 KB. Moving the cursor reads the next file at once, but its text shows only once the cursor has rested 150 ms (`state::preview::REST`, on the poll's clock), so ↓ held lays out none of the texts it passes; Space opening the panel, images and details don't wait. |

- Confirm only what is destructive (delete, Replace, quitting with transfers running); the safe
  choice is the default button.
- What the user types into a prompt lives only in the open sheet's field; it is moved into the
  answer on submit and wiped on cancel ([security.md](security.md#standard-secrets-in-memory)).
- Modal surfaces trap Tab and hand focus back to their opener when they close.

### Agent note

Shortcuts of the list don't reach behind a sheet or the palette (`on_key` in `app_root.rs`
returns early); keep it that way for new modal surfaces.

## Reference: The Filter and Search in Subfolders

**Scope:** `views/toolbar.rs`, `views/list.rs`, `views/search.rs`, `state/search.rs`

### Use for

Knowing what the list area shows while the filter field is open, and which state owns it.

### Reference

| Scope | Opened by | The list area shows |
| --- | --- | --- |
| This folder | ⌘F, the Filter button, the switch | The folder's rows whose names contain the text (`conn.rs`'s `Filter`). |
| Subfolders | ⌘⇧F, the palette's "Search in subfolders…", the switch | Results mode: a status line ("Searching… N folders", then "N results in M folders" and "K folders couldn't be read"), then a grid of Name, Folder (relative to the folder searched, `.` for itself), Size, Modified, in the order found. |

- The scope switch (`tb-scope`, a radiogroup) sits beside the open field. Typing in Subfolders
  starts a search once typing pauses (`search::DEBOUNCE`, on the window's clock: the view's poll
  fires it); a substring of the name, or a glob of the whole name when the text has `*` or `?`.
- Results mode is the list's: the same `list` viewport and `rows` row group (one Tab stop, the
  cursor's row the active descendant), rows keyed `hit-<n>` in the order found, the folder a
  grid cell and in each row's description. The header row is for the eye (results aren't
  sorted), so readers don't meet column headers that do nothing. The status bar counts results.
- While results are on screen the list's actions are theirs (`App::results_action`): the
  selection, Enter and double-click (a folder opens; a file opens its folder, selected), and the
  row menu's Download, Copy path and Properties. What would change the folder view out of sight
  (rename, delete, the clip, new folder) does nothing.
- Escape, clearing the field, switching to This folder, navigating, another host or a
  disconnect end the search; the folder view is back as it was. The count is announced
  politely when a search ends.

## Decision: One Model for Native and Drawn Menus

**Status:** Accepted
**Scope:** `state/menu.rs`, `views/menu.rs`

### Context

Rune has native popup menus on macOS and Windows but not on Linux, and headless runs have none.

### Decision

A menu is one model (`state::menu`: entries, ids, enabled, danger, shortcut chord) shown either
natively through `Tx::popup_menu` or drawn by `views/menu.rs`. `views::menu::native()` is false
in tests and headless runs, so goldens, flows and tests see the drawn menu on every platform.

### Rules and examples

- Add an item to the model once; both paths show it. Item keys are `ctx-<id>`.
- Items that change the server, and Open in Terminal, are disabled while the host isn't
  connected; Paste also needs a clip; Edit also needs a file quick preview reads as text.
- Shortcut hints are chord specs (`mod+c`, `mod+shift+n`, `mod+alt+c`) rendered by
  `widgets::chord`, so one string reads ⌘C on macOS and Ctrl+C elsewhere.

### Consequences

Native placement and native-only behaviour need a real window to check; everything else is
covered headless.

### Agent checklist

- Was the item added to the model, not to one of the two renderers?
- Is its chord spec the same as the key handling in `state/keys.rs`?

## Standard: Keyboard Shortcuts

**Scope:** `state/keys.rs`, `app_root.rs` (`on_key`), `windows.rs`

### Purpose

One pure mapping from key events to meaning, correct on every platform.

### Rules

- `state::keys::shortcut(&KeyEvent, KeyPlatform) -> Option<Shortcut>` is the only place a key
  becomes a meaning. It is pure; test new keys in its `mod tests` for each `KeyPlatform` that
  differs.
- ⌘ on macOS is Ctrl elsewhere: Rune reports either as `Mods::cmd` and `Key::Command(c)`.
  Platform differences are explicit: Delete deletes on Windows and Linux (⌘⌫ on macOS), F5
  refreshes there too, Alt+←/→/↑ navigate on Windows and Linux, Ctrl+H toggles hidden files on
  Linux.
- Shift may arrive applied (`K`, `>`) or not; macOS may report ⌥ letters as their typed form
  (`ç`, `√`). Match both, as the existing arms do.
- A focused text field keeps its typing; the palette opens even from a field. File shortcuts
  never fire while a field has focus.
- ⌘N and ⌘W reach the window first on every platform (`keys_first`, `windows.rs`).
- The README's shortcut table and plan §7 describe the shortcuts; change them with the code.

### Agent checklist

- Is the new key in `shortcut`, with tests on Mac and on Windows or Linux?
- Do menus and palette rows show the same chord?
- Did the README's table follow?

## Decision: Accessibility Is Tested

**Status:** Accepted
**Scope:** `ui_a11y_tests.rs`, `ui_keyboard_tests.rs`, `ui_access_tests.rs`, `state/focus.rs`

### Context

Every control must work with the keyboard and a screen reader (plan §6.9). Regressions here are
invisible in screenshots.

### Decision

Accessibility is checked by tests over every scene, not by review alone.

### Rules and examples

- The audit (`ui_a11y_tests.rs`) reads each scene's AccessKit tree (`Runtime::accessibility_json`)
  and fails on a control without a role or name, two siblings that say the same, an unnamed
  image, a control that does nothing, or one the keyboard can't reach. It also checks the file
  list's grid semantics and that states are said in words or shape, not colour alone.
- Give every interactive element `.role(...)` and `.label(...)` (and `.description(...)` where
  the label isn't enough). Modal panels have role `dialog`.
- Tab order (`ui_keyboard_tests.rs`): Tab and Shift+Tab walk every control of each main state
  once per cycle in reading order, with the focus ring on each; popovers, menus and sheets keep
  Tab inside; Escape returns focus to the opener; arrows move inside composites. A new control
  joins the walk; a new modal surface joins the trap test.
- Screen-reader news without moving focus (a finished transfer, a lost connection, a removed
  folder) goes through `state::focus::Announcer` and the window's live region.
- Reduce motion, increased contrast and reduce transparency are driven headless in
  `ui_access_tests.rs`.

### Consequences

A new scene is audited automatically. A control that can't be named or reached fails CI.

### Agent checklist

- Does every new control have a role, a name and a keyboard path?
- Does the Tab walk or trap test cover the new surface?
- Is any state shown by colour also said in words or shape?

## Standard: Icons on the Pixel Grid

**Scope:** `crates/dromon-app/src/icons/`

### Purpose

Every icon and hairline is crisp at 1× and 2×.

### Rules

- Each icon is an `IconPair` of two SVG bodies on the 16 pt grid: 1× with 1 pt strokes on
  half-pixels, 2× with 1.5 pt strokes on quarter-pixels. Views pick one with
  `IconPair::at(scale)` and place the 16 pt element on whole points.
- `icons/tests.rs` rasterises every icon at both scales and fails on half-covered straight
  edges; draw new icons to pass it.
- `icons_sit_on_whole_points_in_every_scene` fails when an icon's origin isn't on a whole point.
  Icons that follow text of fractional width are listed in `TEXT_FLOWED` by element key; add a
  key there only for an icon that really follows flowed text (Rune rounds those to the device
  grid when it paints).
- Icons are tinted by tokens (`text-2`, the folder colour, `text` when selected), never baked in.

### Agent checklist

- Does a new icon have both bodies, named apart (`…@1x`, `…@2x`)?
- Do both icon tests pass without widening `TEXT_FLOWED` for a misplaced icon?

## Standard: Motion and Reduce Motion

**Scope:** `theme/motion.rs`, every view that animates

### Purpose

All motion is interruptible and respects reduce motion.

### Rules

- Use one of the three springs: `SNAP` (fills, selection), `MOVE` (popovers, sheets, rows in
  and out), `SETTLE` (only after momentum). Bounce only on `SETTLE`, and `motion::settle` drops
  it under reduce motion.
- Buttons scale on pointer-down; rows only fill. Feedback comes on press, not release.
- Reduce motion comes from the OS (`Cx::system_reduced_motion`) or is forced
  (`UiState::access`); `views::view` marks the commit with `Cx::reduced_motion`, and Rune turns
  travel into instant changes and opacity cross-fades. A view that picks a spring itself reads
  `Paint::reduced`.
- A one-shot choreography (the ring's pulse, the download-started animation) is a `Sequence`
  of `Beat`s on a new keyed element per play. Beats are static, so travel whose length is the
  window's goes in a finite `LoopSpec::slide` (in fractions of the element's own size) on a
  carrier around it, read from `Cx::layout` while it plays.
- Increased contrast and reduce transparency switch tokens (`theme::tokens_for`), not views.

### Agent checklist

- Does new motion use a theme spring and retarget from where it is?
- Was it checked under reduce motion (`ui_access_tests.rs`)?

## Reference: Scenes

**Scope:** `scenes.rs`, `scenes_settings.rs`

### Use for

Rendering a named state headless.

### Reference

- A scene is `(name, fn(&mut App))` in `scenes::SCENES`, reached by the same actions and events a
  user and the service would produce, starting from the demo fixture on its clock.
- `SCENES` feeds `--scene NAME`, the accessibility audit, the icon grid test and the goldens:
  `scripts/goldens.sh` takes its list from `dromon --list-scenes`, so a new scene gets a golden
  with no other edit.
- Scene names are fixed: goldens and the README's images are named after them.
- Settings has its own scenes in `scenes_settings.rs`.

### Agent note

Build a scene from actions and events, not by poking fields, so it shows a state the app can
really reach.

## Recipe: Change Something Visible

**Scope:** views, theme, icons, scenes

### Use when

A change alters any pixel of a scene.

### Steps

1. Make the change, then render the scene you changed:
   `cargo demo --scene <name> --snap target/<name>.png --scale 2` and open the PNG.
2. Re-render the goldens of the scenes the change reaches: `cargo goldens <scene>…` (writes
   `tests/goldens/<scene>-<theme>@2x.png`; `cargo goldens --list` names them). A change to
   shared chrome (the toolbar, the sidebar, the status bar, theme tokens, fonts) reaches every
   scene: then `cargo goldens` with no names.
3. Look at every golden that changed in `git status`, at full size, before and after. A change
   you didn't mean is a bug, not noise.
4. `cargo goldens --check <scene>…` must pass for those scenes: it renders into
   `target/goldens/new` and allows ±2 per channel (`GOLDEN_TOLERANCE`), for Mesa rounding. On a
   mismatch it writes the new image and a red diff to `target/goldens/diff/`. CI checks every
   scene.
5. In the pull request, name the goldens that changed and say that you looked at each.

### Done when

The goldens in the commit are the ones you meant, each was looked at, and `cargo goldens --check`
passes. CI uploads `target/goldens` as an artifact when the check fails.

## Reference: Headless Tooling

**Scope:** the `dromon` binary (Rune's tooling, on in debug builds and through the `tooling` feature in release)

### Use for

Looking at the app without a window or a network.

### Reference

| Flag | Output |
| --- | --- |
| `--fixture <file.json>` | Run on a fixture ([fixtures.md](fixtures.md)); `DROMON_MOCK=1` runs on the demo |
| `--scene NAME` | Start from a scene |
| `--snap out.png` | One frame as a PNG |
| `--tree out.json` | The element tree (keys, roles, rects, text) |
| `--script s.json` | Replay pointer, key, paste, focus and file-drop events |
| `--strip out.png` | A strip of frames from a script (`--every N`, `--cols N`) |
| `--video out.mp4` | A video of the script (needs ffmpeg) |
| `--size WxH`, `--scale N`, `--theme dark\|light`, `--duration auto`, `--at T\|mark:NAME` | Frame size, device scale, theme, timing |
| `--accent a\|b\|c` | Another accent candidate (Dromon's own flag) |

- `DROMON_TRACE=1` names each service event on stderr, never its contents.
- `cargo smoke` runs the binary headless; `--window` opens a real window (Xvfb on Linux CI);
  `--connect` connects to the sshd fixture through a real window.
- `cargo repro <issue>` turns a bug report's fixture, script and args blocks into
  `target/repro/<n>/` (`summary.md`, `end.png`, `strip.png`, `tree.json`); nothing from the issue
  runs as shell. Maintainers can comment `/repro` to get the same from CI.

### Agent note

Script events and element keys are in
[Script Events and Element Keys](#reference-script-events-and-element-keys); Rune's own README
("Agent tooling") is the full reference for the flags and the script format.

## Recipe: Add a Flow

**Scope:** `tests/flows/`

### Use when

A user journey (several steps across states) needs a reviewable picture, or a bug fix is best
shown as one.

### Steps

1. Add a folder `tests/flows/<name>/` with `script.json`: Rune's script,
   `{"description": "...", "events": [...]}`. The description says what the flow shows; the
   last event is `{"type": "mark", "name": "end"}`. Address elements by key
   ([Script Events and Element Keys](#reference-script-events-and-element-keys)).
2. Add `fixture.json` beside it only when the built-in demo doesn't have what the flow needs
   ([fixtures.md](fixtures.md)).
3. `cargo flows target/flows <name>` renders `<name>.png` (a strip of frames to the end mark),
   `<name>.tree.json` (the element tree at the end), `<name>.log` (with one line per service
   event) and, with ffmpeg, `<name>.mp4`.
4. Look at the strip; check the tree for the end state.

### Done when

`cargo flows` prints `flow <name>: ok`, and the strip shows the journey. Flows are artifacts to
look at, not assertions: a behaviour that must hold also gets a test ([testing.md](testing.md)).

## Reference: Script Events and Element Keys

**Scope:** `tests/flows/*/script.json`, issue scripts for `cargo repro`, `--script`, `--window-script`

### Use for

Writing a script that drives the app.

### Reference

| Area | Keys |
| --- | --- |
| Hosts and navigation | `host-<alias>` (a recent host's id is `[user@]host:port`), `rows/<name>`, `crumb-<n>`, `col-Name`, `col-Size`, `tb-back`, `tb-forward` |
| Sheets and states | `sheet-input`, `sheet-submit`, `sheet-cancel`, `sheet-no`, `state-go-up`, `state-retry`, `state-show-log`, `status-conn`, `log-disconnect`, `log-close` |
| Transfers | `tb-transfers`, `tr-tab-downloads`, `tr-tab-uploads`, `tr-<id>`, `tr-pause-<id>`, `tr-resume-<id>`, `tr-cancel-<id>`, `tr-retry-<id>`, `tr-resolve-<id>`, `tr-clear`; the conflict sheet's `conflict-replace`, `conflict-keep-both`, `conflict-skip`, `conflict-apply-all` |
| Editing | the popover's section `editing`, rows `edit-<id>` (`edit-title-<id>`, `edit-line-<id>`), `edit-stop-<id>`, `edit-resolve-<id>`, `edit-retry-<id>`; the edit conflict sheet `edit-conflict` with `edit-conflict-replace`, `edit-conflict-discard`, `edit-conflict-keep-both`, `edit-conflict-server-*`, `edit-conflict-local-*`; a failed save's toast `toast-note-<n>-retry` |
| File operations | `new-folder`, `edit-input`, `edit-error-text`, `delete-confirm`, `delete-cancel`, toasts `toast-op-<id>` (`-text`, `-dismiss`), the drop and drag pill `drop-pill` (`-text`) |
| Context menus | `ctx-menu` with items `ctx-<id>` (`ctx-open`, `ctx-edit`, `ctx-download`, `ctx-terminal`, `ctx-copy`, `ctx-cut`, `ctx-paste`, `ctx-copy-path`, `ctx-rename`, `ctx-delete`, `ctx-new-folder`, `ctx-refresh`, `ctx-hidden`); a host's menu `ctx-host-connect`, `ctx-host-disconnect`, `ctx-host-add-to-config`, `ctx-host-edit`, `ctx-host-remove`, `ctx-host-save` |
| Quick connect | `sb-quick-connect`, `welcome-quick-connect`, `quick`, `quick-<field>-input` and `quick-<field>-error` for `host`, `user`, `port`, `alias`, `identity`, `jump`; `quick-more`, `quick-pick-identity`, `quick-save`, `quick-connect`; the Add to SSH config sheet `add-config` (`-block-text`, `-target`, `-backup`, `-confirm`, `-cancel`) |
| Filter and search | `tb-filter` (closed), `tb-filter-input`, `tb-filter-clear`, the scope switch `tb-scope` with `tb-scope-folder` and `tb-scope-subfolders`; results `search-grid`, `rows/hit-<n>`, `search-status-text`, `search-unreadable`, `search-empty`; a result's menu items `ctx-download`, `ctx-copy-path`, `ctx-properties` |
| Palette | `tb-palette`, `palette-input`, `palette-group-hosts`, `palette-group-folders`, `palette-group-actions`, rows `pal-host-<id>`, `pal-go`, `pal-folder-<path>`, `pal-<action>`; `palette-empty` |
| Polling | a refresh's new rows carry `fresh-<n>`; the vanished-folder toast is `toast-note-<n>` (`-text`) |
| Preview | the panel `preview` (Space: `{"type": "key", "name": " "}`), `preview-title`, `preview-meta`, `preview-download`, `preview-close`; its text `preview-lines` (the region; block `b` is `preview-lines/<b>`, its lines `…/lines`, their numbers `…/n`), the notes `preview-cut` and `preview-long`, `preview-image`, `preview-loading`, `preview-note`, `preview-kind-value` |

Events the flows use: `tap` and `double-click` on a `key`, `type` (text), `key`, `paste`,
`mark`, and:

- Right-click: `"type": "secondary-click"`. A row drag:
  `{"type": "drag", "key": "rows/<name>", "dx": 0, "dy": -28}`.
- Files from another app: `{"drag_files": {"paths": [...], "at": [x, y]}}` and
  `{"drop_files": {...}}` (or `"key"` for an element's centre).
- A file picker: answer it with `{"type": "file-dialog", "path": …}`.
- A poll: `{"focus": false}` then `{"focus": true}` (the window regains focus); the fixture's
  `changes` land one step per poll or ⌘R.
- Shortcuts: `{"type": "key", "name": "k", "cmd": true}` (⌘K; Ctrl on Windows and Linux), with
  `"shift": true` as needed. Delete is `Delete`; on macOS ⌘⌫ is `"name": "Backspace", "cmd": true`.
- Pasting: a `paste` event carries text (it splits `user@host:port` in Quick connect's Host).
  Scripts can't paste files, and a script's ⌘V reads the clipboard only through a `paste` event,
  so file pastes are covered by `ui_transfer_tests.rs`.

### Agent note

Keys are part of this surface: a renamed key is a change to every flow and issue script that
uses it. Check `tests/flows/` with `cargo flows` after renaming one.
