#!/usr/bin/env python3
"""Checks the *built* app bundle: version, icon key, and that both are real.

Runs after the app build, unlike `version_consistency.py`, which checks the sources
and has to run early. Keeping them separate is not tidiness: doing this in the early
check meant reading whatever bundle happened to be on disk — usually from a previous
release — and reporting its version as if it were the current tree's.

What is asserted, and why each has bitten:

- The resolved `CFBundleShortVersionString` matches the sources. The plist substitutes
  `$(MARKETING_VERSION)`, so a broken substitution would ship the wrong version while
  every source file agreed with itself.
- `CFBundleIconFile` is present **and** the `.icns` it names is in the bundle. Its
  absence shipped a release with the generic blank document icon, and nothing failed:
  the bundle had an icon nobody referenced.
- The deployment target is what the README promises.
"""

from __future__ import annotations

import pathlib
import plistlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]
problems: list[str] = []


def source_version() -> str:
    text = (root / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[workspace.package]", 1)[1]
    match = re.search(r'^version\s*=\s*"([^"]+)"', section, re.M)
    if not match:
        print("Cargo.toml has no workspace version", file=sys.stderr)
        sys.exit(1)
    return match.group(1)


def newest_bundle() -> pathlib.Path | None:
    """The most recently built bundle, wherever it was put.

    Only the newest: older ones reflect the tree as it was when they were built, so
    reporting on them gives a failure (or a pass) about a state that no longer exists.
    """
    candidates = list(pathlib.Path("/tmp").glob("*/MacCV.app"))
    candidates += list((root / "dist").glob("*/MacCV.app"))
    if not candidates:
        return None
    return max(candidates, key=lambda path: path.stat().st_mtime)


expected = source_version()
bundle = newest_bundle()

if bundle is None:
    print("  --   no built bundle found; run Scripts/build-app.sh")
    sys.exit(0)

print(f"  bundle {bundle}")

plist_path = bundle / "Contents" / "Info.plist"
try:
    info = plistlib.loads(plist_path.read_bytes())
except Exception as error:  # noqa: BLE001 - any failure is reported, not raised
    print(f"  FAIL unreadable Info.plist: {error}", file=sys.stderr)
    sys.exit(1)

# --- version -----------------------------------------------------------------
actual = info.get("CFBundleShortVersionString", "")
if actual == expected:
    print(f"  ok   CFBundleShortVersionString              {actual}")
else:
    problems.append(f"the bundle says {actual!r} but the sources say {expected!r}")

# --- build number ------------------------------------------------------------
build = info.get("CFBundleVersion")
if build:
    print(f"  ok   CFBundleVersion                         {build}")
else:
    problems.append("CFBundleVersion is absent; an in-place update would not be seen as newer")

# --- icon --------------------------------------------------------------------
icon = info.get("CFBundleIconFile")
if not icon:
    problems.append(
        "CFBundleIconFile is absent, so the app will show the generic blank icon "
        "even though the bundle carries an .icns"
    )
else:
    resource = bundle / "Contents" / "Resources" / f"{icon}.icns"
    if resource.exists():
        size = resource.stat().st_size
        print(f"  ok   CFBundleIconFile -> {resource.name} ({size} bytes)")
        if size < 1024:
            problems.append(f"{resource.name} is only {size} bytes; that is not a real icon")
    else:
        problems.append(f"CFBundleIconFile names {icon}.icns, which is not in the bundle")

# --- deployment target -------------------------------------------------------
minimum = info.get("LSMinimumSystemVersion", "")
if minimum == "15.0":
    print(f"  ok   LSMinimumSystemVersion                  {minimum}")
elif not minimum:
    problems.append("LSMinimumSystemVersion is absent")
else:
    problems.append(f"LSMinimumSystemVersion is {minimum}, but the README promises 15.0")

# --- menu-bar accessory ------------------------------------------------------
# The app must not appear in the Dock; a stray Dock icon is an immediate regression a
# user notices, and it is one plist key.
if info.get("LSUIElement"):
    print("  ok   LSUIElement                             true (menu-bar only)")
else:
    problems.append("LSUIElement is not set, so MacCV would appear in the Dock")

# --- the privacy string macOS shows ------------------------------------------
if info.get("NSPasteboardUsageDescription"):
    print("  ok   NSPasteboardUsageDescription            present")
else:
    problems.append(
        "NSPasteboardUsageDescription is absent; the system's pasteboard consent "
        "prompt would have nothing to explain the request"
    )

if problems:
    print("", file=sys.stderr)
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)
