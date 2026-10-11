//! # maccv-core
//!
//! Fast, faithful access to the macOS pasteboard (`NSPasteboard`).
//!
//! `maccv-core` is the engine behind [MacCV](https://github.com/yxxbc/Maccv). It
//! aims to be a *complete* and *lossless* wrapper around the native pasteboard:
//! whatever a native app can put on the pasteboard, this crate can read and write
//! back byte-for-byte — plain text, rich text (RTF/RTFD/HTML), images
//! (PNG/TIFF/PDF), file URLs, colors, sounds, and arbitrary custom UTIs.
//!
//! ## Design rules
//!
//! 1. **Fidelity first.** Reading a type gives you the exact bytes the OS holds.
//!    Writing a snapshot replays every type that was captured.
//! 2. **Bytes, not strings.** Rich types are surfaced as `&[u8]` so callers never
//!    pay for an unwanted UTF-8 decode or image re-encode.
//! 3. **Zero-copy where the OS allows it.** Large payloads can be borrowed
//!    straight from the `NSData` backing store.
//! 4. **Cheap change detection.** [`Pasteboard::change_count`] costs well under a
//!    microsecond, so polling is a first-class strategy.
//!
//! ## macOS 27 pasteboard authorization
//!
//! macOS 27 added [`Pasteboard::access_behavior`]. Programmatic reads of the
//! *general* pasteboard may trigger a system authorization prompt the first time
//! they happen. A clipboard manager must therefore expect
//! [`Error::AccessDenied`] and degrade gracefully rather than assume reads
//! always succeed.
//!
//! ## Example
//!
//! ```no_run
//! use maccv_core::{Content, Pasteboard};
//!
//! let pb = Pasteboard::general();
//!
//! // Executing this outside a test will trigger macOS 27's pasteboard access
//! // prompt the first time it runs, and `snapshot()` will then return
//! // `Err(Error::AccessDenied)` if the user declines.
//! let snapshot = pb.snapshot()?;
//! for item in &snapshot.items {
//!     if let Content::Text(text) = &item.content {
//!         println!("{}", text.text);
//!     }
//! }
//! # Ok::<(), maccv_core::Error>(())
//! ```

#![cfg_attr(not(target_os = "macos"), allow(unused))]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(target_os = "macos"))]
compile_error!("maccv-core only supports macOS: it wraps AppKit's NSPasteboard.");

mod content;
mod error;
mod pasteboard;
mod redaction;
mod secure_input;
mod types;
mod updater;

pub use content::{
    ColorValue, Content, ContentKind, FileList, ImageData, RichText, SoundData, Text, WebLoc,
};
pub use error::{Error, Result};
pub use pasteboard::{
    AccessBehavior, ChangeCount, Pasteboard, PasteboardName, Representation, Snapshot,
    SnapshotItem, WatchConfig, WatchHandle,
};
pub use redaction::{MASK, Redacted, RedactionMode, SensitiveKind, SensitiveMatch, detect, redact};
pub use secure_input::{AccessibilityPermission, SecureInput, TypeReport, TypingCadence};
pub use types::{PasteboardType, WellKnownType};
pub use updater::{
    DEFAULT_MANIFEST_URL, QUARANTINE_ATTR, ReleaseManifest, StagedUpdate, UpdateChannel,
    UpdatePolicy, UpdateStatus, check_for_update, compare_versions, download_and_verify,
    has_quarantine, quarantine_paths, read_quarantine, remove_quarantine, sha256_file,
};

/// Crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
