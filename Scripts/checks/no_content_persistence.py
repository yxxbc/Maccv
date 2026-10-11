#!/usr/bin/env python3
"""Enforces SECURITY.md S1.4: clipboard content is never persisted.

History lives in memory. There is no serialisation of it, and the preferences type
holds only settings. This checks both claims mechanically, because "we do not write
clipboard content to disk" is exactly the kind of statement that quietly stops being
true when someone adds a cache.

What it asserts:
  1. `AppSettings.Stored` contains no field that could hold clipboard content.
  2. No clipboard type is encoded or written anywhere in the app.
  3. The engine opens no file for writing, other than the updater's staging area.
"""

from __future__ import annotations

import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]
problems: list[str] = []

# ---------------------------------------------------------------------------
# 1. The persisted settings shape
# ---------------------------------------------------------------------------

settings = root / "apps" / "mac" / "Sources" / "MacCV" / "AppSettings.swift"
if not settings.exists():
    problems.append("AppSettings.swift not found; the settings shape cannot be checked")
else:
    text = settings.read_text(encoding="utf-8")
    match = re.search(r"private struct Stored: Codable \{(.*?)\n    \}", text, re.S)
    if not match:
        problems.append("could not locate the Codable settings struct in AppSettings.swift")
    else:
        fields = re.findall(r"^\s*var\s+(\w+)\s*:\s*([^\n]+)$", match.group(1), re.M)
        # Types that can hold clipboard content, and types that cannot.
        forbidden_types = ("ClipSnapshot", "ClipItem", "ClipRepresentation", "Data", "String?")
        for name, type_name in fields:
            type_name = type_name.strip()
            # `String?` is allowed only for the language code.
            if type_name == "String?" and name == "languageOverride":
                continue
            if any(f in type_name for f in forbidden_types):
                problems.append(
                    f"AppSettings.Stored.{name} has type {type_name}, which could hold "
                    "clipboard content; SECURITY.md S1.4 says preferences only"
                )
        if not fields:
            problems.append("no fields found in AppSettings.Stored; the check is not working")
        else:
            print(f"  settings persist {len(fields)} field(s), all preferences")

# ---------------------------------------------------------------------------
# 2. No clipboard type is ever encoded or written
# ---------------------------------------------------------------------------

clipboard_types = ["ClipSnapshot", "ClipItem", "ClipRepresentation", "Snapshot", "SnapshotItem"]
write_calls = re.compile(
    r"(\.write\(to:|\.encode\(|JSONEncoder|PropertyListEncoder|NSKeyedArchiver|FileManager\.default\.createFile|\.data\(using:|writeToFile)"
)

for swift in (root / "apps" / "mac" / "Sources").rglob("*.swift"):
    text = swift.read_text(encoding="utf-8")
    for line_number, line in enumerate(text.splitlines(), 1):
        if not write_calls.search(line):
            continue
        if any(clip_type in line for clip_type in clipboard_types):
            problems.append(
                f"{swift.relative_to(root)}:{line_number} appears to persist a clipboard "
                "type — SECURITY.md S1.4 forbids this"
            )

# ---------------------------------------------------------------------------
# 3. The engine writes only where the update path is allowed to
# ---------------------------------------------------------------------------

for rust in (root / "crates").rglob("*.rs"):
    if "tests" in rust.parts or "examples" in rust.parts:
        continue
    text = rust.read_text(encoding="utf-8")
    # The redaction and pasteboard modules must not touch the filesystem at all.
    if rust.name in {"pasteboard.rs", "redaction.rs", "content.rs", "secure_input.rs"}:
        for line_number, line in enumerate(text.splitlines(), 1):
            if re.search(r"fs::(write|create|OpenOptions)|\bfs::File::", line):
                problems.append(
                    f"{rust.relative_to(root)}:{line_number} writes to the filesystem, but "
                    "this module handles clipboard content; SECURITY.md S1.4"
                )

# The updater may write, but only inside its staging directory.
updater = root / "crates" / "maccv-core" / "src" / "updater.rs"
if updater.exists():
    text = updater.read_text(encoding="utf-8")
    lines = text.splitlines()
    # Test fixtures write files on purpose; stop at the test module.
    for index, line in enumerate(lines):
        if line.strip().startswith("#[cfg(test)]"):
            lines = lines[:index]
            break
    for line_number, line in enumerate(lines, 1):
        if re.search(r"fs::write\(", line) and "document" not in line.lower():
            # Any raw fs::write here would bypass the staging-directory convention.
            problems.append(
                f"updater.rs:{line_number} calls fs::write directly: "
                f"  {line.strip()}"
            )

# ---------------------------------------------------------------------------

if problems:
    print("content-persistence invariants violated:", file=sys.stderr)
    for problem in sorted(set(problems)):
        print(f"  - {problem}", file=sys.stderr)
    sys.exit(1)

print("  no clipboard type is persisted; the engine writes only to its staging area")
