<div align="center">

<img src="assets/logo.png" alt="MacCV" width="120" height="120">

# MacCV

**The clipboard manager that never puts your secrets on the clipboard.**

A menu-bar clipboard manager for macOS. It keeps every format a copy carried — not just
the text — and can hand content to an app as keystrokes, so nothing else on your Mac can
read what you pasted.

<br>

[![Download](https://img.shields.io/badge/Download-latest%20release-6D4AE8?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/yxxbc/Maccv/releases/latest)
&nbsp;
[![Website](https://img.shields.io/badge/Website-yxxbc.github.io-22D3EE?style=for-the-badge&logo=safari&logoColor=white)](https://yxxbc.github.io/Maccv/)
&nbsp;
[![简体中文](https://img.shields.io/badge/简体中文-Read%20in%20Chinese-4C3DDB?style=for-the-badge)](README.zh-CN.md)

<br>

[![CI](https://github.com/yxxbc/Maccv/actions/workflows/ci.yml/badge.svg)](https://github.com/yxxbc/Maccv/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/yxxbc/Maccv?include_prereleases&sort=semver&color=6D4AE8)](https://github.com/yxxbc/Maccv/releases)
[![Downloads](https://img.shields.io/github/downloads/yxxbc/Maccv/total?color=369eff&labelColor=black&logo=github&logoColor=white&label=Downloads)](https://github.com/yxxbc/Maccv/releases)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![macOS](https://img.shields.io/badge/macOS-15%2B-black.svg)](#requirements)
[![Universal](https://img.shields.io/badge/binary-Apple%20silicon%20%2B%20Intel-black.svg)](#install)
[![Rust](https://img.shields.io/badge/engine-Rust-orange.svg)](#how-it-is-built)
[![Bundle](https://img.shields.io/badge/app-1.9%20MB-brightgreen.svg)](#light)

</div>

<br>

<div align="center">
  <img src="assets/preview-en.png" alt="The MacCV panel: a rich-text entry, an image, a three-file copy, and an API key shown masked" width="720">
</div>

<br>

## The thing no other clipboard manager does

Every clipboard manager writes to the system pasteboard, and **the system pasteboard is
world-readable**. That is not a limitation of those apps — it is how macOS works. There
is no API to write to it privately:

- `NSPasteboardContentsOptionsCurrentHostOnly` stops syncing to your other devices, not
  to other apps.
- AppKit has no `concealed` flag. `org.nspasteboard.ConcealedType` is a convention apps
  follow voluntarily, and nothing enforces it.
- `NSPasteboardAccessBehavior` decides whether *this* app may read. It has no say over
  anyone else.

So MacCV takes the clipboard out of the path. With **Paste Without Clipboard**, the
entry is delivered to the frontmost app as synthetic keystrokes. It never enters a shared
channel, so no other process *can* observe it.

It is off by default, because the costs are real and you should get to weigh them:

| | |
|---|---|
| **Needs Accessibility access** | Injecting input into another app is exactly what that permission gates |
| **Slower** | Delivered at typing pace, not instantly |
| **Text only** | An image has no keystroke form, so images use the clipboard |

This is written down plainly, including what MacCV **cannot** do, because a privacy
claim you cannot verify is worse than no claim at all.
→ [The full security position](docs/SECURITY.md)

What MacCV does guarantee:

- **Nothing leaves your Mac.** No telemetry, no analytics, no crash reporting, no account.
  The only network requests are update checks.
- **Your history is never written to disk**, so no other application can read it.
- **No AI ever reads your data.** There is no model, local or cloud.

And what it does **not** claim: it cannot stop another app reading the clipboard. Nothing
can. That is stated here rather than buried, because a privacy claim you cannot verify is
worse than no claim at all.

## What else it does

**Keeps every format.** Copy a table from Numbers and most managers give you
tab-separated text. MacCV keeps the rich text, the HTML *and* the plain text — so it
pastes correctly into whatever app you are in. Copy three files from Finder and you get
three files back, not three lines of text.

**Hides what it should.** Card numbers, API keys, JWTs, private keys, credential-bearing
URLs, emails and ID numbers are detected and masked **when the entry is stored**, not
just when it is drawn. Entries a password manager marked concealed are never recorded.

**Stays out of the way.** It notices a copy in about half a microsecond, so it does not
have to watch your keystrokes — which means **no Accessibility permission just to run**.
It sits in the menu bar and costs nothing while idle.

**Speaks your language.** English and 简体中文, following the system.

## Install

Download the build for your Mac, open the DMG, drag **MacCV** onto **Applications**.

| Your Mac | Download |
|---|---|
| **Apple silicon** (M1–M4) | [MacCV-…-arm64.dmg](https://github.com/yxxbc/Maccv/releases/latest) |
| **Intel** | [MacCV-…-x86_64.dmg](https://github.com/yxxbc/Maccv/releases/latest) |

→  About This Mac. If it says *Chip: Apple M…* take the Apple silicon build.

**macOS will warn the first time you open it** — *"Apple could not verify MacCV is free
of malware"*. That is Gatekeeper saying the build is not yet notarized, not a report of
anything in it. Clearing the quarantine attribute does **not** fix it; the verdict is
about the signature. Right-click → **Open** once, or `System Settings → Privacy &
Security → Open Anyway`. Notarization is [on the roadmap](#roadmap).

Every release publishes `SHA256SUMS`, and the app verifies the digest and signature
before installing an update of itself.

## Requirements

- **macOS 15 or later** (Sequoia), Apple silicon or Intel
- Nothing else. No helper process, no daemon, no account, no telemetry

Windows and Linux are not planned. This is a native macOS app driving AppKit's
pasteboard directly; porting it would mean writing it again.

## Light

An idle clipboard manager should be invisible.

<div align="center">

| | | | |
|---|---|---|---|
| **0.53 µs** | **9.9 µs** | **1.9 MB** | **0 bytes** |
| to notice a copy | to read text | app bundle | written to disk while idle |

</div>

It checks the clipboard ~2000× a second right after a copy, then backs off to four times
a second when nothing is happening. No timer runs in the UI. Every number here is
asserted by a test, so it cannot quietly stop being true →
[performance standards](docs/PERFORMANCE.md).

## How it is built

A Rust engine linked **as a static library** into a SwiftUI app, over a C ABI. No IPC, no
serialization step, no helper process — that decision is what makes a pasteboard read
microseconds rather than milliseconds, and most other choices follow from it.

```
crates/maccv-core/     the engine: pasteboard, redaction, synthetic input, updates
crates/maccv-ffi/      a thin C ABI; cbindgen generates include/maccv.h
apps/mac/              the SwiftUI app; project.yml is the source of truth
docs/                  the website, plus the standards this project is held to
Scripts/               build, verify, package, release
```

If you are here for the Rust: `maccv-core` is plain Rust with no C concerns and is
testable on its own. `maccv-ffi` only translates. The AppKit behaviour worth knowing is
in `crates/maccv-core/src/pasteboard.rs`, and the two constraints that shaped it are that
`objc2` does not implement `Send` for `NSObject` subclasses (so the watcher reopens its own
handle rather than moving one across threads) and that `writeObjects:` requires
`prepareForNewContents` first.

```bash
git clone https://github.com/yxxbc/Maccv.git && cd Maccv
Scripts/verify.sh                      # 26 stages, ~3 min — the same command CI runs
cargo test -p maccv-core --release     # engine only, with timings
```

`apps/mac/project.yml` is the source of truth for the Xcode project; the `.xcodeproj` is
generated and not committed. A build phase compiles the engine and links it in.

## Documentation

| | |
|---|---|
| [Security](docs/SECURITY.md) | What is guaranteed, what macOS makes impossible, the plugin model |
| [Performance](docs/PERFORMANCE.md) | Budgets, how each number is measured, regression policy |
| [Testing](docs/TESTING.md) | What is tested — and what deliberately is not |
| [Releasing](docs/RELEASING.md) | Channels, the release procedure, emergency revocation |
| [Contributing](CONTRIBUTING.md) | Setup, conventions, the house rules |
| [Changelog](CHANGELOG.md) | What changed, per version |

## Roadmap

- [x] Every pasteboard format, lossless
- [x] Menu-bar panel with search
- [x] Paste without the clipboard
- [x] Sensitive-content masking
- [x] Per-architecture DMGs with checksums and a verified updater
- [x] English and 简体中文, macOS 15, universal binary
- [ ] Global hotkey with a centred panel, like the system clipboard
- [ ] Drag content in and out, with the source app shown
- [ ] History on disk that survives relaunch
- [ ] Notarization, so the first launch is silent

## Contributing

Issues and pull requests are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). Two rules
matter most: tests run against a **process-private pasteboard** so your own clipboard is
never touched, and a privacy claim needs a check behind it, not a sentence in a document.

## License

[Apache-2.0](LICENSE) © 2026 yxxbc

Built on [`objc2`](https://github.com/madsmtm/objc2), which is what makes a Rust AppKit
engine pleasant rather than hazardous.
