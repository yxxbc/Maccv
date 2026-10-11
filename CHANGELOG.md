# Changelog

All notable changes to MacCV are documented here.

This file is the source of truth for release notes: the release workflow reads the
section matching the tag it is publishing. Follow the conventions below and the
notes write themselves.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Conventions

- One section per released version, newest first.
- A version heading is `## [X.Y.Z] - YYYY-MM-DD`.
- Group changes under `### Added`, `### Changed`, `### Fixed`, `### Removed`,
  `### Security`, or `### Deprecated`. Omit groups that do not apply.
- Write entries for users, not for the diff: describe the behaviour that changed,
  not the file that changed.
- Add a link definition at the bottom so the version heading links to its diff.

<!--
  The [Unreleased] section is for changes that have landed but are not yet
  released. On release, move its entries into a new version heading and leave the
  section empty again.
-->

## [Unreleased]

## [0.1.0] - 2026-10-11

First release. MacCV is a menu-bar clipboard manager that treats the macOS
pasteboard as it actually is: multi-item, multi-format, and worth preserving
exactly.

### Added

- **Full-type pasteboard support.** Reads and writes every type macOS defines,
  including RTF, RTFD, HTML, PNG, TIFF, PDF, file URLs, colors, sounds, fonts and
  rulers, plus arbitrary custom UTIs. A single copy carrying several
  representations keeps all of them, so it can paste into whichever target app
  asks for something specific.
- **Lossless replay.** A captured snapshot is rewritten byte-for-byte, so pasting
  into the originating app reproduces the original formatting rather than an
  approximation of it.
- **Multi-item fidelity.** A three-file Finder copy stays three items instead of
  collapsing into one entry.
- **Change watching with adaptive polling.** Detection begins at a 400 µs interval
  so a copy is noticed almost immediately, then backs off to 250 ms while the
  pasteboard is idle. An idle MacCV costs no measurable CPU.
- **Menu-bar panel** with live previews, search, restore, paste-as-plain-text and
  per-entry removal, built with SwiftUI for macOS 27.
- **Rust engine linked as a static library.** No IPC, no serialization step, no
  helper process.
- **macOS 27 pasteboard authorization handled explicitly.** When the system denies
  access, the engine reports it and the panel explains it, instead of retrying.
- **C ABI with a generated header.** `maccv.h` is produced by cbindgen at build
  time and drives the Swift bridging header.
- **`--self-test` mode** that exercises the Swift → C ABI → Rust path end to end
  and exits non-zero on failure, so CI can verify the integration and not just the
  compile.

### Notes

- Requires macOS 27 or later.
- Clipboard history is in-memory only; it does not survive relaunch yet.
- The app is not sandboxed. Nothing leaves the machine.

[Unreleased]: https://github.com/yxxbc/Maccv/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/yxxbc/Maccv/releases/tag/v0.1.0
