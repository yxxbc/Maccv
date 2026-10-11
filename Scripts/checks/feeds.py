#!/usr/bin/env python3
"""Validates the update feed schema.

The feed is generated at deploy time by `publish_feed.py` from the newest published
release, so it is normally absent from a working tree. When it is present — during a
deploy, or after running the generator locally — it is validated here against the same
rules the engine enforces client-side.

A feed the engine rejects means every installed copy silently stops updating, so the
rules are duplicated deliberately: the engine must not trust a server to have got it
right.
"""

from __future__ import annotations

import json
import pathlib
import sys

root = pathlib.Path(__file__).resolve().parents[2]
docs = root / "docs"

# Only the stable feed is generated at deploy time. A preview feed would be added by the
# same mechanism if a preview channel is reinstated.
FEEDS = ["latest.json"]

problems: list[str] = []
checked = 0

for name in FEEDS:
    path = docs / name
    if not path.exists():
        print(f"  --   {name} absent (generated at deploy time by publish_feed.py)")
        continue

    checked += 1
    try:
        feed = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        problems.append(f"{name}: not valid JSON ({error})")
        continue

    if not feed.get("version"):
        problems.append(f"{name}: no version")

    digest = str(feed.get("sha256", ""))
    if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
        problems.append(f"{name}: sha256 is not a 64-character lowercase hex digest")

    url = str(feed.get("download_url", ""))
    if not url.startswith("https://"):
        problems.append(f"{name}: download_url must be HTTPS, got {url!r}")

    revoked = feed.get("revoked", [])
    if not isinstance(revoked, list) or any(not isinstance(v, str) for v in revoked):
        problems.append(f"{name}: revoked must be a list of version strings")

    # Per-architecture entries are what stop an Intel Mac fetching an arm64 build.
    downloads = feed.get("downloads", [])
    if not isinstance(downloads, list) or not downloads:
        problems.append(f"{name}: no per-architecture downloads")
    else:
        architectures = [entry.get("arch") for entry in downloads]
        if sorted(architectures) != ["arm64", "x86_64"]:
            problems.append(f"{name}: expected arm64 and x86_64, found {architectures}")
        if len(set(architectures)) != len(architectures):
            problems.append(f"{name}: duplicate architecture")
        for entry in downloads:
            arch = entry.get("arch", "?")
            entry_digest = str(entry.get("sha256", ""))
            if len(entry_digest) != 64:
                problems.append(f"{name}/{arch}: sha256 is not a digest")
            if not str(entry.get("url", "")).startswith("https://"):
                problems.append(f"{name}/{arch}: url is not HTTPS")
            if not isinstance(entry.get("size"), int) or entry["size"] <= 0:
                problems.append(f"{name}/{arch}: size must be a positive integer")

    # The top-level fields must agree with the first architecture, since older clients
    # read only those.
    if downloads:
        first = downloads[0]
        if feed.get("sha256") != first.get("sha256"):
            problems.append(f"{name}: top-level sha256 disagrees with downloads[0]")
        if feed.get("size") != first.get("size"):
            problems.append(f"{name}: top-level size disagrees with downloads[0]")

    print(f"  ok   {name} ({feed.get('version')}), {len(downloads)} architectures")

if problems:
    print()
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)

if checked == 0:
    print("  no feed to validate (it is generated during a deploy)")
