#!/usr/bin/env bash
#
# Extracts the release notes for one version out of CHANGELOG.md.
#
# The changelog is the single source of truth for release notes: whatever is
# written under `## [X.Y.Z]` is exactly what the GitHub Release gets. Keeping the
# extraction here means a human can run it and see the notes before tagging.
#
# Usage:
#   Scripts/changelog-notes.sh 0.1.0          # notes for 0.1.0
#   Scripts/changelog-notes.sh --latest       # the highest released version
#   Scripts/changelog-notes.sh --check 0.1.0  # just verify a section exists
#
# Exit codes:
#   0  notes printed (or the section exists, with --check)
#   1  no matching section

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANGELOG="$REPO_ROOT/CHANGELOG.md"

if [[ ! -f "$CHANGELOG" ]]; then
  echo "error: $CHANGELOG not found" >&2
  exit 1
fi

CHECK_ONLY=0
VERSION=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check) CHECK_ONLY=1; shift ;;
    --latest) VERSION="--latest"; shift ;;
    -h|--help) sed -n '2,18p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) VERSION="$1"; shift ;;
  esac
done

# Strip a leading `v` so tag `v0.1.0` and heading `## [0.1.0]` agree.
VERSION="${VERSION#v}"

python3 - "$CHANGELOG" "$VERSION" "$CHECK_ONLY" <<'PY'
import re
import sys

path, wanted, check_only = sys.argv[1], sys.argv[2], sys.argv[3] == "1"
text = open(path, encoding="utf-8").read()
lines = text.splitlines()

# A version heading looks like: `## [0.1.0] - 2026-10-11`
heading = re.compile(r"^##\s+\[(?P<version>[^\]]+)\](?:\s*-\s*(?P<date>.*))?\s*$")

sections = []
for index, line in enumerate(lines):
    match = heading.match(line)
    if match:
        sections.append((index, match.group("version").strip(), (match.group("date") or "").strip()))

released = [(i, v, d) for i, v, d in sections if v.lower() != "unreleased"]

if wanted == "--latest":
    if not released:
        print("error: no released version section found in CHANGELOG.md", file=sys.stderr)
        sys.exit(1)
    # Take the first released section: the changelog is newest-first.
    index, wanted, date = released[0]
else:
    match = next(((i, v, d) for i, v, d in sections if v == wanted), None)
    if match is None:
        known = ", ".join(v for _, v, _ in sections) or "none"
        print(
            f"error: CHANGELOG.md has no section for version {wanted!r}.\n"
            f"       sections present: {known}",
            file=sys.stderr,
        )
        sys.exit(1)
    index, wanted, date = match

if check_only:
    print(f"ok: CHANGELOG.md has a section for {wanted}" + (f" ({date})" if date else ""))
    sys.exit(0)

# Body runs to the next heading at the same level (or lower), exclusive.
body_start = index + 1
body_end = len(lines)
for other_index, _, _ in sections:
    if other_index > index:
        body_end = other_index
        break

body = lines[body_start:body_end]

# Drop leading and trailing blank lines, and any trailing link-reference block.
while body and not body[0].strip():
    body.pop(0)
while body and not body[-1].strip():
    body.pop()

notes = "\n".join(body).strip()

if not notes:
    print(
        f"error: the CHANGELOG.md section for {wanted} is empty. Add entries before releasing.",
        file=sys.stderr,
    )
    sys.exit(1)

print(notes)
PY
