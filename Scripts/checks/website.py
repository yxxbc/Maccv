#!/usr/bin/env python3
"""Checks the website for broken internal links and language drift.

The usual failure mode of a hand-maintained bilingual site is one language
quietly going stale, so the two pages are compared structurally rather than
trusted to stay in step.
"""

import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]
docs = root / "docs"

broken: list[str] = []
for page in docs.rglob("*.html"):
    text = page.read_text(encoding="utf-8")
    for href in re.findall(r'(?:href|src)="([^"]+)"', text):
        if href.startswith(("http://", "https://", "#", "mailto:")):
            continue
        clean = href.split("#")[0]
        if not clean:
            continue
        if not (page.parent / clean).resolve().exists():
            broken.append(f"{page.relative_to(docs)}: {href}")

if broken:
    print("broken internal links:", file=sys.stderr)
    for entry in broken:
        print(f"  - {entry}", file=sys.stderr)
    sys.exit(1)


def structure(path: pathlib.Path):
    text = path.read_text(encoding="utf-8")
    return (
        re.findall(r'<section id="([^"]+)"', text),
        sorted(set(re.findall(r'href="#([^"]+)"', text))),
        text.count("<table"),
        text.count("<li>"),
    )


# Each page must show its own language's hero, and the file must resolve from where the
# page says it is — the Chinese page references `../preview-hero-zh.png`, which is a
# different location from the English page's. Joining the name onto the page's own
# directory would look in the wrong place and report a false failure.
HERO_IMAGES = {
    "index.html": "preview-hero-en.png",
    "zh/index.html": "preview-hero-zh.png",
}

for relative, image in HERO_IMAGES.items():
    page = docs / relative
    text = page.read_text(encoding="utf-8")
    if image not in text:
        print(f"{relative} does not reference its hero image ({image})", file=sys.stderr)
        sys.exit(1)
    # Resolve the reference the way a browser would.
    import re

    match = re.search(rf'src="([^"]*{re.escape(image)})"', text)
    if not match:
        print(f"{relative}: could not find the src for {image}", file=sys.stderr)
        sys.exit(1)
    resolved = (page.parent / match.group(1)).resolve()
    if not resolved.exists():
        print(
            f"{relative} references {match.group(1)}, which resolves to "
            f"{resolved} and does not exist",
            file=sys.stderr,
        )
        sys.exit(1)

en = structure(docs / "index.html")
zh = structure(docs / "zh" / "index.html")
if en != zh:
    print("the two language pages have drifted apart:", file=sys.stderr)
    print(f"  en: {en}", file=sys.stderr)
    print(f"  zh: {zh}", file=sys.stderr)
    sys.exit(1)

print(
    f"  {len(list(docs.rglob('*.html')))} pages, links resolve, "
    "hero images present, languages in step"
)
