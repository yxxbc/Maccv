#!/usr/bin/env python3
"""Generates the DMG background image, light and dark, at 1x and 2x.

Why generate rather than ship a PNG: the canvas has to match the Finder window
size exactly, and a mismatch shows up as a background that is cropped or padded
inside the mounted image. Deriving it from one set of numbers means the layout in
`dmg-settings.py` and the artwork cannot disagree.

Output (into the directory given as the first argument):
    background.png        1x, the TIFF's base layer
    background@2x.png     2x, merged by `tiffutil -cathidpicheck`
    background.tiff       both, which is what Finder and .DS_Store want
    background-dark.*     the same for macOS dark appearance

Dark mode is not optional: a black-on-white canvas is unreadable for anyone
running dark appearance, which for a developer tool is most people.
"""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    print("error: Pillow is required (python3 -m pip install Pillow)", file=sys.stderr)
    sys.exit(1)

# Must match `window_rect` in dmg-settings.py. Finder draws the background at this
# size and does not scale it.
WIDTH = 660
HEIGHT = 420
SCALE = 2

# The same palette as the site and the app icon, so all three agree.
PALETTE = {
    "indigo": (76, 61, 219),
    "violet": (109, 74, 232),
    "violet_light": (139, 92, 246),
    "cyan": (34, 211, 238),
}

LIGHT = {
    "background": (251, 251, 253),
    "text": (22, 22, 29),
    "text_dim": (110, 110, 132),
    "arrow": (196, 196, 214),
}

DARK = {
    "background": (16, 16, 24),
    "text": (242, 242, 247),
    "text_dim": (150, 150, 172),
    "arrow": (70, 70, 96),
}


def find_font() -> str | None:
    """Locates a font that exists on a stock Mac and in CI."""
    candidates = [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFNSDisplay.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
    ]
    for path in candidates:
        if pathlib.Path(path).exists():
            return path
    return None


def draw(canvas: Image.Image, theme: dict, scale: int, font_path: str | None) -> None:
    """Paints one resolution of the background."""
    w, h = WIDTH * scale, HEIGHT * scale
    draw_ctx = ImageDraw.Draw(canvas, "RGBA")

    # A soft radial glow, built as a downscaled gradient rather than stacked
    # ellipses: drawing hundreds of concentric filled circles accumulates alpha at
    # the centre and produces a solid disc, not a glow.
    glow = Image.new("L", (96, 96), 0)
    glow_pixels = glow.load()
    for gy in range(96):
        for gx in range(96):
            # Normalised distance from the centre, clamped at the edge.
            dx = (gx - 47.5) / 47.5
            dy = (gy - 47.5) / 47.5
            distance = min(1.0, (dx * dx + dy * dy) ** 0.5)
            # Quadratic falloff: a linear ramp bands visibly at this size.
            glow_pixels[gx, gy] = int(52 * (1 - distance) ** 2)
    # Upscaling a small gradient is both cheap and smoother than drawing at scale.
    glow = glow.resize(canvas.size, Image.BICUBIC)

    tint = Image.new("RGBA", canvas.size, (*PALETTE["violet"], 255))
    tint.putalpha(glow)
    canvas.alpha_composite(tint)

    # The drag arrow, drawn rather than shipped so it scales cleanly and can be
    # tinted per appearance.
    arrow_y = int(h * 0.52)
    left = int(w * 0.42)
    right = int(w * 0.58)
    thickness = max(2, int(2.5 * scale))
    draw_ctx.line(
        [(left, arrow_y), (right - int(16 * scale), arrow_y)],
        fill=(*theme["arrow"], 255),
        width=thickness,
    )
    head = int(11 * scale)
    tip_x = right - int(4 * scale)
    draw_ctx.polygon(
        [
            (tip_x, arrow_y),
            (tip_x - head, arrow_y - int(head * 0.62)),
            (tip_x - head, arrow_y + int(head * 0.62)),
        ],
        fill=(*theme["arrow"], 255),
    )

    if font_path is None:
        return

    def centred(text: str, y: int, size: int, colour: tuple, weight: str = "normal") -> None:
        try:
            font = ImageFont.truetype(font_path, size * scale)
        except OSError:
            return
        box = draw_ctx.textbbox((0, 0), text, font=font)
        draw_ctx.text(
            ((w - (box[2] - box[0])) // 2, y * scale),
            text,
            font=font,
            fill=(*colour, 255),
        )
        _ = weight

    # Captions sit outside the glow's strongest area so they stay legible in both
    # appearances.
    centred("MacCV", 40, 22, theme["text"])
    centred("Drag MacCV to Applications", 322, 13, theme["text_dim"])


def emit(directory: pathlib.Path, name: str, theme: dict, font_path: str | None) -> None:
    """Writes the 1x, 2x and combined TIFF for one appearance."""
    for scale, suffix in ((1, ""), (SCALE, "@2x")):
        canvas = Image.new(
            "RGBA",
            (WIDTH * scale, HEIGHT * scale),
            (*theme["background"], 255),
        )
        draw(canvas, theme, scale, font_path)
        # Flattened to RGB: Finder renders the background opaquely, and an alpha
        # channel here only risks a wrong composite.
        canvas.convert("RGB").save(directory / f"{name}{suffix}.png")

    # A single flattened multi-resolution TIFF is the format Finder actually reads,
    # and `tiffutil` is Apple's own tool for producing it.
    subprocess.run(
        [
            "/usr/bin/tiffutil",
            "-cathidpicheck",
            str(directory / f"{name}.png"),
            str(directory / f"{name}@2x.png"),
            "-out",
            str(directory / f"{name}.tiff"),
        ],
        check=True,
        capture_output=True,
    )


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    directory = pathlib.Path(sys.argv[1])
    directory.mkdir(parents=True, exist_ok=True)

    font_path = find_font()
    emit(directory, "background", LIGHT, font_path)
    emit(directory, "background-dark", DARK, font_path)

    for produced in sorted(directory.glob("background*")):
        print(f"  {produced.name:26s} {produced.stat().st_size:>7d} B")
    return 0


if __name__ == "__main__":
    sys.exit(main())
