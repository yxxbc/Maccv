#!/usr/bin/env python3
"""Writes the update feed for a release.

Called by the release workflow with the artifact URLs and digests it just produced.
Kept as a script rather than an inline heredoc for two reasons: a nested heredoc
inside a YAML block scalar is fragile, and this is the one piece of the release
pipeline that a user's auto-update depends on, so it deserves to be readable and
testable on its own.

Environment:
    VERSION, TAG, CHANNEL        the release identity
    ARM64_URL, ARM64_SHA, ARM64_SIZE
    X86_64_URL, X86_64_SHA, X86_64_SIZE
"""

from __future__ import annotations

import datetime
import json
import os
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[2]
os.chdir(root)


def required(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        print(f"error: {name} is not set", file=sys.stderr)
        sys.exit(1)
    return value


version = required("VERSION")
tag = required("TAG")
channel = required("CHANNEL")

if channel not in {"stable", "preview"}:
    print(f"error: CHANNEL must be stable or preview, got {channel!r}", file=sys.stderr)
    sys.exit(1)

downloads = [
    {
        "arch": "arm64",
        "url": required("ARM64_URL"),
        "sha256": required("ARM64_SHA").lower(),
        "size": int(required("ARM64_SIZE")),
    },
    {
        "arch": "x86_64",
        "url": required("X86_64_URL"),
        "sha256": required("X86_64_SHA").lower(),
        "size": int(required("X86_64_SIZE")),
    },
]

# The engine refuses a manifest it cannot verify, so a feed that fails these rules
# would silently stop updating every installed copy. Fail the release instead.
problems: list[str] = []
for entry in downloads:
    digest = entry["sha256"]
    if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
        problems.append(f"{entry['arch']}: sha256 is not a 64-character hex digest")
    if not entry["url"].startswith("https://"):
        problems.append(f"{entry['arch']}: download_url must be HTTPS")
    if entry["size"] <= 0:
        problems.append(f"{entry['arch']}: size must be positive")
if len({e["arch"] for e in downloads}) != len(downloads):
    problems.append("duplicate architecture")
if problems:
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    sys.exit(1)

target = pathlib.Path("docs") / ("preview.json" if channel == "preview" else "latest.json")

# Release notes come from the changelog, so the feed and the release page cannot
# disagree about what changed.
notes = subprocess.run(
    ["./Scripts/changelog-notes.sh", version],
    capture_output=True,
    text=True,
    check=True,
).stdout.strip()

# The revocation list accumulates: dropping it on each release would silently
# un-revoke every withdrawn build.
revoked: list[str] = []
if target.exists():
    try:
        revoked = json.loads(target.read_text(encoding="utf-8")).get("revoked", [])
    except json.JSONDecodeError:
        revoked = []

feed = {
    "version": version,
    "tag": tag,
    "download_url": downloads[0]["url"],
    "sha256": downloads[0]["sha256"],
    "size": downloads[0]["size"],
    "minimum_system_version": "15.0",
    "prerelease": channel == "preview",
    "published_at": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "notes": notes,
    "revoked": revoked,
    "downloads": downloads,
}

target.write_text(json.dumps(feed, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
print(f"{target} -> {version} ({channel}), {len(downloads)} architectures")
if revoked:
    print(f"revoked: {', '.join(revoked)}")
