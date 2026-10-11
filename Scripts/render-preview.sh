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

# Rendered at 2x and downsampled. The raw capture is 1800×1240 and about 2.2 MB, far too
# heavy for an image that loads on every page view of the repository — this is the width
# a README column actually displays, and rendering at 2x first keeps it sharp on Retina.
TARGET_WIDTH=1000

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
  local language="$1" output="$2" label="$3"
  local raw="${output%.png}-2x.png"

  echo "==> rendering $label ($language)"
  # `--render-preview <path> <language> <appearance>`; exits without starting the UI.
  "$BINARY" --render-preview "$raw" "$language" dark 2>/dev/null

  if [[ ! -s "$raw" ]]; then
    echo "error: $raw was not produced" >&2
    exit 1
  fi

  python3 "$REPO_ROOT/Scripts/dmg/check-preview.py" "$raw" "$output" "$TARGET_WIDTH"
  rm -f "$raw"
}

render "en" "assets/preview-en.png" "English"
render "zh-Hans" "assets/preview-zh.png" "简体中文"

echo
ls -lh assets/preview-*.png | awk '{print "  " $9 "  " $5}'
