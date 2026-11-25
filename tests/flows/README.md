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
