#!/usr/bin/env python3
"""Build every Dromon logo asset from one source of truth.

Usage:
    python3 tools/build_logo.py [--accent #3DDC97] [--deep #0E9F6E] [--font path/to/Inter-SemiBold.woff2]
                               [--font-regular path/to/Inter-Regular.woff2] [--out .]

Needs: Python 3.9+, Pillow, fontTools (+ brotli for .woff2), and `rsvg-convert` (librsvg) on PATH.
The ship is drawn in a 240 x 160 unit box. Details (planking, oar ports, the eye, shield rims)
are real cut-outs (an SVG mask), so every mark works on any background.
"""
import argparse
import os
import shutil
import subprocess
from io import BytesIO

from PIL import Image

# ---------------------------------------------------------------- the ship

FULL_OARS = [66, 82, 98, 114, 130, 146, 162, 178]
SIMPLE_OARS = [76, 106, 136, 166]
SHIELDS = [64, 80, 96, 120, 136, 152, 172]


def ship(detail: str, color: str, uid: str, wave_alpha=(0.45, 0.22)) -> str:
    """SVG elements for the galley in its 240x160 box. `detail` is 'full' or 'simple'."""
    full = detail == "full"
    oars = FULL_OARS if full else SIMPLE_OARS
    sw = 2.5 if full else 4.5
    mw, bw, cw = sw + 1, sw + 2.5, sw + 2

    cut = [
        '<rect x="-20" y="-20" width="280" height="200" fill="#fff"/>',
        '<path d="M44 100 Q120 108 202 98" fill="none" stroke="#000" stroke-width="2"/>',
        '<circle cx="194" cy="97" r="2.6" fill="#000"/>',
    ]
    if full:
        cut.append('<path d="M56 111 Q120 118 200 108" fill="none" stroke="#000" stroke-width="1.5"/>')
        cut += [f'<circle cx="{x}" cy="90" r="3.6" fill="none" stroke="#000" stroke-width="1.5"/>' for x in SHIELDS]
    cut += [f'<circle cx="{x}" cy="105" r="1.8" fill="#000"/>' for x in oars]

    p = []
    p.append(f'<path d="M14 134 q10 -6 20 0 t20 0 t20 0 t20 0 t20 0 t20 0 t20 0 t20 0 t20 0 t20 0 t20 0" '
             f'fill="none" stroke="{color}" stroke-opacity="{wave_alpha[0]}" stroke-width="{sw}" stroke-linecap="round"/>')
    if full:
        p.append(f'<path d="M54 146 q10 -5 20 0 t20 0 t20 0 t20 0 t20 0 t20 0 t20 0" fill="none" stroke="{color}" '
                 f'stroke-opacity="{wave_alpha[1]}" stroke-width="2" stroke-linecap="round"/>')
    for x in oars:
        p.append(f'<line x1="{x}" y1="100" x2="{x-18}" y2="134" stroke="{color}" stroke-width="{sw}" stroke-linecap="round"/>')
        p.append(f'<line x1="{x-14}" y1="128" x2="{x-19}" y2="137" stroke="{color}" stroke-width="{bw}" stroke-linecap="round"/>')
    for d in ("M58 18 L150 58 Q124 70 106 86 Q66 70 58 18 Z", "M138 40 L196 64 Q178 74 164 84 Q140 66 138 40 Z"):
        p.append(f'<path d="{d}" fill="{color}" fill-opacity="0.22" stroke="{color}" stroke-width="{sw}" stroke-linejoin="round"/>')
    if full:
        seams = [(81, 28, 106, 86), (104, 38, 106, 86), (127, 48, 106, 86), (157, 48, 164, 84), (177, 56, 164, 84)]
        p.append(f'<g stroke="{color}" stroke-opacity="0.55" stroke-width="1.5" stroke-linecap="round">'
                 + "".join(f'<line x1="{a}" y1="{b}" x2="{c}" y2="{d}"/>' for a, b, c, d in seams) + "</g>")
    for a, b, c, d in ((104, 92, 104, 20), (160, 90, 165, 42), (58, 18, 150, 58), (138, 40, 196, 64)):
        p.append(f'<line x1="{a}" y1="{b}" x2="{c}" y2="{d}" stroke="{color}" stroke-width="{mw}" stroke-linecap="round"/>')
    p.append(f'<path d="M104 19 L80 12 L87 19 L80 26 Z" fill="{color}"/>')
    p.append(f'<path d="M40 96 C24 80 26 60 42 58 C52 57 54 68 46 70" fill="none" stroke="{color}" stroke-width="{cw}" stroke-linecap="round"/>')
    if full:
        p += [f'<circle cx="{x}" cy="90" r="3.6" fill="{color}"/>' for x in SHIELDS]
    p.append(f'<path d="M34 92 Q115 99 196 90 L206 104 L226 108 L206 112 Q120 128 44 114 Q30 106 34 92 Z" fill="{color}"/>')
    p.append(f'<path d="M176 91 L178 80 H200 L199 92 Z" fill="{color}"/>')

    return (f'<defs><mask id="cut-{uid}" maskUnits="userSpaceOnUse" x="-20" y="-20" width="280" height="200">'
            + "".join(cut) + f'</mask></defs><g mask="url(#cut-{uid})">' + "".join(p) + "</g>")


def svg_doc(w, h, body, view=None):
    vb = view or f"0 0 {w} {h}"
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{vb}">{body}</svg>\n')


# ---------------------------------------------------------------- the wordmark

class Wordmark:
    def __init__(self, font_path):
        from fontTools.ttLib import TTFont
        from fontTools.pens.svgPathPen import SVGPathPen
        self.font = TTFont(font_path)
        self.Pen = SVGPathPen
        self.upm = self.font["head"].unitsPerEm
        self.cmap = self.font.getBestCmap()
        self.glyphs = self.font.getGlyphSet()

    def path(self, text, size, x, baseline, tracking_em=-0.03, color="#ECEFEE"):
        """Outlined text as one <path>. Returns (svg, advance_width)."""
        s = size / self.upm
        pen_x = 0.0
        parts = []
        for ch in text:
            g = self.cmap[ord(ch)]
            pen = self.Pen(self.glyphs)
            self.glyphs[g].draw(pen)
            d = pen.getCommands()
            if d:
                parts.append(f'<path transform="translate({x + pen_x:.2f} {baseline:.2f}) scale({s:.5f} {-s:.5f})" d="{d}"/>')
            pen_x += self.font["hmtx"][g][0] * s + tracking_em * size
        width = pen_x - tracking_em * size
        return f'<g fill="{color}">' + "".join(parts) + "</g>", width


# ---------------------------------------------------------------- compositions

def mark_svg(detail, color, uid):
    # Content spans roughly x 12..228, y 10..148 inside the 240x160 box.
    return svg_doc(240, 160, ship(detail, color, uid))


def app_icon_svg(detail, accent, uid, apple_grid=False):
    """Rounded tile + deep-sea band + ship. apple_grid=True follows Apple's 1024 template (824 tile, 100 margin)."""
    S = 1024
    if apple_grid:
        x0, size, r = 100, 824, 185
    else:
        x0, size, r = 0, 1024, 226
    band_h = size * 0.33
    ship_w = size * 0.92
    scale = ship_w / 240
    ship_h = 160 * scale
    sx = x0 + (size - ship_w) / 2
    sy = x0 + (size - ship_h) / 2 + size * 0.03
    border = max(2, size * 0.004)
    body = (
        f'<defs><clipPath id="tile-{uid}"><rect x="{x0}" y="{x0}" width="{size}" height="{size}" rx="{r}"/></clipPath></defs>'
        f'<g clip-path="url(#tile-{uid})">'
        f'<rect x="{x0}" y="{x0}" width="{size}" height="{size}" fill="#0A0C0C"/>'
        f'<rect x="{x0}" y="{x0 + size - band_h}" width="{size}" height="{band_h}" fill="#0D211C"/>'
        f'<g transform="translate({sx:.2f} {sy:.2f}) scale({scale:.4f})">{ship(detail, accent, uid, (0.6, 0.3))}</g>'
        f'</g>'
        f'<rect x="{x0 + border/2}" y="{x0 + border/2}" width="{size - border}" height="{size - border}" rx="{r - border/2}" '
        f'fill="none" stroke="#FFFFFF" stroke-opacity="0.08" stroke-width="{border}"/>'
    )
    return svg_doc(S, S, body)


def lockup_svg(wm, mark_color, text_color, uid, detail="full"):
    mark_h = 120
    scale = mark_h / 160
    mark_w = 240 * scale
    size = 62
    gap = 22
    baseline = 89  # sits the baseline on the ship's keel line
    text, tw = wm.path("dromon", size, mark_w + gap, baseline, color=text_color)
    W = mark_w + gap + tw + 4
    body = f'<g transform="scale({scale})">{ship(detail, mark_color, uid)}</g>{text}'
    return svg_doc(round(W), mark_h, body), round(W), mark_h


def social_svg(wm, wm_regular, accent):
    W, H = 1280, 640
    lock, lw, lh = lockup_svg(wm, accent, "#ECEFEE", "social")
    inner = lock.split(">", 1)[1].rsplit("</svg>", 1)[0]
    k = 820 / lw
    tag, tw = wm_regular.path("A fast, beautiful SFTP file browser", 34, 0, 0, tracking_em=-0.005, color="#A3ABA9")
    block = lh * k + 56 + 34
    x = (W - lw * k) / 2
    y = (H - block) / 2
    body = (f'<rect width="{W}" height="{H}" fill="#0C0E0E"/>'
            f'<g transform="translate({x:.1f} {y:.1f}) scale({k:.4f})">{inner}</g>'
            f'<g transform="translate({(W - tw) / 2:.1f} {y + lh * k + 56 + 26:.1f})">{tag}</g>')
    return svg_doc(W, H, body)


# ---------------------------------------------------------------- output helpers

def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def render_png(svg_text, w, h):
    out = subprocess.run(["rsvg-convert", "-w", str(w), "-h", str(h), "-f", "png"],
                         input=svg_text.encode(), capture_output=True, check=True).stdout
    return Image.open(BytesIO(out)).convert("RGBA")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--accent", default="#3DDC97")
    ap.add_argument("--deep", default="#0E9F6E", help="accent for light backgrounds")
    ap.add_argument("--font", required=True, help="Inter SemiBold (.ttf/.otf/.woff2)")
    ap.add_argument("--font-regular", required=True, help="Inter Regular, for the social card tagline")
    ap.add_argument("--out", default=".")
    a = ap.parse_args()
    O = lambda *p: os.path.join(a.out, *p)
    wm, wm_reg = Wordmark(a.font), Wordmark(a.font_regular)

    # SVG sources
    svgs = {
        "svg/dromon-mark.svg": mark_svg("full", a.accent, "m"),
        "svg/dromon-mark-on-light.svg": mark_svg("full", a.deep, "ml"),
        "svg/dromon-mark-simple.svg": mark_svg("simple", a.accent, "s"),
        "svg/dromon-mark-simple-on-light.svg": mark_svg("simple", a.deep, "sl"),
        "svg/dromon-mark-white.svg": mark_svg("full", "#FFFFFF", "w"),
        "svg/dromon-mark-black.svg": mark_svg("full", "#000000", "b"),
        "svg/dromon-app-icon.svg": app_icon_svg("full", a.accent, "ai"),
        "svg/dromon-app-icon-small.svg": app_icon_svg("simple", a.accent, "ais"),
        "svg/dromon-app-icon-macos.svg": app_icon_svg("full", a.accent, "mac", apple_grid=True),
    }
    lock_dark, LW, LH = lockup_svg(wm, a.accent, "#ECEFEE", "ld")
    lock_light, _, _ = lockup_svg(wm, a.deep, "#111515", "ll")
    svgs["svg/dromon-lockup-on-dark.svg"] = lock_dark
    svgs["svg/dromon-lockup-on-light.svg"] = lock_light
    social = social_svg(wm, wm_reg, a.accent)
    for k, v in svgs.items():
        write(O(k), v)

    def icon(px, apple=False):
        if apple:
            return render_png(svgs["svg/dromon-app-icon-macos.svg"] if px >= 128 else app_icon_svg("simple", a.accent, f"m{px}", True), px, px)
        return render_png(svgs["svg/dromon-app-icon.svg"] if px >= 128 else svgs["svg/dromon-app-icon-small.svg"], px, px)

    def save(img, *p):
        os.makedirs(os.path.dirname(O(*p)), exist_ok=True)
        img.save(O(*p))

    # Generic app icon PNGs (full-bleed tile: Windows, Linux, web)
    for px in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
        save(icon(px), "png", "app-icon", f"dromon-{px}.png")

    # macOS .icns (Apple grid, transparent margin)
    mac = {px: icon(px, apple=True) for px in (16, 32, 64, 128, 256, 512, 1024)}
    os.makedirs(O("macos"), exist_ok=True)
    mac[1024].save(O("macos", "Dromon.icns"), append_images=[mac[p] for p in (16, 32, 64, 128, 256, 512)])
    save(mac[1024], "macos", "Dromon-1024.png")
    for px in (16, 32, 128, 256, 512):
        save(mac[px], "macos", "Dromon.iconset", f"icon_{px}x{px}.png")
        save(mac[px * 2] if px * 2 in mac else icon(px * 2, apple=True), "macos", "Dromon.iconset", f"icon_{px}x{px}@2x.png")

    # Windows .ico
    win = {px: icon(px) for px in (16, 24, 32, 48, 64, 128, 256)}
    os.makedirs(O("windows"), exist_ok=True)
    win[256].save(O("windows", "dromon.ico"), sizes=[(p, p) for p in win], append_images=[win[p] for p in win if p != 256])

    # Linux hicolor theme
    for px in (16, 24, 32, 48, 64, 128, 256, 512):
        save(icon(px), "linux", "hicolor", f"{px}x{px}", "apps", "dromon.png")
    write(O("linux", "hicolor", "scalable", "apps", "dromon.svg"), svgs["svg/dromon-app-icon.svg"])

    # Web
    os.makedirs(O("web"), exist_ok=True)
    icon(32).save(O("web", "favicon.ico"), sizes=[(16, 16), (32, 32)], append_images=[icon(16)])
    write(O("web", "favicon.svg"), svgs["svg/dromon-app-icon-small.svg"])
    save(icon(180), "web", "apple-touch-icon.png")

    # Marks and lockups as PNG
    for name, svg in (("dromon-mark", svgs["svg/dromon-mark.svg"]), ("dromon-mark-on-light", svgs["svg/dromon-mark-on-light.svg"])):
        for w in (480, 960, 1920):
            save(render_png(svg, w, w * 2 // 3), "png", "mark", f"{name}-{w}.png")
    for name, svg in (("dromon-lockup-on-dark", lock_dark), ("dromon-lockup-on-light", lock_light)):
        for k in (1, 2, 4):
            save(render_png(svg, LW * k, LH * k), "png", "lockup", f"{name}@{k}x.png")
    write(O("svg", "social-preview.svg"), social)
    save(render_png(social, 1280, 640), "png", "social", "social-preview-1280x640.png")

    shutil.copy(__file__, O("tools", "build_logo.py")) if os.path.abspath(__file__) != os.path.abspath(O("tools", "build_logo.py")) else None
    print("done")


if __name__ == "__main__":
    main()
