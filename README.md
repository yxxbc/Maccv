<div align="center">

<img src="assets/logo.png" alt="MacCV" width="128" height="128">

# MacCV

**A fast, light, private macOS clipboard.**

A menu-bar clipboard manager with a Rust engine and a SwiftUI front end. It reads
and writes the *native* pasteboard — not just text — and it can hand content to an
app without the clipboard seeing it at all.

[![CI](https://github.com/yxxbc/Maccv/actions/workflows/ci.yml/badge.svg)](https://github.com/yxxbc/Maccv/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/yxxbc/Maccv?include_prereleases&sort=semver)](https://yxxbc.github.io/Maccv/changelog/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![macOS](https://img.shields.io/badge/macOS-27%2B-black.svg)](#requirements)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

[English](README.md) · [简体中文](README.zh-CN.md) · [Website](https://yxxbc.github.io/Maccv/)

</div>

---

## The three ideas

MacCV is built around three commitments. They are specific enough to be checked,
and where macOS limits them, this README says so rather than implying otherwise.

### 1. Fast

Speed is not a feature bolted on; it decided the architecture. The engine is Rust
compiled into a **static library linked into the app**, so there is no IPC hop, no
serialization step, and no helper process. A change is detected in **0.53 µs**,
which is what makes polling viable — and polling is what lets MacCV avoid an event
tap, and therefore avoid demanding Accessibility access just to notice a copy.

Breakdown of what "fast" means in practice, asserted by tests rather than measured
once by hand:

| Operation | Cost |
|---|---|
| Detect a change (`changeCount`) | **0.53 µs** |
| Read plain text | **9.9 µs** |
| Notice an external `pbcopy` | **116 µs** |
| Write text | 215 µs — AppKit's own cost; the OS notifies every interested process |

### 2. Light

An idle clipboard manager should be invisible. MacCV's watcher polls at a 400 µs
interval right after a copy — imperceptible — and backs off geometrically to
250 ms while nothing changes. No timer runs in the UI layer. History lives in
memory, and the release app is **1.9 MB**.

### 3. Private

This is where the honest version matters, and where most clipboard managers
overstate what they can do.

**The uncomfortable fact:** once content is on the system pasteboard, *any*
application on the Mac can read it. That is not a bug or a missing feature; it is
how macOS is designed. There is no API to write to the pasteboard privately.
`NSPasteboardContentsOptionsCurrentHostOnly` stops syncing to your other devices,
not to other apps. There is no "concealed" or "transient" flag in AppKit — the
`org.nspasteboard.ConcealedType` convention is honoured only voluntarily, and
`NSPasteboardAccessBehavior` governs whether *this* process may read, never whether
anyone else may.

So MacCV does the one thing that actually works: **it can paste without using the
clipboard at all.**

With **Paste Without Clipboard** enabled, choosing it from an entry's context menu
sends the text to the frontmost app as synthetic keystrokes via
`CGEventKeyboardSetUnicodeString`. The content never enters a shared channel, so no
other process *can* observe it. That is a real guarantee, not a policy.

Its costs are real too, so it is off by default and offered as a choice:

- It needs the **Accessibility permission**, because injecting events into another
  app is precisely what that permission gates.
- It is **slower** — events are delivered at typing pace.
- It is **text only**. An image has no keystroke representation, so images still
  take the clipboard path.

Alongside that, the things that *are* fully within the app's control:

| Control | What it actually guarantees |
|---|---|
| History stays in the app container | Other applications cannot read your history. Only the pasteboard write is shared, never the log. |
| Skip concealed entries | Entries a password manager marked `ConcealedType` are **never recorded** — honouring a stated intent, not guessing. |
| Mask sensitive content | Detected card numbers, API keys, JWTs, private keys, credentialed URLs, emails and ID numbers are redacted **when recorded**, not just when drawn. |
| Confirmation before replacing itself | A downloaded update is verified, then presented. MacCV never silently rewrites its own bundle. |

**What MacCV does not claim:** it cannot stop another app from reading the
clipboard. Nothing can. If you need that, use Paste Without Clipboard, or don't put
the secret on the clipboard. The README says this plainly because a privacy claim
you cannot verify is worse than no claim.

## What it handles

Every pasteboard type macOS defines, not a text-only subset:

| Category | Types |
|---|---|
| Text | `public.utf8-plain-text`, `public.utf8-tab-separated-values-text` |
| Rich text | `public.rtf`, `com.apple.flat-rtfd`, `public.html` |
| Images | `public.png`, `public.tiff`, `com.adobe.pdf` |
| Files | `public.file-url` — multi-file copies stay multi-file |
| Links | `public.url` |
| Colour | `com.apple.cocoa.pasteboard.color`, resolved to sRGB |
| Sound | `com.apple.cocoa.pasteboard.sound` |
| Fonts & rulers | `com.apple.cocoa.pasteboard.font`, `…ruler` |
| Anything else | Custom UTIs, preserved and replayed verbatim |

A single copy can carry several of these at once — a paragraph from Safari arrives
as plain text, RTF *and* HTML — and MacCV keeps all of them, so it can paste into
whatever the target app asks for.

## Settings

- **General** — history size, and interface language (follows the system, or pin
  English / 简体中文).
- **Privacy** — mask sensitive content, keep a recognisable hint, skip concealed
  entries, paste without the clipboard, and the Accessibility permission state.
- **Updates** — automatic / notify-only / manual, an optional pre-release channel,
  and `Check for Updates`.
- **About** — version, build, engine version, and links.

## Updates

`Check for Updates` reads a manifest published on the [website](https://yxxbc.github.io/Maccv/latest.json).
Installing runs a fixed sequence, and the order is the point:

1. download to a staging directory
2. verify the **SHA-256** — a mismatch deletes the file and stops
3. verify the **code signature** with `spctl`
4. **only then** clear `com.apple.quarantine`
5. report which of those actually succeeded

Step 4 is what prevents the notorious *"MacCV is damaged and can't be opened"*
dialog. An app that downloads a new version of itself inherits the quarantine
attribute from its parent, and macOS refuses to launch a quarantined,
non-notarized bundle. Clearing the flag *before* verification would defeat the flag
entirely — which is why an update whose signature does not validate is reported as
such and left quarantined rather than silently installed.

MacCV does **not** replace its own running bundle. A half-completed swap of a live
app is worse than asking you to drag one file, so the verified, de-quarantined
bundle is revealed in Finder and you finish the job.

## Requirements

- macOS 27 or later
- To build: Xcode 27, Rust 1.85+, [XcodeGen](https://github.com/yonaskolb/XcodeGen)

## Build and run

```bash
git clone https://github.com/yxxbc/Maccv.git
cd Maccv

# Rust engine: build and test it on its own
cargo test
cargo build --release -p maccv-ffi

# The app (generates MacCV.xcodeproj, then builds it)
cd apps/mac
xcodegen generate
xcodebuild -scheme MacCV -configuration Release build
```

`apps/mac/project.yml` is the source of truth for the Xcode project; the
`.xcodeproj` is generated and not committed. A pre-build phase compiles the Rust
engine and links it in, so an Xcode build is the only step you need.

### Verifying a build

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test                                  # 109 tests
xcodebuild -scheme MacCV build

# The compiled app exercises the whole stack, end to end:
"$BUILD_DIR/MacCV.app/Contents/MacOS/MacCV" --self-test
```

The self-test runs against a **process-private** pasteboard, so it never reads or
clobbers your real clipboard and CI does not depend on what happens to be copied.

`Scripts/publish.sh` creates the GitHub repository, sets its description and
topics, and pushes. It is a script you run deliberately, because publishing is an
outward-facing action rather than a build step.

## Project layout

```
crates/maccv-core/       Rust engine — the interesting part
  pasteboard.rs            NSPasteboard read/write, snapshots, watching
  content.rs               typed views and classification
  secure_input.rs          synthetic typing (paste without a clipboard)
  redaction.rs             sensitive-content detection and masking
  updater.rs               update check, verification, de-quarantining
crates/maccv-ffi/        C ABI bridge; generates include/maccv.h
apps/mac/                SwiftUI app; project.yml is the project source of truth
  Resources/Localizable.xcstrings   English and Simplified Chinese
docs/                    The website (GitHub Pages)
Scripts/                 build-rust.sh, changelog-notes.sh, publish.sh
assets/                  Logo and social preview (SVG + rendered PNG)
```

### How the two halves connect

`maccv-core` is plain Rust with no C concerns, so it is used and tested directly.
`maccv-ffi` is a thin `staticlib` exposing it over a C ABI; `cbindgen` generates
`maccv.h` during the build. Swift imports that header through a bridging header and
calls straight into Rust — same address space, no marshalling layer.

Three decisions worth knowing about:

**The watcher opens its own handle.** `objc2` deliberately does not mark `NSObject`
subclasses `Send`. Rather than force it with an `unsafe impl`, the watcher thread
reopens the pasteboard by name and only the change counter crosses the thread
boundary.

**A snapshot is one allocation, and owns its bytes.** The C ABI returns a single
block containing the item array, representation arrays, payload bytes and strings.
Freeing the snapshot frees all of it, and nothing points into Rust memory that has
already been dropped.

**Redaction happens on the way in.** Masking an entry when it is recorded, rather
than when it is drawn, means the stored preview and payloads are redacted too — so
a screenshot of the panel or a look at process memory does not expose the value.

## macOS 27 and pasteboard privacy

macOS 27 polices programmatic access to the general pasteboard. The first time an
app reads it the system may ask, and the answer can be remembered as *never allow*,
permanently.

MacCV treats that as a first-class state rather than an error to retry: the engine
reports `AccessDenied`, the panel explains what happened and links to the right
System Settings pane, and nothing polls in a loop trying to get around a refusal.

## Roadmap

- [x] Full-type pasteboard read/write, lossless round trips
- [x] Change watching with adaptive polling
- [x] Multi-item (*n* files, *n* items) fidelity
- [x] SwiftUI menu-bar panel
- [x] Paste without the clipboard
- [x] Sensitive-content masking
- [x] In-app updates with signature verification and quarantine removal
- [x] English and Simplified Chinese interface
- [ ] On-disk history (SQLite) surviving relaunch
- [ ] Global hotkey
- [ ] Drag and drop into and out of the panel
- [ ] Pinned entries, per-app rules
- [ ] Notarization so the app opens without a Gatekeeper prompt

## Contributing

Issues and pull requests are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) and
the [Code of Conduct](CODE_OF_CONDUCT.md).

The tests in `crates/maccv-core/tests/clipboard.rs` run against a real
`NSPasteboard` (a private one, so your clipboard is untouched), and they are the
best description of intended behaviour.

## License

[Apache-2.0](LICENSE) © yxxbc

## Acknowledgements

Built on [`objc2`](https://github.com/madsmtm/objc2) for safe Objective-C interop,
which is what makes a Rust AppKit engine pleasant rather than hazardous.
