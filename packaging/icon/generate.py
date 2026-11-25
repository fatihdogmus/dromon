#!/usr/bin/env python3
"""Renders Dromon's placeholder app icon from the shapes below into the committed outputs:

    packaging/icon/dromon-{16,32,48,64,128,256,512,1024}.png
    packaging/icon/dromon.ico   (16, 24, 32, 48, 64, 128, 256)
    packaging/icon/dromon.icns  (16 to 1024, @1x and @2x)

    python3 packaging/icon/generate.py           # writes the files above
    python3 packaging/icon/generate.py --svg     # prints dromon.svg (the same shapes)

The shapes live here once; dromon.svg is their vector copy, and the run fails if that file has
drifted from them. Each size is drawn at 8x and reduced with a Lanczos filter (no hinting, so the
small sizes are soft: a placeholder). Pillow writes no timestamps, so a rerun with the same Pillow
reproduces the files byte for byte.
"""

from __future__ import annotations

import io
import sys
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
CANVAS = 1024
SUPERSAMPLE = 8

BACKGROUND = "#0F1412"  # near-black, a little green
MINT = "#3DDC97"  # the app's accent (plan section 6.3)

# Rounded square, inset so macOS's icon grid (824/1024 content) and the other platforms agree.
SQUARE = (80, 80, 944, 944)
SQUARE_RADIUS = 192

# A galley in profile: a stern that curls up, a straight deck, the bow's ram, a lateen sail and
# a bank of oars. Paths are absolute M / L / C commands, closed.
HULL = [
    ("M", (204, 452)),
    ("C", (226, 520), (256, 548), (306, 556)),
    ("L", (792, 556)),
    ("L", (868, 604)),
    ("L", (784, 618)),
    ("C", (700, 684), (384, 690), (304, 648)),
    ("C", (254, 622), (218, 562), (204, 452)),
]
SAIL = [
    ("M", (400, 516)),
    ("L", (704, 232)),
    ("L", (664, 516)),
]
SAIL_OPACITY = 0.6
OARS = [((x, 640), (x - 88, 792)) for x in (360, 460, 560, 660)]
OAR_WIDTH = 30


def bezier(p0, p1, p2, p3, steps=48):
    return [
        (
            (1 - t) ** 3 * p0[0] + 3 * (1 - t) ** 2 * t * p1[0] + 3 * (1 - t) * t**2 * p2[0] + t**3 * p3[0],
            (1 - t) ** 3 * p0[1] + 3 * (1 - t) ** 2 * t * p1[1] + 3 * (1 - t) * t**2 * p2[1] + t**3 * p3[1],
        )
        for t in (i / steps for i in range(1, steps + 1))
    ]


def polygon(path):
    points = []
    for command, *args in path:
        if command in ("M", "L"):
            points.append(args[0])
        else:
            points.extend(bezier(points[-1], *args))
    return points


def svg_path(path):
    return " ".join(command + " " + " ".join(f"{x} {y}" for x, y in args) for command, *args in path) + " Z"


def svg():
    x0, y0, x1, y1 = SQUARE
    # Stroke attributes on each line, not a group: simple renderers don't inherit them.
    oars = "\n".join(
        f'  <line x1="{a[0]}" y1="{a[1]}" x2="{b[0]}" y2="{b[1]}" stroke="{MINT}" stroke-width="{OAR_WIDTH}" stroke-linecap="round"/>'
        for a, b in OARS
    )
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {CANVAS} {CANVAS}" width="{CANVAS}" height="{CANVAS}">
  <!-- Placeholder app icon, written by packaging/icon/generate.py: edit the shapes there. -->
  <rect x="{x0}" y="{y0}" width="{x1 - x0}" height="{y1 - y0}" rx="{SQUARE_RADIUS}" fill="{BACKGROUND}"/>
  <path d="{svg_path(SAIL)}" fill="{MINT}" fill-opacity="{SAIL_OPACITY}"/>
  <path d="{svg_path(HULL)}" fill="{MINT}"/>
{oars}
</svg>
"""


def hex_rgba(color, alpha=1.0):
    color = color.lstrip("#")
    return tuple(int(color[i : i + 2], 16) for i in (0, 2, 4)) + (round(alpha * 255),)


def render(size):
    scale = size * SUPERSAMPLE / CANVAS
    big = size * SUPERSAMPLE

    def s(point):
        return (point[0] * scale, point[1] * scale)

    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle([c * scale for c in SQUARE], radius=SQUARE_RADIUS * scale, fill=hex_rgba(BACKGROUND))

    # The sail is translucent mint over the background: composite it as its own layer.
    sail = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    ImageDraw.Draw(sail).polygon([s(p) for p in polygon(SAIL)], fill=hex_rgba(MINT, SAIL_OPACITY))
    image = Image.alpha_composite(image, sail)

    draw = ImageDraw.Draw(image)
    draw.polygon([s(p) for p in polygon(HULL)], fill=hex_rgba(MINT))
    radius = OAR_WIDTH * scale / 2
    for a, b in OARS:
        draw.line([s(a), s(b)], fill=hex_rgba(MINT), width=round(OAR_WIDTH * scale))
        for x, y in (s(a), s(b)):
            draw.ellipse([x - radius, y - radius, x + radius, y + radius], fill=hex_rgba(MINT))
    return image.resize((size, size), Image.Resampling.LANCZOS)


def png_bytes(image):
    out = io.BytesIO()
    image.save(out, format="PNG", optimize=True)
    return out.getvalue()


def main(argv):
    if argv[1:] == ["--svg"]:
        sys.stdout.write(svg())
        return 0
    if argv[1:]:
        print(__doc__, file=sys.stderr)
        return 2

    svg_file = HERE / "dromon.svg"
    if not svg_file.is_file() or svg_file.read_text() != svg():
        print(f"{svg_file} differs from the shapes in generate.py; rewrite it from `generate.py --svg`.", file=sys.stderr)
        return 1

    sizes = sorted({16, 24, 32, 48, 64, 128, 256, 512, 1024})
    images = {size: render(size) for size in sizes}

    for size in (16, 32, 48, 64, 128, 256, 512, 1024):
        (HERE / f"dromon-{size}.png").write_bytes(png_bytes(images[size]))

    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    images[256].save(
        HERE / "dromon.ico",
        format="ICO",
        sizes=[(n, n) for n in ico_sizes],
        append_images=[images[n] for n in ico_sizes if n != 256],
    )

    # Pillow's ICNS writer takes the largest image and finds the other sizes in append_images.
    images[1024].save(HERE / "dromon.icns", format="ICNS", append_images=[images[n] for n in (16, 32, 64, 128, 256, 512)])

    for name in sorted(p.name for p in HERE.iterdir() if p.suffix in (".png", ".ico", ".icns")):
        print(f"packaging/icon/{name}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
