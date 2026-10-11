#!/usr/bin/env python3
"""Checks the *published* update feed is usable, not merely reachable.

Runs in `Scripts/verify.sh`. A feed that returns HTTP 200 but fails the engine's own
validation breaks every installed copy silently: the app reports a check failure that
looks like a network problem, exactly as happened when the feed was never written at all.

Network access is optional here. An absent feed before the first release is normal, and a
machine with no network should not fail verification — so this reports what it found and
only fails when the response is present *and* wrong.
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

FEED_URL = "https://yxxbc.github.io/Maccv/latest.json"

response = subprocess.run(
    ["/usr/bin/curl", "--silent", "--show-error", "--location", "--max-time", "20", FEED_URL],
    capture_output=True,
    text=True,
)

if response.returncode != 0 or not response.stdout.strip():
    # Before the first release the URL legitimately 404s. No network is also fine: this is
    # the one check that reaches out, and it must not be the reason a build fails offline.
    print("  feed not reachable; treating as not-yet-published or offline")
    sys.exit(0)

try:
    feed = json.loads(response.stdout)
except json.JSONDecodeError as error:
    print(f"  the feed is not valid JSON: {error}", file=sys.stderr)
    print(f"  first 200 bytes: {response.stdout[:200]!r}", file=sys.stderr)
    sys.exit(1)

problems: list[str] = []

# The same rules the engine enforces client-side. A field the engine rejects means every
# installed copy stops updating, so the server side is checked against them here.
version = feed.get("version")
if not version:
    problems.append("no version")

digest = str(feed.get("sha256", ""))
if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
    problems.append(f"sha256 is not a 64-character lowercase hex digest: {digest[:20]!r}")

url = str(feed.get("download_url", ""))
if not url.startswith("https://"):
    problems.append(f"download_url is not HTTPS: {url!r}")

minimum = feed.get("minimum_system_version")
if minimum and minimum != "15.0":
    problems.append(f"minimum_system_version is {minimum}, but the README promises 15.0")

revoked = feed.get("revoked", [])
if not isinstance(revoked, list):
    problems.append("revoked must be a list")

downloads = feed.get("downloads", [])
if not isinstance(downloads, list) or not downloads:
    problems.append("no per-architecture downloads; an Intel Mac would get the wrong build")
else:
    architectures = [entry.get("arch") for entry in downloads]
    if sorted(architectures) != ["arm64", "x86_64"]:
        problems.append(f"expected arm64 and x86_64, found {architectures}")

    if len(set(architectures)) != len(architectures):
        problems.append(f"duplicate architecture in {architectures}")

    for entry in downloads:
        arch = entry.get("arch", "?")
        entry_digest = str(entry.get("sha256", ""))
        if len(entry_digest) != 64:
            problems.append(f"{arch}: sha256 is not a digest")
        if not str(entry.get("url", "")).startswith("https://"):
            problems.append(f"{arch}: url is not HTTPS")
        if not isinstance(entry.get("size"), int) or entry["size"] <= 0:
            problems.append(f"{arch}: size must be a positive integer")

if problems:
    print(f"  the live feed at {FEED_URL} is not usable:", file=sys.stderr)
    for problem in problems:
        print(f"    {problem}", file=sys.stderr)
    sys.exit(1)

print(
    f"  live feed: {version}, {sorted(entry['arch'] for entry in downloads)}"
    + (f", revoked {revoked}" if revoked else "")
)
