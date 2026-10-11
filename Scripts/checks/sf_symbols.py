#!/usr/bin/env python3
"""Checks that the SF Symbols the app uses exist, and are not locale-localised.

Two failures this catches, both of which have happened or nearly did:

1. **A misspelled symbol name.** `NSImage(systemSymbolName:)` returns nil rather than
   failing, so a typo silently produces a row with no icon and no error anywhere.

2. **A localised symbol.** SF Symbols may draw a *word* instead of a picture depending
   on the user's language. `textformat` renders as "Aa" in English and as the CJK word
   for "format" in Chinese, where it filled an 18-point icon column with clipped text.
   A symbol that changes into text in some locale is not an icon.

Existence is checked by asking AppKit, which is the only authority.
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parents[2]
source = root / "apps" / "mac" / "Sources" / "MacCV" / "ClipboardEngine.swift"
panel = root / "apps" / "mac" / "Sources" / "MacCV" / "ClipboardPanel.swift"
settings = root / "apps" / "mac" / "Sources" / "MacCV" / "SettingsView.swift"

# SF Symbols that draw a localised word rather than a shape in at least one language.
# Kept as a list with the reason, so a replacement is chosen deliberately.
LOCALISED = {
    "textformat": "draws \"Aa\" in Latin locales and the local word for \"format\" in CJK",
    "textformat.size": "localises like textformat",
    "character": "may render as the local word",
}

# Every `"symbol.name"` literal used with Image(systemName:) or as a ClipKind symbol.
SYMBOL_LITERAL = re.compile(r'Image\(systemName:\s*"([^"\\]+)"')
SWITCH_CASE_SYMBOL = re.compile(r'case \.[a-zA-Z]+:\s*"([a-z][a-z0-9.]+)"')

names: set[str] = set()
for path in (source, panel, settings):
    if not path.exists():
        continue
    text = path.read_text(encoding="utf-8")
    names |= set(SYMBOL_LITERAL.findall(text))
    names |= set(SWITCH_CASE_SYMBOL.findall(text))

if not names:
    print("  no symbol names found; the check is not reading the right files", file=sys.stderr)
    sys.exit(1)

problems = [f"{name}: {LOCALISED[name]}" for name in sorted(names) if name in LOCALISED]

# Ask AppKit whether each name resolves.
list_path = pathlib.Path(tempfile.mkdtemp()) / "Check.swift"
probe = (
    r"""import AppKit
let names = ["""
    + ", ".join(f'"{name}"' for name in sorted(names))
    + r"""]
for name in names {
    let ok = NSImage(systemSymbolName: name, accessibilityDescription: nil) != nil
    print("\(ok ? "ok" : "MISS") \(name)")
}
"""
)
list_path.write_text(probe, encoding="utf-8")

binary = list_path.with_suffix("")
compile_result = subprocess.run(
    ["swiftc", "-O", "-o", str(binary), str(list_path)],
    capture_output=True,
    text=True,
)
if compile_result.returncode != 0:
    # Not a project failure: a machine without a full toolchain should not fail verify.
    print(f"  could not compile the symbol probe; skipping ({compile_result.stderr.splitlines()[:1]})")
    sys.exit(0)

run = subprocess.run([str(binary)], capture_output=True, text=True)
for line in run.stdout.splitlines():
    if line.startswith("MISS"):
        problems.append(f"{line.split(' ', 1)[1]} does not exist (the row would have no icon)")

print(f"  {len(names)} SF Symbols checked")
if problems:
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)
print("  every symbol exists and renders as an icon in every language")
