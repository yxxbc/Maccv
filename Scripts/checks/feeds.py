#!/usr/bin/env python3
"""Validates the published update feeds.

The engine refuses a manifest it cannot verify, so shipping one that fails these
rules would silently break updating for every installed copy. A missing feed is
not an error: they are written by the release workflow.
"""

import json
import pathlib
import sys

root = pathlib.Path(__file__).resolve().parents[2]
docs = root / "docs"

problems: list[str] = []

for name in ("latest.json", "preview.json"):
    path = docs / name
    if not path.exists():
        print(f"  --   {name} absent (written by the release workflow)")
        continue

    try:
        feed = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        problems.append(f"{name}: not valid JSON ({error})")
        continue

    digest = str(feed.get("sha256", ""))
    if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
        problems.append(f"{name}: sha256 is not a 64-character lowercase hex digest")

    url = str(feed.get("download_url", ""))
    if not url.startswith("https://"):
        problems.append(f"{name}: download_url must be HTTPS, got {url!r}")

    revoked = feed.get("revoked", [])
    if not isinstance(revoked, list) or any(not isinstance(v, str) for v in revoked):
        problems.append(f"{name}: revoked must be a list of version strings")

    if not feed.get("version"):
        problems.append(f"{name}: no version")

    print(f"  ok   {name} ({feed.get('version', '?')})"
          + (f", revoked: {', '.join(revoked)}" if revoked else ""))

if problems:
    print()
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)
