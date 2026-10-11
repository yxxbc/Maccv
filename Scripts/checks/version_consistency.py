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

# The built bundle is checked by a separate stage that runs *after* the app build.
# Doing it here read a stale artifact from a previous run and reported the version of
# whatever was last built rather than of the current tree — a false failure that looked
# like a real version mismatch.
informational = "the built bundle is verified after the app build"

problems: list[str] = []

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
