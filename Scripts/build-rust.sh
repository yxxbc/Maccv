#!/usr/bin/env bash
#
# Builds the Rust clipboard engine and hands the artifacts to Xcode.
#
# Called both by `project.yml`'s pre-build phase and directly by CI. Keeping the
# logic here (rather than inline in the Xcode project) means the same steps run
# locally and in GitHub Actions, and the project file stays readable.
#
# Usage:
#   Scripts/build-rust.sh [--profile debug|release] [--arch native|universal]
#
# Environment:
#   MACCV_PROFILE   Overrides --profile (set by Xcode configurations)
#   MACCV_ARCH      Overrides --arch
#   CARGO_TARGET_DIR  Respected if already set.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PROFILE="release"
ARCH="native"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile) PROFILE="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

PROFILE="${MACCV_PROFILE:-$PROFILE}"
ARCH="${MACCV_ARCH:-$ARCH}"

case "$PROFILE" in
  debug|release) ;;
  *) echo "invalid profile: $PROFILE (expected debug or release)" >&2; exit 2 ;;
esac

# The Rust target directory is shared with Xcode's derived data via the project's
# build settings, so make sure we build where Xcode will look.
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
export CARGO_TARGET_DIR="$TARGET_DIR"

# Xcode injects its own SDK, deployment target and (for SwiftPM) a toolchain that
# will not have cargo. Strip those so cargo links against the same macOS SDK.
unset SDKROOT MACOSX_DEPLOYMENT_TARGET TOOLCHAINS 2>/dev/null || true

declare -a TARGETS
if [[ "$ARCH" == "universal" ]]; then
  TARGETS=("aarch64-apple-darwin" "x86_64-apple-darwin")
else
  case "$(uname -m)" in
    arm64) TARGETS=("aarch64-apple-darwin") ;;
    x86_64) TARGETS=("x86_64-apple-darwin") ;;
    *) echo "unsupported architecture: $(uname -m)" >&2; exit 2 ;;
  esac
fi

# Ensure the secondary target is installed for universal builds.
if [[ "$ARCH" == "universal" ]]; then
  for target in "${TARGETS[@]}"; do
    if ! rustup target list --installed | grep -qx "$target"; then
      echo "==> installing rust target $target"
      rustup target add "$target"
    fi
  done
fi

echo "==> cargo build (profile=$PROFILE, arch=$ARCH)"
for target in "${TARGETS[@]}"; do
  cargo build -p maccv-ffi --profile "$PROFILE" --target "$target"
done

# Where cargo put things, and where we want the final artifacts.
if [[ "$PROFILE" == "release" ]]; then
  LIB_DIR_NAME="release"
else
  LIB_DIR_NAME="debug"
fi

OUT_DIR="$TARGET_DIR/maccv-apple"
mkdir -p "$OUT_DIR"

if [[ ${#TARGETS[@]} -eq 1 ]]; then
  src="$TARGET_DIR/${TARGETS[0]}/$LIB_DIR_NAME/libmaccv_ffi.a"
  cp "$src" "$OUT_DIR/libmaccv_ffi.a"
  echo "==> $(basename "$src") -> $OUT_DIR/libmaccv_ffi.a"
else
  inputs=()
  for target in "${TARGETS[@]}"; do
    inputs+=("$TARGET_DIR/$target/$LIB_DIR_NAME/libmaccv_ffi.a")
  done
  lipo -create "${inputs[@]}" -output "$OUT_DIR/libmaccv_ffi.a"
  echo "==> lipo'd ${#inputs[@]} slices -> $OUT_DIR/libmaccv_ffi.a"
fi

# The header is checked in, but keep the copy Xcode sees in sync.
mkdir -p "$OUT_DIR/include"
cp "crates/maccv-ffi/include/maccv.h" "$OUT_DIR/include/maccv.h"

# A stable path Xcode can link against without guessing the profile directory.
ln -sfn "$OUT_DIR/libmaccv_ffi.a" "$TARGET_DIR/libmaccv_ffi.a"

echo "==> engine ready: $OUT_DIR"
