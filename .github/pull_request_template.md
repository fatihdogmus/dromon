### Issue

Closes #

### Type of change

The label decides the release-notes section; PRs are squash merged, so this PR's **title** becomes the commit and the release-notes line: write it for users ("Upload button: pick files to upload", not "fix toolbar.rs").

- [ ] New feature (label `feature`)
- [ ] Bug fix (label `bug`)
- [ ] Faster or lighter (label `performance`)
- [ ] Refactor, tests, CI or docs: nothing a user would notice (label `skip-changelog`)

### What does this PR do, and why?

What was wrong or missing, what you changed, and why that's the right fix. If you aren't sure why something works, say so: it tells the reviewer where to look. Large, clearly generated descriptions that don't match the diff will be closed.

### How did you verify it?

The commands you ran and what they showed, and where you tried it by hand (macOS / Windows / Linux, real server or `cargo demo`).

```sh
cargo test --workspace     # Docker runs the sshd fixture
cargo lint
cargo goldens --check
```

### Screenshots

For a UI change: which goldens changed (`cargo goldens` re-renders `tests/goldens/`, and the PR's file view shows each image before and after), and that you looked at every one. For something the goldens don't show, a screenshot or a recording.

### Checklist

- [ ] Tests that fail without this change and pass with it (unit, headless UI in `ui_*_tests.rs`, or end-to-end against the sshd fixture)
- [ ] Changed goldens were looked at, not just re-rendered
- [ ] No unrelated changes (renames, reformatting, drive-by fixes)
- [ ] Nothing blocks or does I/O on the UI thread; secrets (passwords, passphrases, codes) are never logged, printed or serialized ([AGENTS.md](https://github.com/fatihdogmus/dromon/blob/main/AGENTS.md))
- [ ] Docs follow the change where they describe it (README's shortcuts, `docs/fixtures.md`, `docs/releasing.md`)
