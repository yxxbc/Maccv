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


en = structure(docs / "index.html")
zh = structure(docs / "zh" / "index.html")
if en != zh:
    print("the two language pages have drifted apart:", file=sys.stderr)
    print(f"  en: {en}", file=sys.stderr)
    print(f"  zh: {zh}", file=sys.stderr)
    sys.exit(1)

print(f"  {len(list(docs.rglob('*.html')))} pages, links resolve, languages in step")
