#!/usr/bin/env python3
"""Fails if a file the build needs is excluded by .gitignore.

This is a specific failure that has already happened: `MacCV-Info.plist` was ignored
while `project.yml` referenced it, so every build worked locally — the file was
present — and `xcodebuild` failed in CI with "Build input file cannot be found". The
file simply was not in the repository.

`.gitignore` and the build are two descriptions of the same tree, and nothing else
compares them. This does.
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[2]

# Files the build reads and which must therefore be committed, not ignored.
REQUIRED = [
    "apps/mac/MacCV-Info.plist",
    "apps/mac/project.yml",
    "crates/maccv-ffi/include/maccv.h",
    "crates/maccv-ffi/cbindgen.toml",
    "Scripts/build-app.sh",
    "Scripts/build-rust.sh",
    "Scripts/make-dmg.sh",
    "Scripts/verify.sh",
    "apps/mac/Resources/Localizable.xcstrings",
]

problems: list[str] = []

for relative in REQUIRED:
    path = root / relative

    if not path.exists():
        problems.append(f"{relative} does not exist")
        continue

    # `--no-index` is essential: without it `git check-ignore` skips files that are
    # already tracked, so a pattern that *is* ignoring the path reports as not
    # ignored — the check would pass while the hazard was present. Verified by
    # restoring the exclusion and watching this fail.
    ignored = subprocess.run(
        ["git", "check-ignore", "--no-index", "-q", relative],
        cwd=root,
        capture_output=True,
    )
    if ignored.returncode == 0:
        problems.append(
            f"{relative} is required by the build but excluded by .gitignore — "
            "it will be missing from a fresh clone"
        )

    # Also confirm it is actually tracked, since a file can be present and ignored.
    tracked = subprocess.run(
        ["git", "ls-files", "--error-unmatch", relative],
        cwd=root,
        capture_output=True,
    )
    if tracked.returncode != 0:
        problems.append(f"{relative} exists but is not tracked by git")

# The generated header is excluded on purpose in some projects; here it is committed so
# the C smoke test and Xcode builds work without a cbindgen install. Guard that comment.
gitignore = (root / ".gitignore").read_text(encoding="utf-8")
if "maccv.h" in gitignore and "NOT ignored" not in gitignore:
    line = next(
        (l for l in gitignore.splitlines() if "maccv.h" in l and not l.strip().startswith("#")),
        "",
    )
    if line and not line.strip().startswith("#"):
        problems.append(
            f".gitignore excludes the generated header ({line.strip()}), which is "
            "committed deliberately so builds need no cbindgen"
        )

print(f"  {len(REQUIRED)} build inputs checked for being tracked and not ignored")

if problems:
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)

print("  every build input is committed and not ignored")
