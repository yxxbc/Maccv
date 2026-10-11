#!/usr/bin/env python3
"""Downsamples a rendered preview and checks the result is a usable image.

Run by `Scripts/render-preview.sh`. Kept as a file rather than an inline heredoc so the
checks are readable and can be run on their own.

Two things are verified, because both failures look like success in a file listing:

1. **The panel actually drew.** A render that produced only the backdrop is a perfectly
   valid PNG of the right size, and would be committed without complaint. Contrast
   across the area the panel occupies is the cheapest signal that rows and text are
   there.
2. **The file is small enough to load.** An image on a repository's front page is
   fetched on every view; 2 MB of PNG is a real cost to a reader on a slow connection.

Usage:
    check-preview.py <raw.png> <out.png> [target_width]
"""

from __future__ import annotations

import pathlib
import sys

try:
    from PIL import Image
except ImportError:
    print("Pillow is required (python3 -m pip install Pillow)", file=sys.stderr)
    sys.exit(1)

# Measured at the target width; a README image above this is a cost to every reader.
MAX_KB = 700
# Below this, the panel almost certainly did not render — the backdrop alone is a smooth
# gradient, so its darkest and lightest samples across the panel area are close together.
MIN_CONTRAST = 120


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__)
        return 2

    raw_path = pathlib.Path(sys.argv[1])
    out_path = pathlib.Path(sys.argv[2])
    target_width = int(sys.argv[3]) if len(sys.argv) > 3 else 1000

    image = Image.open(raw_path)
    original = image.size

    height = round(image.height * target_width / image.width)
    resized = image.resize((target_width, height), Image.LANCZOS)
    resized.save(out_path, optimize=True)

    # --- is there a panel? ---------------------------------------------------
    # Sample the band the panel occupies. The generated backdrop is a smooth gradient;
    # rows, text and dividers give that band a wide range of luminance.
    rgb = resized.convert("RGB")
    band = [
        rgb.getpixel((x, y))
        for y in range(int(height * 0.20), int(height * 0.70), 3)
        for x in range(int(target_width * 0.35), int(target_width * 0.65), 3)
    ]
    contrast = max(sum(p) for p in band) - min(sum(p) for p in band)

    if contrast < MIN_CONTRAST:
        print(
            f"  contrast across the panel area is only {contrast}; the panel probably "
            "did not render",
            file=sys.stderr,
        )
        out_path.unlink(missing_ok=True)
        return 1

    # --- is it small enough? -------------------------------------------------
    size_kb = out_path.stat().st_size / 1024
    if size_kb > MAX_KB:
        print(
            f"  {out_path.name} is {size_kb:.0f} KB at {target_width}px wide, over the "
            f"{MAX_KB} KB budget for a README image",
            file=sys.stderr,
        )
        return 1

    print(
        f"  {out_path.name}: {original[0]}×{original[1]} -> "
        f"{target_width}×{height}, {size_kb:.0f} KB, contrast {contrast}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
