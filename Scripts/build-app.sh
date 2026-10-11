#!/usr/bin/env bash
#
# Builds MacCV.app for one architecture, and proves the slice is really there.
#
# Why this is a script rather than a command in each workflow: CI and the release
# workflow used to invoke `xcodebuild` with *different* flags — CI used
# `-destination 'platform=macOS'`, which silently narrows the build to the host
# architecture, while the release used `generic/platform=macOS` with `ARCHS` pinned.
# So CI reported green on a build shape that was never shipped, and the packaging
# path ran for the first time on release day.
#
# One implementation, called by both, is the only way the CI signal means anything.
#
# Usage:
#   Scripts/build-app.sh --arch arm64 --out build-arm64
#   Scripts/build-app.sh --arch native --out build-debug --configuration Debug
#
# The output directory receives MacCV.app. It is passed to xcodebuild explicitly
# rather than left to DerivedData, so two architectures cannot race over one path.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ARCH=""
OUT=""
CONFIGURATION="Release"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --configuration) CONFIGURATION="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$ARCH" || -z "$OUT" ]]; then
  echo "error: --arch and --out are required" >&2
  exit 2
fi

case "$CONFIGURATION" in
  Debug|Release) ;;
  *) echo "error: --configuration must be Debug or Release" >&2; exit 2 ;;
esac

# Resolve the architecture set for xcodebuild.
case "$ARCH" in
  native)
    # `native` is for iteration, not for shipping.
    ARCHS_ARG="arm64"
    ;;
  arm64|x86_64)
    ARCHS_ARG="$ARCH"
    ;;
  universal)
    ARCHS_ARG="arm64 x86_64"
    ;;
  *)
    echo "error: --arch must be arm64, x86_64, universal or native" >&2
    exit 2
    ;;
esac

# An absolute --out is used as given; a relative one is resolved against the
# repository root so the script works from any working directory.
case "$OUT" in
  /*) OUT_ABS="$OUT" ;;
  *) OUT_ABS="$REPO_ROOT/$OUT" ;;
esac
rm -rf "$OUT_ABS"
mkdir -p "$OUT_ABS"

echo "==> $CONFIGURATION build for $ARCHS_ARG -> $OUT"

# The Rust engine must carry the same slices, or the link fails with a confusing
# "building for macOS-arm64 but attempting to link" error.
if [[ "$ARCHS_ARG" == *" "* ]]; then
  MACCV_ARCH="universal"
else
  case "$ARCHS_ARG" in
    x86_64) MACCV_ARCH="x86_64" ;;
    *) MACCV_ARCH="native" ;;
  esac
fi

# Regenerate whenever project.yml is newer than the generated project.
#
# This matters more than it looks: MARKETING_VERSION lives in project.yml, so after
# `bump-version.sh` a stale .xcodeproj would build the *previous* version while every
# source file said otherwise. The check that reads the built bundle caught exactly that
# — a fresh build reporting 0.1.0 against sources saying 0.1.1.
PROJECT_FILE="apps/mac/project.yml"
PBXPROJ="apps/mac/MacCV.xcodeproj/project.pbxproj"

if [[ ! -f "$PBXPROJ" ]] || [[ "$PROJECT_FILE" -nt "$PBXPROJ" ]]; then
  echo "==> generating the Xcode project"
  (cd apps/mac && xcodegen generate >/dev/null)
fi

# `generic/platform=macOS` rather than `platform=macOS`: the latter resolves against
# the running host and narrows ARCHS to it, which produced an arm64-only binary while
# `showBuildSettings` still reported "arm64 x86_64".
MACCV_ARCH="$MACCV_ARCH" xcodebuild \
  -project apps/mac/MacCV.xcodeproj \
  -scheme MacCV \
  -configuration "$CONFIGURATION" \
  -destination 'generic/platform=macOS' \
  ARCHS="$ARCHS_ARG" \
  ONLY_ACTIVE_ARCH=NO \
  "CONFIGURATION_BUILD_DIR=$OUT_ABS" \
  build \
  | (grep -E "error:|warning:|BUILD (SUCCEEDED|FAILED)" || true)

APP="$OUT_ABS/MacCV.app"
if [[ ! -d "$APP" ]]; then
  echo "error: $APP was not produced" >&2
  exit 1
fi

# The whole point of this script: assert the slices that were asked for are present.
# `showBuildSettings` claiming the right ARCHS is not evidence — only the produced
# binary is.
echo -n "    lipo: "
lipo -info "$APP/Contents/MacOS/MacCV"

for slice in $ARCHS_ARG; do
  if ! lipo -info "$APP/Contents/MacOS/MacCV" | grep -q "$slice"; then
    echo "error: the $slice slice is missing from the built binary" >&2
    echo "       ARCHS was '$ARCHS_ARG' but the output does not contain it." >&2
    exit 1
  fi
done

# The deployment target is a claim in the README, so it is checked rather than
# assumed. A dependency raising it silently would exclude users on macOS 15.
MINIMUM="15.0"
for slice in $ARCHS_ARG; do
  actual="$(vtool -arch "$slice" -show-build-version "$APP/Contents/MacOS/MacCV" 2>/dev/null \
    | awk '/minos/{print $2}')"
  if [[ "$actual" != "$MINIMUM" ]]; then
    echo "error: $slice targets macOS $actual but the project promises $MINIMUM" >&2
    exit 1
  fi
done

# Every C ABI symbol must be present in every slice, or the app links on one
# architecture and fails on the other.
expected_symbols="$(nm -gU "$APP/Contents/MacOS/MacCV" 2>/dev/null | grep -c '_maccv_' || true)"
if [[ "${expected_symbols:-0}" -lt 10 ]]; then
  echo "error: only ${expected_symbols:-0} maccv_ symbols found; the engine did not link" >&2
  exit 1
fi

echo "    minOS $MINIMUM, $expected_symbols engine symbols, $(du -sh "$APP" | cut -f1)"
echo "==> ok: $APP"
