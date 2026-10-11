#!/usr/bin/env python3
"""Asserts every localisation key the code uses exists, in both languages.

A key missing from the catalog renders as the key itself. That is invisible while
developing in English and immediately obvious to a Chinese user, so only a
mechanical check catches it.
"""

import json
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]
catalog_path = root / "apps" / "mac" / "Resources" / "Localizable.xcstrings"
required_languages = {"en", "zh-Hans"}

used: set[str] = set()
for swift in (root / "apps" / "mac" / "Sources").rglob("*.swift"):
    used |= set(re.findall(r'localized:\s*"([^"]+)"', swift.read_text(encoding="utf-8")))

catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
defined: set[str] = set(catalog["strings"])

missing = sorted(used - defined)
incomplete = sorted(
    key
    for key, entry in catalog["strings"].items()
    if required_languages - set(entry.get("localizations", {}))
)

if missing:
    print("used in code but absent from the catalog:", file=sys.stderr)
    for key in missing:
        print(f"  - {key}", file=sys.stderr)
if incomplete:
    print("missing a translation:", file=sys.stderr)
    for key in incomplete:
        print(f"  - {key}", file=sys.stderr)

if missing or incomplete:
    sys.exit(1)

print(f"  {len(defined)} keys, both languages present")
