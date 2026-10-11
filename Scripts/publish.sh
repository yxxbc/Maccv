#!/usr/bin/env bash
#
# Publishes MacCV to GitHub: creates the repository, configures it, and pushes.
#
# This is deliberately a separate, manually run script rather than something a
# build step does. Creating a public repository is an outward-facing action on
# your account, so it should be a decision you make explicitly.
#
# Usage:
#   Scripts/publish.sh                 # create + push, using the defaults below
#   Scripts/publish.sh --private       # create a private repository instead
#   Scripts/publish.sh --description "…"
#
# Prerequisites:
#   - `gh auth status` shows you as logged in, with the `repo` scope
#   - The working tree is committed

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

OWNER="yxxbc"
NAME="Maccv"
VISIBILITY="--public"
DESCRIPTION="A fast, faithful macOS clipboard manager. Rust engine, SwiftUI front end, full NSPasteboard fidelity."
HOMEPAGE="https://github.com/${OWNER}/${NAME}"

# The shape of the repository's About panel.
TOPICS=(
  macos
  clipboard
  clipboard-manager
  nspasteboard
  rust
  rust-lang
  swift
  swiftui
  appkit
  ffi
  macos-app
  objc2
)

while [[ $# -gt 0 ]]; do
  case "$1" in
    --private) VISIBILITY="--private"; shift ;;
    --public) VISIBILITY="--public"; shift ;;
    --description) DESCRIPTION="$2"; shift 2 ;;
    --owner) OWNER="$2"; HOMEPAGE="https://github.com/$2/${NAME}"; shift 2 ;;
    -h|--help) sed -n '2,18p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# ---------------------------------------------------------------------------
# Preconditions
# ---------------------------------------------------------------------------

if ! command -v gh >/dev/null 2>&1; then
  echo "error: the GitHub CLI is not installed. See https://cli.github.com" >&2
  exit 1
fi

if ! gh auth status >/dev/null 2>&1; then
  echo "error: not logged in to GitHub. Run 'gh auth login' first." >&2
  exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: there are uncommitted changes. Commit them first:" >&2
  git status --short >&2
  exit 1
fi

if [[ ! -f README.md || ! -f LICENSE ]]; then
  echo "error: README.md or LICENSE is missing" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Create the repository
# ---------------------------------------------------------------------------

FULL_NAME="${OWNER}/${NAME}"

if gh repo view "$FULL_NAME" >/dev/null 2>&1; then
  echo "==> ${FULL_NAME} already exists; will push into it"
else
  echo "==> creating ${FULL_NAME} ${VISIBILITY#--}"
  gh repo create "$FULL_NAME" "$VISIBILITY" \
    --description "$DESCRIPTION" \
    --homepage "$HOMEPAGE"
fi

# `gh repo create` can add a remote itself; make sure it is correct either way.
if git remote get-url origin >/dev/null 2>&1; then
  git remote set-url origin "https://github.com/${FULL_NAME}.git"
else
  git remote add origin "https://github.com/${FULL_NAME}.git"
fi

echo "==> pushing main"
git push -u origin main

# ---------------------------------------------------------------------------
# Configure the About panel
# ---------------------------------------------------------------------------

echo "==> setting description and homepage"
gh repo edit "$FULL_NAME" \
  --description "$DESCRIPTION" \
  --homepage "$HOMEPAGE"

echo "==> setting topics"
# Space-separated in one argument, as the API expects.
gh repo edit "$FULL_NAME" --add-topic "${TOPICS[*]// /,}"

# ---------------------------------------------------------------------------
# Social preview
#
# GitHub exposes no API for the social preview image, so this stays manual.
# ---------------------------------------------------------------------------

SOCIAL="$REPO_ROOT/assets/social-preview.png"
echo
echo "==> repository is live: https://github.com/${FULL_NAME}"
echo
echo "One step is still manual, because GitHub has no API for it:"
echo "  1. Open https://github.com/${FULL_NAME}/settings"
echo "  2. Social preview → Upload an image → ${SOCIAL}"
echo
echo "Verify:"
echo "  gh repo view ${FULL_NAME} --web"
echo "  gh run list --repo ${FULL_NAME}    # CI should start on the pushed commit"
