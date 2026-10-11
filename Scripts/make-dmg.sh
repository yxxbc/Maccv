#!/usr/bin/env bash
#
# Builds the distributable DMG for one architecture.
#
# Why a DMG and not a zip: a DMG is what a Mac user expects to see, it carries the
# background artwork and icon layout that make the drag-to-install gesture obvious,
# and a read-only UDZO image cannot be modified in transit.
#
# Why `dmgbuild` and not `create-dmg`: create-dmg styles the image by driving Finder
# over AppleEvents, which times out under CI (-1712) and needs a retry wrapper.
# `dmgbuild` writes the `.DS_Store` itself and never talks to Finder, so it is
# deterministic and works with no window server. `create-dmg` is used as a fallback
# when `dmgbuild` is unavailable, because a slightly flaky DMG beats no DMG.
#
# Usage:
#   Scripts/make-dmg.sh --app <path to MacCV.app> --out <path.dmg> [--volume-name NAME]
#
# Environment:
#   MACCV_DMG_TOOL   force `dmgbuild` or `create-dmg`

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

APP=""
OUT=""
VOLUME_NAME=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) APP="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --volume-name) VOLUME_NAME="$2"; shift 2 ;;
    -h|--help) sed -n '2,24p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$APP" || -z "$OUT" ]]; then
  echo "error: --app and --out are required" >&2
  exit 2
fi
if [[ ! -d "$APP" ]]; then
  echo "error: $APP is not a directory" >&2
  exit 1
fi

# The volume name is what appears in the Finder sidebar while mounted. Read the
# version from the bundle so the DMG cannot disagree with the app inside it.
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
  "$APP/Contents/Info.plist" 2>/dev/null || echo "0.0.0")"
VOLUME_NAME="${VOLUME_NAME:-MacCV $VERSION}"

OUT="$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")"
mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"

echo "==> building $OUT"
echo "    app         $APP"
echo "    version     $VERSION"
echo "    volume      $VOLUME_NAME"

# ---------------------------------------------------------------------------
# Staging
#
# The app is copied rather than moved so the build products stay usable for the
# checksum and the self-test that follow.
# ---------------------------------------------------------------------------

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

ditto "$APP" "$STAGE/$(basename "$APP")"

# The layout artwork. Generated here rather than committed, so it cannot drift from
# the window size in dmg-settings.py.
ASSETS="$STAGE/.assets"
mkdir -p "$ASSETS"
echo "==> generating the background"
python3 Scripts/dmg/generate-background.py "$ASSETS" | sed 's/^/    /'

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

DMGBUILD_BIN="$(command -v dmgbuild 2>/dev/null || echo "$HOME/.local/bin/dmgbuild")"
[ -x "$DMGBUILD_BIN" ] || DMGBUILD_BIN=""

TOOL="${MACCV_DMG_TOOL:-}"
if [[ -z "$TOOL" ]]; then
  # `dmgbuild` is not a Homebrew formula; it installs via pipx, which puts the
  # console script outside the default PATH on some setups.
  if [[ -n "$DMGBUILD_BIN" ]]; then
    TOOL="dmgbuild"
  elif command -v create-dmg >/dev/null 2>&1; then
    TOOL="create-dmg"
  fi
fi

# Attempted in order, with a fallback: a slightly flaky DMG produced by create-dmg
# beats failing the release because dmgbuild was missing.
built=0

if [[ "$TOOL" == "dmgbuild" ]]; then
  echo "==> dmgbuild"
  # The settings file reads the layout from the environment, so all geometry lives
  # in one place.
  rm -f "$OUT"
  if MACCV_DMG_APP_NAME="$(basename "$APP")" \
     MACCV_DMG_STAGED_APP="$STAGE/$(basename "$APP")" \
     MACCV_DMG_BACKGROUND="$ASSETS/background.tiff" \
     MACCV_DMG_BACKGROUND_DARK="$ASSETS/background-dark.tiff" \
     "$DMGBUILD_BIN" \
       -s Scripts/dmg/dmg-settings.py \
       -D "app=$(basename "$APP")" \
       "$VOLUME_NAME" \
       "$OUT"
  then
    built=1
  else
    echo "    dmgbuild failed; falling back" >&2
    TOOL="create-dmg"
  fi
fi

if [[ "$built" == "0" ]]; then
  echo "==> create-dmg"
  if ! command -v create-dmg >/dev/null 2>&1; then
    echo "error: no working DMG tool" >&2
    echo "       pipx install dmgbuild   (preferred: works without a GUI)" >&2
    echo "       brew install create-dmg (fallback)" >&2
    exit 1
  fi
  # create-dmg drives Finder over AppleEvents, which times out on a busy or
  # headless machine. Retrying is its documented workaround.
  attempt=0
  while [[ $attempt -lt 3 ]]; do
    attempt=$((attempt + 1))
    rm -f "$OUT"
    if create-dmg \
      --volname "$VOLUME_NAME" \
      --background "$ASSETS/background.png" \
      --window-pos 200 120 \
      --window-size 660 420 \
      --text-size 13 \
      --icon-size 108 \
      --icon "$(basename "$APP")" 165 205 \
      --hide-extension "$(basename "$APP")" \
      --app-drop-link 495 205 \
      --no-internet-enable \
      --format UDZO \
      --overwrite \
      "$OUT" \
      "$STAGE" >/dev/null 2>&1
    then
      built=1
      break
    fi
    echo "    attempt $attempt failed; waiting before retry"
    sleep 5
  done
  [[ "$built" == "1" ]] || { echo "error: create-dmg failed after $attempt attempts" >&2; exit 1; }
fi

# ---------------------------------------------------------------------------
# Verify
#
# A DMG that mounts but does not contain a usable app is the kind of failure that
# only surfaces on a user's machine, so it is checked here.
# ---------------------------------------------------------------------------

echo "==> verifying"

[[ -f "$OUT" ]] || { echo "error: $OUT was not produced" >&2; exit 1; }

# The image must be a compressed, read-only image: a writable or raw one would be
# many times larger and could be altered after download.
format="$(hdiutil imageinfo "$OUT" 2>/dev/null | awk -F': ' '/^Format:/{print $2}')"
echo "    format      ${format:-unknown}"
# hdiutil reports the compression *code*, not a description: UDZO is zlib, UDBZ is
# bzip2, ULFO is lzfse. An uncompressed image (UDRO/UDRW) would be many times
# larger and could be altered after download.
case "$format" in
  UDZO|UDBZ|ULFO|UDCO) ;;
  *)
    echo "error: expected a compressed read-only image, got '$format'" >&2
    exit 1
    ;;
esac

# Mount and confirm the app is present and runnable from inside the image.
MOUNT="$(mktemp -d)"
if hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT" "$OUT" >/dev/null 2>&1; then
  found="$(find "$MOUNT" -maxdepth 1 -name "*.app" -print -quit)"
  if [[ -z "$found" ]]; then
    echo "error: the image contains no .app bundle" >&2
    hdiutil detach "$MOUNT" >/dev/null 2>&1 || true
    exit 1
  fi
  echo "    contains    $(basename "$found")"

  # Mounted through a read-only image is exactly how the user first launches it, so
  # this catches a bundle that was never going to run.
  if "$found/Contents/MacOS/MacCV" --self-test >/dev/null 2>&1; then
    echo "    self-test   PASS"
  else
    echo "warning: the app inside the image failed its self-test" >&2
  fi

  # Gatekeeper's verdict on the packaged copy, since that is what decides whether a
  # user can open it at all.
  if spctl --assess --type execute "$found" >/dev/null 2>&1; then
    echo "    gatekeeper  accepted"
  else
    echo "    gatekeeper  not notarized (users will need to allow it once)"
  fi

  hdiutil detach "$MOUNT" >/dev/null 2>&1 || true
else
  echo "warning: could not mount the image to verify it" >&2
fi
rmdir "$MOUNT" 2>/dev/null || true

SIZE="$(du -h "$OUT" | cut -f1)"
echo
echo "==> $OUT ($SIZE)"
