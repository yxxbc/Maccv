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

stage "READMEs agree on the facts" \
  python3 Scripts/checks/readme_parity.py

stage "README and site images exist" \
  bash -c '
    set -e
    images="assets/preview-en.png assets/preview-zh.png assets/preview-hero-en.png assets/preview-hero-zh.png assets/logo.png assets/social-preview.png docs/preview-hero-en.png docs/preview-hero-zh.png"
    count=0
    for image in $images; do
      [ -s "$image" ] || { echo "missing or empty: $image"; exit 1; }
      # These load on every page view of the repository or the site, so size is a real
      # cost to a reader rather than a detail of the build.
      kb=$(( $(stat -f%z "$image") / 1024 ))
      [ "$kb" -lt 800 ] || { echo "$image is ${kb} KB, over the 800 KB budget"; exit 1; }
      count=$((count + 1))
    done
    echo "$count images present and within budget"
  '

stage "AGENTS.md matches the repository" \
  python3 Scripts/checks/agents_doc.py

# A file the build needs but which .gitignore excludes works locally and fails in a
# fresh clone. That has already happened once, with the Info.plist.
stage "build inputs are committed" \
  python3 Scripts/checks/tracked_files.py

stage "release signing is either configured or documented" \
  bash -c '
    set -e
    # An ad-hoc signed release is acceptable, but it must be a stated choice rather
    # than an accident: the docs have to say what users will see.
    if grep -q "could not verify" docs/RELEASING.md && grep -q "adhoc" docs/RELEASING.md; then
      echo "unsigned path documented"
    else
      echo "docs/RELEASING.md does not explain the ad-hoc signing outcome"
      exit 1
    fi
    if grep -q "MACCV_DEVELOPER_ID" .github/workflows/release.yml; then
      echo "signing secrets wired"
    else
      echo "release.yml has no signing path"
      exit 1
    fi
  '

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
    stage "app builds for arm64 and x86_64" \
      bash -c '
        set -e
        # The same script CI and the release workflow call, so all three exercise
        # one implementation. Previously this stage used a different destination
        # from the release and silently validated the wrong build shape.
        for arch in arm64 x86_64; do
          Scripts/build-app.sh --arch "$arch" --out "dist/verify-$arch" >/dev/null
        done
        echo "both slices built"
      '

    # Running a slice of the other architecture needs Rosetta on Apple silicon (or
    # an Intel host). Where it is unavailable the stage reports SKIP with the reason
    # rather than either failing falsely or passing without testing anything —
    # a check that silently does nothing is worse than no check.
    stage "app self-test per architecture" \
      bash -c '
        set -e
        host="$(uname -m)"
        ran=0

        for arch in arm64 x86_64; do
          binary="dist/verify-$arch/MacCV.app/Contents/MacOS/MacCV"

          if [ "$arch" = "$host" ]; then
            "$binary" --self-test >/dev/null
            echo "  $arch (native) verified"
            ran=$((ran + 1))
            continue
          fi

          # Non-native: needs Rosetta. Probe rather than assume.
          if arch -"$arch" /usr/bin/true 2>/dev/null; then
            arch -"$arch" "$binary" --self-test >/dev/null
            echo "  $arch (Rosetta) verified"
            ran=$((ran + 1))
          else
            echo "  $arch SKIPPED: cannot execute a $arch binary on this $host host"
            echo "    (install Rosetta: softwareupdate --install-rosetta --agree-to-license)"
          fi
        done

        [ "$ran" -ge 1 ] || { echo "no architecture could be tested"; exit 1; }
      '

    # After the build, so it inspects a bundle from *this* run. The source-only version
    # check runs early instead; it cannot see the resolved plist.
    stage "built bundle: version, icon, target" \
      python3 Scripts/checks/built_bundle.py

    # A DMG is a zip of nothing: it only counts as working if it mounts and the app
    # inside runs. This is the check that would have caught the empty first image.
    if command -v dmgbuild >/dev/null 2>&1 || python3 -c "import dmgbuild" 2>/dev/null; then
      stage "DMG builds, mounts, and its app runs" \
        bash -c '
          set -e
          PATH="$PATH:$HOME/.local/bin" \
            Scripts/make-dmg.sh --app dist/verify-arm64/MacCV.app \
              --out dist/verify.dmg >/dev/null
          mount="$(mktemp -d)"
          hdiutil attach -nobrowse -readonly -mountpoint "$mount" dist/verify.dmg >/dev/null
          app="$(find "$mount" -maxdepth 1 -name "*.app" -print -quit)"
          [ -n "$app" ] || { echo "the image contains no app"; hdiutil detach "$mount" >/dev/null; exit 1; }
          "$app/Contents/MacOS/MacCV" --self-test >/dev/null
          hdiutil detach "$mount" >/dev/null
          echo "$(du -h dist/verify.dmg | cut -f1) image verified"
        '
    else
      printf '%-42s %s\n' "DMG builds, mounts, and its app runs" "SKIP (dmgbuild not installed)"
    fi
  else
    printf '%-42s %s\n' "app build" "SKIP (xcodegen not installed)"
  fi

  if [[ "$ALLOW_NETWORK" == "1" ]]; then
    # Reachable is not the same as usable: the engine refuses a manifest it cannot
    # verify, so a feed with a bad digest would break every install while the URL
    # returned 200. Also tolerates absence and offline, since it is the only check
    # that reaches the network.
    stage "published update feed is usable" \
      python3 Scripts/checks/live_feed.py
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
