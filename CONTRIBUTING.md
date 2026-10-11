# Contributing to MacCV

Thanks for wanting to help. This file covers what you need to get a change merged.

## Getting set up

You will need macOS 27, Xcode 27, Rust 1.85 or newer, and
[XcodeGen](https://github.com/yonaskolb/XcodeGen) (`brew install xcodegen`).
For packaging, `pipx install dmgbuild` and
`python3 -m pip install --break-system-packages Pillow`.

```bash
git clone https://github.com/yxxbc/Maccv.git
cd Maccv

Scripts/verify.sh          # every check: 25 stages, the same command CI runs
./Scripts/verify.sh --quick   # Rust and the source checks only, no app build
```

> **[AGENTS.md](AGENTS.md) is the detailed guide** — in particular its
> "coupled-files map", which lists what else has to change for a given edit. This
> file duplicates only what a human contributor needs first.

`MacCV.xcodeproj` is generated from `apps/mac/project.yml` and is not committed.
Change the YAML, not the project file.

## Before you open a pull request

Everything below is enforced by CI, so running it locally saves a round trip:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/mac && xcodebuild -scheme MacCV -configuration Debug build && cd ../..
```

And to prove the layers actually work together, not merely that they compile:

```bash
cargo build --release -p maccv-ffi
clang -std=c11 -Wall -Wextra -Werror \
  -I crates/maccv-ffi/include crates/maccv-ffi/tests/smoke.c \
  -o /tmp/maccv_smoke -L target/release -lmaccv_ffi \
  -framework AppKit -framework Foundation
/tmp/maccv_smoke
```

## How the project is laid out

| Path | What belongs there |
|---|---|
| `crates/maccv-core/` | The engine. Plain Rust, no C ABI concerns, directly testable. |
| `crates/maccv-ffi/` | The C ABI surface. Thin — it translates, it does not decide. |
| `crates/maccv-ffi/include/maccv.h` | Generated. Never edit by hand. |
| `apps/mac/` | The SwiftUI app. |
| `Scripts/` | Build helpers shared by Xcode and CI. |
| `assets/` | Logo and social preview. SVG sources; the PNGs are rendered from them. |

Put logic in `maccv-core`. If you find yourself writing a decision in `maccv-ffi`,
it almost certainly belongs one layer down, where it can be tested without a C
caller.

## Guidelines

### Tests

New engine behaviour needs a test. The live tests in
`crates/maccv-core/tests/clipboard.rs` run against a real pasteboard — a
process-private one, so the developer's own clipboard is never touched. That is the
pattern to follow for anything touching AppKit: exercise the real framework, then
clean up.

Documented performance claims should be asserted, not measured once by hand. See
`change_count_is_cheap_enough_to_poll` for the shape of that.

### Memory

The C ABI is the risky surface. If you add an entry point:

- Validate every pointer before dereferencing, and report `MaccvError_NullPointer`
  rather than trapping. Null must never crash.
- Document the ownership rule in the function's doc comment, and state how the
  caller releases whatever you return.
- If you hand back pointers into backing storage, make sure that storage owns the
  bytes and outlives the handle. `SnapshotStorage` in `crates/maccv-ffi/src/lib.rs`
  documents why this matters.
- Run the C smoke test. It exists to catch exactly these mistakes.

### Rust style

- `cargo fmt` decides formatting; do not argue with it in review.
- Public items need doc comments; the workspace denies `missing_docs`.
- `unsafe` blocks need a `// SAFETY:` comment explaining why the block is sound.
  The workspace denies `unsafe_op_in_unsafe_fn`, so be explicit.
- Prefer `Result` over panicking. The release profile is `panic = "abort"`, so a
  panic is an immediate crash.

### Swift style

- Strict concurrency is on. Keep the main actor for UI state.
- Objective-C objects must not cross thread boundaries. If you need work off the
  main actor, see how the watcher reopens its own pasteboard handle in
  `crates/maccv-core/src/pasteboard.rs` instead of moving a handle across threads.
- C pointers that the Rust side borrows must outlive the call. Do not point at a
  Swift `Data`'s transient storage — see `CRepresentationStorage`.

### Commits and pull requests

Commits follow [Conventional Commits](https://www.conventionalcommits.org/):
`feat:`, `fix:`, `perf:`, `refactor:`, `docs:`, `test:`, `build:`, `ci:`, `chore:`.

A pull request should explain what changed and why. If it changes behaviour, add a
`CHANGELOG.md` entry under `[Unreleased]` using the existing style — notes are
generated from that file, so it is not optional for user-visible changes.

Keep pull requests focused. One concern per PR is much easier to review than five.

## Reporting bugs

Use the [issue templates](.github/ISSUE_TEMPLATE). For anything to do with
clipboard content, please include:

- What you copied, and which app it came from
- What MacCV showed versus what you expected
- Which macOS version you are on

Bug reports about a specific app are especially useful, because every app puts
slightly different types on the pasteboard.
