#!/usr/bin/env python3
"""Asserts the version is recorded identically everywhere.

A version that disagrees with itself only surfaces after publishing, when
auto-update stops working for everyone who installed the app. Cargo.toml is the
authority; the rest are checked against it.
"""

import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]


def cargo_version() -> str:
    text = (root / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[workspace.package]", 1)[1]
    match = re.search(r'^version\s*=\s*"([^"]+)"', section, re.M)
    if not match:
        print("Cargo.toml has no workspace version")
        sys.exit(1)
    return match.group(1)


def project_yml(key: str) -> str:
    text = (root / "apps/mac/project.yml").read_text(encoding="utf-8")
    match = re.search(rf'{key}:\s*"([^"]+)"', text)
    return match.group(1) if match else ""


authority = cargo_version()
records = {
    "Cargo.toml": authority,
    "project.yml MARKETING_VERSION": project_yml("MARKETING_VERSION"),
    "project.yml CFBundleShortVersionString": project_yml("CFBundleShortVersionString"),
}

disagreements = {name: value for name, value in records.items() if value != authority}
for name, value in records.items():
    mark = "ok  " if value == authority else "FAIL"
    print(f"  {mark} {name:42s} {value or '(absent)'}")

if disagreements:
    print(f"\nexpected every record to say {authority}", file=sys.stderr)
    sys.exit(1)
