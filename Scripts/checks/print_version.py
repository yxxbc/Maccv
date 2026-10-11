#!/usr/bin/env python3
"""Prints the workspace version from Cargo.toml.

Separate from version_consistency.py so a shell caller can compare values without
parsing Python output.
"""

import pathlib
import re
import sys

text = (pathlib.Path(__file__).resolve().parents[2] / "Cargo.toml").read_text(encoding="utf-8")
section = text.split("[workspace.package]", 1)[1]
match = re.search(r'^version\s*=\s*"([^"]+)"', section, re.M)
if not match:
    print("Cargo.toml has no workspace version", file=sys.stderr)
    sys.exit(1)
print(match.group(1))
