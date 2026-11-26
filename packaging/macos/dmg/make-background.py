#!/usr/bin/env python3
"""Draws the DMG window's background: background.png (660x400) and background@2x.png.

    python3 packaging/macos/dmg/make-background.py      # needs Pillow; writes beside this file

Dromon's dark canvas, a tile under each icon, an arrow in the accent from the app to
Applications, and the instruction. Finder draws each icon's name itself, black in light mode and
white in dark mode, so the tiles are a mid-tone both read on (about 4.5:1 either way). The icon
positions here and in settings.py must agree. The PNGs are committed; run this after a change
and look at both.
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).resolve().parent
FONT = HERE.parents[2] / "crates/dromon-app/assets/fonts/InterVariable.ttf"

WIDTH, HEIGHT = 660, 400  # the window's content, in points (settings.py)
APP, APPLICATIONS = (165, 175), (495, 175)  # icon centres (settings.py)
TILE = (190, 196)  # under each 128 pt icon and its name
CANVAS = (12, 14, 14)  # theme canvas #0C0E0E
GLOW = (61, 220, 151)  # Spearmint #3DDC97
TILE_FILL = (112, 120, 118)  # #707876: 4.5:1 against both black and white labels
TILE_EDGE = (255, 255, 255, 30)
CAPTION = (169, 177, 175)  # theme text-2 #A9B1AF
DRAW_SCALE = 4  # drawn at 4x per point, then reduced: smooth edges


def font(size: float, weight: str) -> ImageFont.FreeTypeFont:
    f = ImageFont.truetype(str(FONT), round(size))
    f.set_variation_by_name(weight)
    return f


def draw(scale: int) -> Image.Image:
    s = scale * DRAW_SCALE
    img = Image.new("RGBA", (WIDTH * s, HEIGHT * s), CANVAS + (255,))

    # A faint accent glow behind the arrow, so the canvas isn't flat.
    glow = Image.new("RGBA", img.size, (0, 0, 0, 0))
    g = ImageDraw.Draw(glow)
    cx, cy = (APP[0] + APPLICATIONS[0]) / 2 * s, APP[1] * s
    r = 190
    g.ellipse([cx - r * s, cy - r * 0.55 * s, cx + r * s, cy + r * 0.55 * s], fill=GLOW + (30,))
    img = Image.alpha_composite(img, glow.filter(ImageFilter.GaussianBlur(70 * s)))

    d = ImageDraw.Draw(img)
    tw, th = TILE
    for x, y in (APP, APPLICATIONS):
        box = [(x - tw / 2) * s, (y - 84) * s, (x + tw / 2) * s, (y - 84 + th) * s]
        d.rounded_rectangle(box, radius=22 * s, fill=TILE_FILL + (255,), outline=TILE_EDGE, width=s)

    # The arrow: a shaft and a head, between the tiles.
    x0, x1, y = (APP[0] + tw / 2 + 22) * s, (APPLICATIONS[0] - tw / 2 - 22) * s, APP[1] * s
    stroke = 5 * s
    head = 18 * s
    d.line([x0, y, x1 - head * 0.6, y], fill=GLOW + (255,), width=stroke)
    d.polygon([(x1, y), (x1 - head, y - head * 0.75), (x1 - head, y + head * 0.75)], fill=GLOW + (255,))

    text = "Drag Dromon to Applications"
    f = font(17 * s, "SemiBold")
    w = d.textlength(text, font=f)
    d.text(((WIDTH * s - w) / 2, 330 * s), text, font=f, fill=CAPTION + (255,))

    return img.resize((WIDTH * scale, HEIGHT * scale), Image.LANCZOS).convert("RGB")


if __name__ == "__main__":
    for scale, name in ((1, "background.png"), (2, "background@2x.png")):
        draw(scale).save(HERE / name, optimize=True)
        print(HERE / name)
