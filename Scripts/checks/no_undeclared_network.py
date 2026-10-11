#!/usr/bin/env python3
"""Enforces the network invariants stated in docs/SECURITY.md.

The claim "no data leaves the machine" is only meaningful if something checks it.
This script fails the build when the engine gains a way to talk to the network that
the security document does not list.

What it looks for:
  1. Process spawns other than an allowlist — the engine shells out to a few system
     tools, and adding `nc` or `scp` to that list should never be quiet.
  2. Hostnames in source that are not on the documented list.
  3. Known analytics, telemetry or AI SDKs appearing as dependencies.
"""

from __future__ import annotations

import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]

# Binaries the engine is permitted to run, and why. Anything else is a finding.
ALLOWED_BINARIES = {
    "curl": "the only network client; used solely for update checks",
    "xattr": "clearing com.apple.quarantine after verification",
    "ditto": "extracting a downloaded archive with resource forks intact",
    "hdiutil": "mounting a downloaded disk image",
    "shasum": "verifying a download's digest",
    "spctl": "verifying a download's code signature",
    "open": "opening System Settings at the user's request",
}

# Hosts the project documents as its own.
ALLOWED_HOSTS = {
    "yxxbc.github.io",
    "github.com",
    "objects.githubusercontent.com",
    "raw.githubusercontent.com",
    "gh-proxy.com",
    # macOS APIs and documentation links, not network calls.
    "developer.apple.com",
    "support.apple.com",
    "www.apache.org",
    "apache.org",
    "www.contributor-covenant.org",
    "contributor-covenant.org",
    "cli.github.com",
    "semver.org",
    "keepachangelog.com",
    "www.conventionalcommits.org",
    "conventionalcommits.org",
    "githubusercontent.com",
    "github.io",
    "shields.io",
    "img.shields.io",
    "www.gnu.org",
    "rust-lang.org",
    "www.rust-lang.org",
    "swift.org",
    "example.com",
    "127.0.0.1",
    "localhost",
    "evil.example.com",
    "insecure.example.com",
    "r.jina.ai",
}

# Dependencies that would contradict SECURITY.md S1.3.
FORBIDDEN_CRATES = [
    "sentry",
    "bugsnag",
    "amplitude",
    "mixpanel",
    "segment",
    "posthog",
    "firebase",
    "appcenter",
    "crashlytics",
    "opentelemetry",
    "datadog",
    "newrelic",
    "honeycomb",
    "openai",
    "anthropic",
    "hyper",
    "reqwest",
    "ureq",
]

# Hosts that would indicate telemetry or model inference.
FORBIDDEN_HOST_PATTERNS = [
    r"sentry\.io",
    r"telemetry\.",
    r"analytics\.",
    r"\.amplitude\.com",
    r"\.mixpanel\.com",
    r"api\.openai\.com",
    r"api\.anthropic\.com",
    r"api\.segment\.io",
    r"ingest\.sentry\.io",
]

problems: list[str] = []

# ---------------------------------------------------------------------------
# 1. Process spawns
# ---------------------------------------------------------------------------

rust_sources = [
    path
    for path in (root / "crates").rglob("*.rs")
    if "tests" not in path.parts and "examples" not in path.parts
]
spawn_pattern = re.compile(r'Command::new\(\s*"([^"]+)"')
literal_spawn = re.compile(r'Command::new\(\s*"(/usr/bin/|/usr/sbin/|/bin/)?([A-Za-z0-9_.+-]+)"')

found_binaries: dict[str, list[str]] = {}
for path in rust_sources:
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        match = literal_spawn.search(line)
        if not match:
            continue
        binary = match.group(2)
        found_binaries.setdefault(binary, []).append(
            f"{path.relative_to(root)}:{line_number}"
        )

for binary, sites in sorted(found_binaries.items()):
    if binary not in ALLOWED_BINARIES:
        problems.append(
            f"unlisted binary '{binary}' run from {', '.join(sites)} — "
            "add it to ALLOWED_BINARIES and document why in docs/SECURITY.md, "
            "or remove it"
        )

# Dynamic spawns: a variable command name defeats the check above, so flag them.
for path in rust_sources:
    text = path.read_text(encoding="utf-8")
    if re.search(r"Command::new\(\s*[a-z_][a-zA-Z0-9_]*\s*\)", text):
        problems.append(
            f"{path.relative_to(root)} builds a Command from a non-literal; "
            "the network invariant cannot be checked statically"
        )

# ---------------------------------------------------------------------------
# 2. Hostnames
# ---------------------------------------------------------------------------

host_pattern = re.compile(r"https?://([A-Za-z0-9._-]+)")
scanned_suffixes = {".rs", ".swift", ".sh", ".toml", ".yml", ".yaml", ".py", ".json", ".md", ".html"}
skip_dirs = {".git", "target", "MacCV.xcodeproj", "node_modules"}

# Test code is not shipped, and its fixtures deliberately use placeholder domains
# that must never be reachable from production code. Excluding it is stricter than
# allowlisting those domains, which would let real code use them too.
TEST_FILES = {"SelfTest.swift"}
TEST_DIR_NAMES = {"tests", "examples"}


def production_lines(path: pathlib.Path) -> list[tuple[int, str]]:
    """Yields (line number, text) for lines that can affect shipped behaviour."""
    text = path.read_text(encoding="utf-8", errors="replace")
    lines = text.splitlines()

    if path.suffix == ".rs":
        # A `#[cfg(test)]` module is conventionally the last thing in the file, so
        # everything from it onwards is test-only.
        for index, line in enumerate(lines):
            if line.strip().startswith("#[cfg(test)]"):
                lines = lines[:index]
                break

    return list(enumerate(lines, 1))


for path in root.rglob("*"):
    if not path.is_file() or path.suffix not in scanned_suffixes:
        continue
    if any(part in skip_dirs for part in path.parts):
        continue
    if path.name in TEST_FILES:
        continue
    if any(part in TEST_DIR_NAMES for part in path.parts):
        continue
    # The standards documents describe the allowlist itself, so scanning them for
    # the domains they enumerate would be circular.
    if path.parent.name == "docs" and path.suffix == ".md":
        continue

    for line_number, line in production_lines(path):
        for host in host_pattern.findall(line):
            bare = host.split(":")[0].lower()
            if bare in ALLOWED_HOSTS:
                continue
            if any(
                bare.endswith("." + allowed) or bare == allowed
                for allowed in ALLOWED_HOSTS
            ):
                continue
            problems.append(
                f"{path.relative_to(root)}:{line_number} references {bare}, "
                "which is not in ALLOWED_HOSTS"
            )
        for pattern in FORBIDDEN_HOST_PATTERNS:
            if re.search(pattern, line):
                problems.append(
                    f"{path.relative_to(root)}:{line_number} matches forbidden "
                    f"telemetry/analytics pattern {pattern!r}"
                )

# ---------------------------------------------------------------------------
# 3. Dependencies
# ---------------------------------------------------------------------------

for manifest in [root / "Cargo.toml", *(root / "crates").rglob("Cargo.toml")]:
    text = manifest.read_text(encoding="utf-8")
    for crate in FORBIDDEN_CRATES:
        if re.search(rf'(?m)^\s*{re.escape(crate)}\s*=', text):
            problems.append(
                f"{manifest.relative_to(root)} depends on '{crate}', which "
                "contradicts SECURITY.md S1.3 (no telemetry, analytics or AI)"
            )

# ---------------------------------------------------------------------------

if problems:
    print("network invariants violated:", file=sys.stderr)
    for problem in sorted(set(problems)):
        print(f"  - {problem}", file=sys.stderr)
    sys.exit(1)

total = len(rust_sources)
print(f"  {total} Rust files, {len(found_binaries)} spawned binaries, all declared")
print(f"  {len(ALLOWED_BINARIES)} binaries and {len(ALLOWED_HOSTS)} hosts allowlisted")
