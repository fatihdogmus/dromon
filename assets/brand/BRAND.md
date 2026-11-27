# Dromon brand assets

The Dromon mark is a Byzantine galley under full sail, with its pointed ram, two triangular (lateen) sails, a curled stern post, shields along the rail and a bank of oars in the water. Everything here is generated from one script, `tools/build_logo.py`, so never hand-edit the outputs. Change the script and rebuild.

## Instructions for the coding agent

Put this whole folder in the repo at `assets/brand/` and wire it up as below. Don't redraw, recolour or crop any asset. If something needs a size or format that isn't here, add it to `tools/build_logo.py` and rebuild.

### 1. App icon per platform

| Platform | File | How to use it |
|---|---|---|
| macOS | `macos/Dromon.icns` | Copy into the app bundle at `Contents/Resources/Dromon.icns` and set `CFBundleIconFile = Dromon` in `Info.plist`. With `cargo-bundle` or `cargo-packager`, point their `icon` setting at this file. On a Mac you can rebuild the `.icns` from `macos/Dromon.iconset/` with `iconutil -c icns macos/Dromon.iconset`. |
| Windows | `windows/dromon.ico` (16–256 px) | Embed it in the executable from `build.rs` using the `winresource` crate: `winresource::WindowsResource::new().set_icon("assets/brand/windows/dromon.ico").compile()?;` (only when `target_os = "windows"`). Use the same file for the installer later. |
| Linux | `linux/hicolor/<size>/apps/dromon.png` and `linux/hicolor/scalable/apps/dromon.svg` | Install the tree under `/usr/share/icons/hicolor/` (or `~/.local/share/icons/hicolor/`), and set `Icon=dromon` in `dromon.desktop`. |

### 2. Icon at runtime (Dock, taskbar, window)
Rune can set the app's icon at runtime with `rune::set_app_icon_choice`, which takes an `AppIcon`: `Bundle`, `Image(bytes)`, or a light/dark `Pair`.
- **macOS:** keep `AppIcon::Bundle` so the Dock uses the compiled `.icns`.
- **Windows and Linux:** pass `AppIcon::Image(include_bytes!("../../assets/brand/png/app-icon/dromon-256.png").to_vec())` early in startup. This sets the window and taskbar icon on Windows, and the window icon on Linux.
- Check the exact names against the pinned Rune revision (`crates/rune/src/platform/app_icon.rs`).

### 3. README and GitHub
- **README header:** use `<picture>` so it follows GitHub's light or dark theme:
  ```html
  <p align="center">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="assets/brand/svg/dromon-lockup-on-dark.svg">
      <img alt="Dromon" src="assets/brand/svg/dromon-lockup-on-light.svg" width="360">
    </picture>
  </p>
  ```
- **Social preview:** upload `png/social/social-preview-1280x640.png` under Settings › Social preview on GitHub. A person has to do this in the browser.

### 4. Website / docs (later)
- `web/favicon.ico`, `web/favicon.svg` and `web/apple-touch-icon.png` (180 px) go in the site root.
- Big hero art: `svg/dromon-mark.svg` on dark backgrounds, `svg/dromon-mark-on-light.svg` on light ones.

## Which file for what

| Need | Use |
|---|---|
| Mark on a dark background | `svg/dromon-mark.svg` |
| Mark on a light background | `svg/dromon-mark-on-light.svg` |
| Mark at 48 px or smaller (toolbar, about box, tabs) | `svg/dromon-mark-simple.svg` / `-on-light` (4 oars, no seams or shields, so it stays crisp) |
| One-colour print, embossing, watermarks | `svg/dromon-mark-white.svg`, `svg/dromon-mark-black.svg` |
| Logo with name | `svg/dromon-lockup-on-dark.svg`, `svg/dromon-lockup-on-light.svg` (wordmark already outlined, so no font is needed) |
| App icon art | `svg/dromon-app-icon.svg` (≥128 px), `svg/dromon-app-icon-small.svg` (≤64 px), `svg/dromon-app-icon-macos.svg` (Apple's 1024 grid with transparent margin) |
| Ready PNGs | `png/app-icon/`, `png/mark/`, `png/lockup/` (@1x/@2x/@4x), `png/social/` |

All marks have transparent backgrounds. The planking, oar ports, eye and shield rims are real cut-outs, so the mark works on any colour.

## Colours

| Token | Hex | Use |
|---|---|---|
| Mint (accent) | `#3DDC97` | Mark on dark backgrounds |
| Deep mint | `#0E9F6E` | Mark on light backgrounds |
| Ink | `#0A0C0C` | App icon tile |
| Deep sea | `#0D211C` | App icon sea band |
| Canvas | `#0C0E0E` | Dark page background |
| Text on dark | `#ECEFEE` | Wordmark on dark |
| Text on light | `#111515` | Wordmark on light |

The accent is still provisional; the plan picks between spearmint, teal and seafoam. To switch, rebuild with `--accent` and `--deep`:
- teal: `--accent #2DD4BF --deep #0F8F85`
- seafoam: `--accent #6EE7C8 --deep #13896F`

## Wordmark
- Lowercase "dromon" in Inter SemiBold, tracking −0.03em, outlined in the SVGs.
- Inter is SIL Open Font License, free to use and embed.

## Rules
- **Clear space:** at least the height of the hull (about ¼ of the mark's height) on every side.
- **Minimum size:** 120 px wide for the full mark; below that use the simple mark. 16 px only as the app icon.
- **Don't:** recolour outside the tokens above, add gradients, shadows or outlines, stretch, rotate, or put the mint mark on mid-tone backgrounds.

## Rebuilding
```bash
pip install pillow fonttools brotli          # plus librsvg: brew install librsvg / apt install librsvg2-bin
python3 tools/build_logo.py \
  --font path/to/Inter-SemiBold.woff2 \
  --font-regular path/to/Inter-Regular.woff2 \
  --out assets/brand
```
Inter can be downloaded from https://rsms.me/inter/ (the `.ttf`, `.otf` or `.woff2` files all work).
