#!/usr/bin/env python3
"""Checks the two READMEs agree on the facts they both state.

A bilingual README drifts quietly: one language keeps a number the other updated,
or a claim gets softened in translation. The prose may differ — it should, or the
translation reads like a translation — but the load-bearing facts may not.
"""

from __future__ import annotations

import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]
en = (root / "README.md").read_text(encoding="utf-8")
zh = (root / "README.zh-CN.md").read_text(encoding="utf-8")

# (label, needle in English, needle in Chinese). Numbers are identical in both by
# construction; the prose around them is not.
SHARED_FACTS = [
    ("macOS minimum", "macOS 15", "macOS 15"),
    ("change detection", "0.53 µs", "0.53 µs"),
    ("read latency", "9.9 µs", "9.9 µs"),
    ("bundle size", "1.9 MB", "1.9 MB"),
    ("engine linkage", "static", "静态库"),
    ("licence", "Apache-2.0", "Apache-2.0"),
    ("objc2", "objc2", "objc2"),
    ("the honest limit", "any application on your Mac can read", "任何应用都能读取"),
    ("secure paste", "Paste Without Clipboard", "不经过剪贴板粘贴"),
    ("no AI", "No AI ever reads", "AI 永不读取"),
    ("no disk history", "never written to disk", "从不写入磁盘"),
    ("no telemetry", "no telemetry", "没有遥测"),
    ("cross-link", "README.zh-CN.md", "README.md"),
    ("website", "yxxbc.github.io", "yxxbc.github.io"),
    ("security doc", "docs/SECURITY.md", "docs/SECURITY.md"),
    # The preview differs by language on purpose — a Chinese README showing an English
    # panel would read as an untranslated app.
    ("preview image", "assets/preview-en.png", "assets/preview-zh.png"),
]

problems: list[str] = []
for label, en_needle, zh_needle in SHARED_FACTS:
    if en_needle not in en:
        problems.append(f"English README is missing: {label} ('{en_needle}')")
    if zh_needle not in zh:
        problems.append(f"Chinese README is missing: {label} ('{zh_needle}')")

# The badge block must be centred in both, or one renders left-aligned.
for name, text in (("README.md", en), ("README.zh-CN.md", zh)):
    if '<div align="center">' not in text:
        problems.append(f"{name} has no centred block for the badges and logo")

# A differing section count means one language has gone stale.
en_sections = len(re.findall(r"^## ", en, re.M))
zh_sections = len(re.findall(r"^## ", zh, re.M))
if en_sections != zh_sections:
    problems.append(
        f"section counts differ: English {en_sections}, Chinese {zh_sections}"
    )

if problems:
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)

print(f"  {len(SHARED_FACTS)} shared facts present in both, {en_sections} sections each")
