#!/usr/bin/env python3
"""Asserts the version is recorded identically everywhere it appears.

A version that disagrees with itself only surfaces after publishing, when auto-update
stops working for everyone who installed the app. Cargo.toml is the authority; the
rest are checked against it, including the *built* Info.plist where one is available.

The plist uses $(MARKETING_VERSION) substitution rather than a literal, so the check
verifies both the substitution exists and that Xcode resolved it to the same value.
"""

from __future__ import annotations

import pathlib
import plistlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (root / path).read_text(encoding="utf-8")


def cargo_version() -> str:
    text = read("Cargo.toml")
    section = text.split("[workspace.package]", 1)[1]
    match = re.search(r'^version\s*=\s*"([^"]+)"', section, re.M)
    if not match:
        print("Cargo.toml has no workspace version", file=sys.stderr)
        sys.exit(1)
    return match.group(1)


def project_setting(key: str) -> str:
    match = re.search(rf'{key}:\s*"([^"]+)"', read("apps/mac/project.yml"))
    return match.group(1) if match else ""


authority = cargo_version()
records: dict[str, str] = {
    "Cargo.toml": authority,
    "project.yml MARKETING_VERSION": project_setting("MARKETING_VERSION"),
}

# The committed plist must reference the build setting rather than hard-coding a
# version, or bump-version.sh cannot keep it in step.
plist_text = read("apps/mac/MacCV-Info.plist")
if "$(MARKETING_VERSION)" in plist_text:
    records["Info.plist substitutes MARKETING_VERSION"] = authority
else:
    records["Info.plist substitutes MARKETING_VERSION"] = "(no substitution)"

# If a build exists, check what Xcode actually produced — that is what a user reads.
#
# Only the most recently modified build is checked. Builds accumulate in /tmp and
# dist/, and an older artifact would report the state of the source at the time it was
# built rather than now — a false failure if the fix came later, a false pass if the
# regression did. Newest is the only one that reflects the current tree.
candidates = list(pathlib.Path("/tmp").glob("*/MacCV.app/Contents/Info.plist"))
candidates += list((root / "dist").glob("*/MacCV.app/Contents/Info.plist"))

# Informational only. verify.sh runs this stage *before* the app is built, so the
# absence of a build is the normal case there and must not read as a disagreement —
# it was doing exactly that, failing the stage on a clean tree.
informational: list[str] = []
if not candidates:
    informational.append("no built bundle found (the check ran before the app build)")

built: list[pathlib.Path] = []
if candidates:
    newest = max(candidates, key=lambda path: path.stat().st_mtime)
    built = [newest]

for plist in built:
    label = plist.parents[2].name
    try:
        info = plistlib.loads(plist.read_bytes())
    except Exception as error:  # noqa: BLE001 - any read failure is reported, not raised
        records[f"plist {label}"] = f"unreadable: {error}"
        continue

    records[f"plist {label} version"] = info.get("CFBundleShortVersionString", "(absent)")

    # The icon key is asserted here because its absence shipped a release with no app
    # icon and *nothing failed*: the bundle simply had an .icns nobody referenced.
    # A standing check costs one line and closes that hole.
    icon = info.get("CFBundleIconFile")
    records[f"plist {label} icon"] = icon or "(absent — the app would have no icon)"
    if icon:
        resource = plist.parent / "Resources" / f"{icon}.icns"
        if not resource.exists():
            records[f"plist {label} icon file"] = f"(missing: {resource.name})"

problems: list[str] = []

for note in informational:
    print(f"  --   {note}")

for name, value in records.items():
    if name.endswith(" icon") or "icon file" in name:
        # Expected to be a name, not a version.
        good = bool(value) and "absent" not in value and "missing" not in value
        print(f"  {'ok  ' if good else 'FAIL'} {name:44s} {value}")
        if not good:
            problems.append(f"{name}: {value}")
    else:
        good = value == authority
        print(f"  {'ok  ' if good else 'FAIL'} {name:44s} {value or '(absent)'}")
        if not good:
            problems.append(f"{name} says {value}, expected {authority}")

if problems:
    print("", file=sys.stderr)
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)
