#!/usr/bin/env bash
#
# Regenerates the README preview images from the app's own view code.
#
# Why not a screenshot: a screenshot has to be retaken by hand whenever the UI changes,
# so it silently goes stale — the image in a README is usually a release or two behind,
# and nobody notices because nobody looks at it after the first time.
#
# Rendering the real panel means the image cannot disagree with what the app draws, and
# it keeps two things out of the repository that a screenshot would put there: the
# developer's actual clipboard history, and their desktop wallpaper.
#
# Usage:
#   Scripts/render-preview.sh                 # both languages
#   Scripts/render-preview.sh --arch arm64    # a specific architecture's build
#
# Output: assets/preview-en.png, assets/preview-zh.png

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ARCH="native"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    -h|--help) sed -n '2,16p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Two framings, because the two places the image appears need different ones:
#
#   padded — for a README, shown at the full width of the column. A generous backdrop
#            reads well there.
#   tight  — for the website hero, where the panel sits beside the headline and is
#            therefore about half as wide. At the padded framing the panel's 12pt rows
#            shrink to roughly 6pt, which is legible in a design file and not on a screen.
#
# Both are rendered at 2x and downsampled: the raw capture is far too heavy to ship
# (2.2 MB), and rendering at 2x first keeps the result sharp on a Retina display.
README_WIDTH=1000
HERO_WIDTH=760

BUILD_DIR="dist/preview-$ARCH"
echo "==> building the app to render from"
Scripts/build-app.sh --arch "$ARCH" --out "$BUILD_DIR" >/dev/null

BINARY="$BUILD_DIR/MacCV.app/Contents/MacOS/MacCV"
if [[ ! -x "$BINARY" ]]; then
  echo "error: $BINARY was not built" >&2
  exit 1
fi

mkdir -p assets

# Each language is rendered by its own process: Foundation resolves the interface
# language once at launch, so one process cannot draw both.
render() {
  local language="$1" output="$2" width="$3" framing="$4" label="$5"
  local raw="${output%.png}-2x.png"

  echo "==> $label ($language, $framing)"
  # `--render-preview <path> <language> <appearance> <framing>`; exits without starting the UI.
  "$BINARY" --render-preview "$raw" "$language" dark "$framing" 2>/dev/null

  if [[ ! -s "$raw" ]]; then
    echo "error: $raw was not produced" >&2
    exit 1
  fi

  python3 "$REPO_ROOT/Scripts/dmg/check-preview.py" "$raw" "$output" "$width"
  rm -f "$raw"
}

for language in en zh-Hans; do
  case "$language" in
    en) suffix="en"; label="English" ;;
    zh-Hans) suffix="zh"; label="简体中文" ;;
  esac

  render "$language" "assets/preview-$suffix.png" "$README_WIDTH" padded "$label (README)"
  render "$language" "assets/preview-hero-$suffix.png" "$HERO_WIDTH" tight "$label (site hero)"
done

# The site serves its own copies: a relative path from docs/ cannot reach assets/.
cp assets/preview-hero-en.png docs/preview-hero-en.png
cp assets/preview-hero-zh.png docs/preview-hero-zh.png

echo
ls -lh assets/preview-*.png docs/preview-hero-*.png | awk '{print "  " $9 "  " $5}'
