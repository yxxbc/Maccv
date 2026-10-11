#!/usr/bin/env bash
#
# Runs every check the project has, in the order that fails fastest.
#
# This is the single command CI runs and the single command a contributor runs
# before opening a pull request, so the two cannot drift. Each stage prints PASS or
# FAIL and the script exits non-zero on the first failure, so a broken build is
# obvious rather than buried in output.
#
# Usage:
#   Scripts/verify.sh                 # everything
#   Scripts/verify.sh --quick         # skip the app build (Rust only)
#   Scripts/verify.sh --no-network    # skip checks that reach the network

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

QUICK=0
ALLOW_NETWORK=1
for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=1 ;;
    --no-network) ALLOW_NETWORK=0 ;;
    -h|--help) sed -n '2,18p' "${BASH_SOURCE[0]}"; exit 0 ;;
  esac
done

PASS=0
FAIL=0
FAILED_NAMES=()

# Runs a stage, capturing output so a passing stage stays quiet but a failing one
# shows everything it printed.
stage() {
  local name="$1"; shift
  printf '%-42s ' "$name"
  local log
  log="$(mktemp)"
  if "$@" >"$log" 2>&1; then
    printf 'PASS\n'
    PASS=$((PASS + 1))
  else
    printf 'FAIL\n'
    FAIL=$((FAIL + 1))
    FAILED_NAMES+=("$name")
    sed 's/^/    /' "$log" | tail -30
  fi
  rm -f "$log"
}

# ---------------------------------------------------------------------------
# Rust
# ---------------------------------------------------------------------------

stage "cargo fmt --check" \
  cargo fmt --all --check

stage "clippy (warnings are errors)" \
  cargo clippy --workspace --all-targets -- -D warnings

stage "cargo test" \
  cargo test --workspace --all-targets

stage "cargo doc (warnings are errors)" \
  env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# ---------------------------------------------------------------------------
# C ABI
#
# The header is generated but committed, so a stale copy silently disagrees with
# the library and only fails in a C caller. This is the check that catches it.
# ---------------------------------------------------------------------------

stage "build the static library" \
  cargo build --release -p maccv-ffi

stage "generated header is current" \
  bash -c '
    cd "'"$REPO_ROOT"'"
    git diff --exit-code -- crates/maccv-ffi/include/maccv.h
  '

stage "C smoke test compiles and runs" \
  bash -c '
    set -e
    out="$(mktemp -d)"
    # The static archive is named explicitly rather than via -lmaccv_ffi: with both
    # a .a and a .dylib present the linker prefers the dylib, and the resulting
    # binary then needs a library search path at runtime. The app links the static
    # archive, so the test must too.
    clang -std=c11 -Wall -Wextra -Werror -O1 \
      -I crates/maccv-ffi/include \
      crates/maccv-ffi/tests/smoke.c \
      -o "$out/smoke" \
      target/release/libmaccv_ffi.a \
      -framework AppKit -framework Foundation -framework CoreGraphics
    "$out/smoke"
    rm -rf "$out"
  '

# ---------------------------------------------------------------------------
# Workflows and scripts
# ---------------------------------------------------------------------------

if command -v actionlint >/dev/null 2>&1; then
  stage "actionlint (workflows)" actionlint
else
  printf '%-42s %s\n' "actionlint (workflows)" "SKIP (not installed)"
fi

stage "shell script syntax" \
  bash -c 'for f in Scripts/*.sh; do bash -n "$f" || exit 1; done'

# ---------------------------------------------------------------------------
# Version consistency
#
# A version that disagrees with itself only shows up after publishing, when
# auto-update stops working for everyone who installed.
# ---------------------------------------------------------------------------

stage "version recorded consistently" \
  python3 Scripts/checks/version_consistency.py

# ---------------------------------------------------------------------------
# Website
# ---------------------------------------------------------------------------

stage "website links and i18n parity" \
  python3 Scripts/checks/website.py

stage "update feeds are valid" \
  python3 Scripts/checks/feeds.py

# ---------------------------------------------------------------------------
# Localisation
#
# A key used in code but missing from the catalog renders as the key itself, which
# is invisible in English and obvious in Chinese. Only a check catches it.
# ---------------------------------------------------------------------------

stage "localisation keys all exist" \
  python3 Scripts/checks/i18n.py

# ---------------------------------------------------------------------------
# Security standards (docs/SECURITY.md)
#
# These are the invariants the project's privacy claims rest on, so they are
# checked rather than documented and hoped for.
# ---------------------------------------------------------------------------

stage "no undeclared network access" \
  python3 Scripts/checks/no_undeclared_network.py

stage "clipboard content is never persisted" \
  python3 Scripts/checks/no_content_persistence.py

stage "documented standards exist" \
  bash -c '
    set -e
    for doc in SECURITY PERFORMANCE TESTING RELEASING; do
      [ -f "docs/$doc.md" ] || { echo "docs/$doc.md is missing"; exit 1; }
      # A standards document with no content is worse than none.
      [ "$(wc -l < "docs/$doc.md")" -gt 40 ] || { echo "docs/$doc.md is too thin"; exit 1; }
    done
  '

# ---------------------------------------------------------------------------
# App
# ---------------------------------------------------------------------------

if [[ "$QUICK" == "0" ]]; then
  if command -v xcodegen >/dev/null 2>&1; then
    stage "xcodegen + Release build (universal)" \
      bash -c '
        set -e
        cd apps/mac
        xcodegen generate >/dev/null
        # generic/platform, not platform=macOS: the latter narrows ARCHS to the
        # host and silently produces a single-slice binary.
        xcodebuild -project MacCV.xcodeproj -scheme MacCV \
          -configuration Release -destination "generic/platform=macOS" build
      '

    stage "app is universal at macOS 15" \
      bash -c '
        set -e
        app=$(find ~/Library/Developer/Xcode/DerivedData/MacCV-*/Build/Products/Release \
              -maxdepth 1 -name MacCV.app 2>/dev/null | head -1)
        [ -n "$app" ] || { echo "no Release build found"; exit 1; }
        info=$(lipo -info "$app/Contents/MacOS/MacCV")
        echo "$info"
        for arch in arm64 x86_64; do
          echo "$info" | grep -q "$arch" || { echo "missing $arch"; exit 1; }
          vtool -arch "$arch" -show-build-version "$app/Contents/MacOS/MacCV" \
            | grep -q "minos 15.0" || { echo "$arch is not targeting macOS 15"; exit 1; }
        done
      '

    stage "app self-test (Swift → C ABI → Rust)" \
      bash -c '
        set -e
        app=$(find ~/Library/Developer/Xcode/DerivedData/MacCV-*/Build/Products/Release \
              -maxdepth 1 -name MacCV.app 2>/dev/null | head -1)
        "$app/Contents/MacOS/MacCV" --self-test
      '
  else
    printf '%-42s %s\n' "app build" "SKIP (xcodegen not installed)"
  fi

  if [[ "$ALLOW_NETWORK" == "1" ]]; then
    stage "update feed is reachable" \
      bash -c '
        set -e
        url="https://yxxbc.github.io/Maccv/latest.json"
        code=$(curl -sS -m 20 -o /dev/null -w "%{http_code}" "$url" || echo 000)
        # 404 before the first release is expected, not a failure.
        if [ "$code" = "200" ] || [ "$code" = "404" ]; then
          echo "$url -> $code"
        else
          echo "$url -> $code"
          exit 1
        fi
      '
  fi
else
  printf '%-42s %s\n' "app build" "SKIP (--quick)"
fi

# ---------------------------------------------------------------------------

echo
if [[ "$FAIL" == "0" ]]; then
  echo "$PASS stage(s) passed"
  exit 0
fi

echo "$FAIL stage(s) failed: ${FAILED_NAMES[*]}"
echo "(and $PASS passed)"
exit 1
