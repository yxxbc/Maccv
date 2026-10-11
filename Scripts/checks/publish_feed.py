#!/usr/bin/env python3
"""Writes the update feed from the newest published release.

Called by `.github/workflows/pages.yml` on every deploy, before the artifact is
uploaded.

Why derived at deploy time rather than written once at release time:

The feed has to live at `docs/latest.json` to be served, but `docs/` is what Pages
uploads — so a feed written during a release is **deleted by the next push to main**.
That is exactly what happened: the release published, the site was redeployed from the
new commit, and every installed copy started reporting a failed update check, because
the URL returned the 404 page rather than a manifest.

Deriving it on every deploy fixes that in a way that cannot drift. The feed is a
function of the newest release's GitHub assets, so it is regenerated from them rather
than maintained alongside them — and there is no in-repo copy that can describe a
release that was never published.

The release notes come from the changelog in the *published* commit, so the feed and the
release page cannot disagree about what changed.
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[2]
os_chdir_done = False


def gh(*args: str) -> str:
    """Runs `gh` and returns stdout, or raises with the message it printed."""
    result = subprocess.run(
        ["gh", *args],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(f"gh {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def changelog_notes(version: str) -> str:
    """Notes for one version, from CHANGELOG.md."""
    result = subprocess.run(
        ["Scripts/changelog-notes.sh", version],
        cwd=root,
        capture_output=True,
        text=True,
    )
    # A missing section is worth knowing about but not worth failing the site deploy for:
    # the feed is still usable without notes.
    if result.returncode != 0:
        print(f"  warning: no changelog notes for {version}", file=sys.stderr)
        return ""
    return result.stdout.strip()


def read_sums(tag: str) -> dict[str, str]:
    """Downloads the release's `SHA256SUMS` and parses it.

    The digests come from the release rather than from a stored copy, so they always
    describe the artifacts that actually exist.
    """
    result = subprocess.run(
        [
            "/usr/bin/curl",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "60",
            f"https://github.com/{REPO}/releases/download/{tag}/SHA256SUMS",
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise RuntimeError(f"could not fetch SHA256SUMS for {tag}")

    sums: dict[str, str] = {}
    for line in result.stdout.splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        digest, name = parts[0], parts[1].lstrip("./")
        sums[name] = digest.lower()
    return sums


REPO = "yxxbc/Maccv"

# ---------------------------------------------------------------------------
# Newest stable release
# ---------------------------------------------------------------------------

# `gh release list --json` accepts only a subset of fields — asking for `assets` fails
# the whole command. So the release is chosen from the list, then its assets are fetched
# separately.
try:
    releases = json.loads(
        gh(
            "release",
            "list",
            "--repo",
            REPO,
            "--limit",
            "30",
            "--json",
            "tagName,isPrerelease,isDraft,publishedAt",
        )
    )
except (json.JSONDecodeError, RuntimeError) as error:
    print(f"  could not read the release list: {error}", file=sys.stderr)
    sys.exit(1)

stable = [release for release in releases if not release["isPrerelease"] and not release["isDraft"]]
if not stable:
    # Nothing published yet: leave whatever is there rather than writing a feed that
    # points at a release nobody can download.
    print("  no stable release published; leaving the feed alone")
    sys.exit(0)

# Newest by publication date, since the list order is not guaranteed.
newest = max(stable, key=lambda release: release["publishedAt"])
tag: str = newest["tagName"]
version = tag.lstrip("v")

try:
    detail = json.loads(gh("release", "view", tag, "--repo", REPO, "--json", "assets"))
except (json.JSONDecodeError, RuntimeError) as error:
    print(f"  could not read the assets of {tag}: {error}", file=sys.stderr)
    sys.exit(1)

assets = detail.get("assets", [])
names = {asset["name"] for asset in assets}

print(f"  newest stable release: {tag}")

# ---------------------------------------------------------------------------
# Per-architecture downloads
# ---------------------------------------------------------------------------

sums = read_sums(tag)
downloads = []

for arch in ("arm64", "x86_64"):
    name = f"MacCV-{version}-{arch}.dmg"
    if name not in names:
        print(f"  warning: {tag} has no {name}", file=sys.stderr)
        continue

    digest = sums.get(name)
    if not digest:
        print(f"  warning: no digest for {name} in SHA256SUMS", file=sys.stderr)
        continue

    # Size is read from the release metadata, which GitHub recorded at upload time.
    size = next((asset["size"] for asset in assets if asset["name"] == name), 0)
    downloads.append(
        {
            "arch": arch,
            "url": f"https://github.com/{REPO}/releases/download/{tag}/{name}",
            "sha256": digest,
            "size": size,
        }
    )

if len(downloads) != 2:
    print(
        f"  refusing to write a feed with {len(downloads)} of 2 architectures: an Intel "
        "Mac would be handed the wrong build",
        file=sys.stderr,
    )
    sys.exit(1)

# ---------------------------------------------------------------------------
# Carry forward the revocation list
#
# It accumulates across releases, so it must survive a regeneration. Read from the
# deployed feed if it is reachable, and from any local copy otherwise.
# ---------------------------------------------------------------------------

revoked: list[str] = []
existing = root / "docs" / "latest.json"
if existing.exists():
    try:
        revoked = json.loads(existing.read_text(encoding="utf-8")).get("revoked", [])
    except json.JSONDecodeError:
        revoked = []

if not revoked:
    deployed = subprocess.run(
        [
            "/usr/bin/curl",
            "--silent",
            "--location",
            "--max-time",
            "20",
            f"https://{REPO.split('/')[0]}.github.io/{REPO.split('/')[1]}/latest.json",
        ],
        capture_output=True,
        text=True,
    )
    if deployed.returncode == 0 and deployed.stdout.lstrip().startswith("{"):
        try:
            revoked = json.loads(deployed.stdout).get("revoked", [])
        except json.JSONDecodeError:
            revoked = []

# ---------------------------------------------------------------------------

feed = {
    "version": version,
    "tag": tag,
    "download_url": downloads[0]["url"],
    "sha256": downloads[0]["sha256"],
    "size": downloads[0]["size"],
    "minimum_system_version": "15.0",
    "prerelease": False,
    "published_at": newest["publishedAt"],
    "notes": changelog_notes(version),
    "revoked": revoked,
    "downloads": downloads,
}

# The engine refuses a manifest it cannot verify, so a feed that fails these would stop
# every installed copy from updating. Fail the deploy instead of publishing one.
problems: list[str] = []
for entry in downloads:
    if not re.fullmatch(r"[0-9a-f]{64}", entry["sha256"]):
        problems.append(f"{entry['arch']}: sha256 is not a 64-character hex digest")
    if not entry["url"].startswith("https://"):
        problems.append(f"{entry['arch']}: url is not HTTPS")
    if not isinstance(entry["size"], int) or entry["size"] <= 0:
        problems.append(f"{entry['arch']}: size must be positive, got {entry['size']!r}")
if problems:
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)

target = root / "docs" / "latest.json"
target.write_text(json.dumps(feed, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
os_chdir_done = True

print(f"  wrote {target.relative_to(root)}: {version}, {len(downloads)} architectures")
if revoked:
    print(f"  revoked: {', '.join(revoked)}")
