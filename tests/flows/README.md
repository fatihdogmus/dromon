# Flows

Each folder is a UI flow rendered headless by `scripts/flows.sh` into reviewable artifacts (a
strip of frames, the element tree at the end, a video when ffmpeg is installed): `script.json` is
Rune's script (its `description` says what the flow shows; end it with a mark named `end`), and an
optional `fixture.json` is the fixture it runs on (`docs/fixtures.md`; without one, the built-in
demo). Element keys to script against: `host-<alias>`, `rows/<name>`, `crumb-<n>`, `col-Name`,
`col-Size`, `tb-back`, `tb-forward`, `sheet-input`, `sheet-submit`, `sheet-cancel`, `sheet-no`,
`state-go-up`, `state-retry`, `state-show-log`, `status-conn`, `log-disconnect`, `log-close`.

Transfers: `tb-transfers` (the toolbar button), `tr-tab-downloads`, `tr-tab-uploads`,
`tr-<id>` (a row), `tr-pause-<id>`, `tr-resume-<id>`, `tr-cancel-<id>`, `tr-retry-<id>`,
`tr-resolve-<id>`, `tr-clear`, and the conflict sheet's `conflict-replace`, `conflict-keep-both`,
`conflict-skip`, `conflict-apply-all`. Files from another app are dragged and dropped with
`{"drag_files": {"paths": [...], "at": [x, y]}}` and `{"drop_files": {...}}` (or `"key"` for an
element's centre; `upload-conflict` drops below the last row). Scripts can't paste files (Rune's
`paste` events carry text, HTML, RTF and images only): ⌘V with files is covered by
`ui_transfer_tests.rs`.

Polling: a poll happens when the window gets focus back, `{"focus": false}` then
`{"focus": true}`; the fixture's `changes` (`docs/fixtures.md`, "Changes elsewhere") land one
step per poll or ⌘R. A refresh's new rows carry a wash `fresh-<n>` (n counts refreshes that
changed rows); the vanished-folder toast is `toast-note-<n>` with `toast-note-<n>-text`.

File operations: `new-folder` (the new folder's row), `edit-input` (its name field, and a
renamed row's), `edit-error-text`, the delete sheet's `delete-confirm` and `delete-cancel`,
toasts `toast-op-<id>` with `toast-op-<id>-text` and `toast-op-<id>-dismiss`, a row drag's
pill (the file drop's: `drop-pill-text`), and the drawn context menu `ctx-menu` with items `ctx-<id>` (`ctx-open`,
`ctx-download`, `ctx-copy`, `ctx-cut`, `ctx-paste`, `ctx-copy-path`, `ctx-rename`, `ctx-delete`,
`ctx-new-folder`, `ctx-refresh`, `ctx-hidden`); right-click with `"type": "secondary-click"`.
Rows drag with `{"type": "drag", "key": "rows/<name>", "dx": 0, "dy": -28}`. ⌘C writes the OS
clipboard; a script's ⌘V reads it only through a `paste` event, so `copy-paste` pastes the
copied path text explicitly. The delete key is Delete here; on macOS it's ⌘⌫ (`"name":
"Backspace", "cmd": true`).

Quick connect and saved hosts: `sb-quick-connect` (the sidebar's +), `welcome-quick-connect`,
the popover `quick` with `quick-<field>-input` and `quick-<field>-error` for `host`, `user`,
`port`, `alias`, `identity`, `jump`, `quick-more`, `quick-pick-identity` (answer its picker with
`{"type": "file-dialog", "path": …}`), `quick-save`, `quick-connect`. A `paste` event into Host
splits `user@host:port`. A host's menu (`secondary-click` on `host-<id>`) has `ctx-host-connect`,
`ctx-host-disconnect`, `ctx-host-add-to-config`, `ctx-host-edit`, `ctx-host-remove`,
`ctx-host-save`; the Add to SSH config sheet `add-config` has `add-config-block-text`,
`add-config-target`, `add-config-backup`, `add-config-confirm`, `add-config-cancel`. A recent
host's id is `[user@]host:port` (`host-deploy@prod.example.com:22`).
