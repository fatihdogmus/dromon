# Flows

Each folder is a UI flow rendered headless by `scripts/flows.sh` into reviewable artifacts (a
strip of frames, the element tree at the end, a video when ffmpeg is installed): `script.json` is
Rune's script (its `description` says what the flow shows; end it with a mark named `end`), and an
optional `fixture.json` is the fixture it runs on (`docs/fixtures.md`; without one, the built-in
demo). Element keys to script against: `host-<alias>`, `rows/<name>`, `crumb-<n>`, `col-Name`,
`col-Size`, `tb-back`, `tb-forward`, `sheet-input`, `sheet-submit`, `sheet-cancel`, `sheet-no`,
`state-go-up`, `state-retry`, `state-show-log`, `status-conn`, `log-disconnect`, `log-close`.
