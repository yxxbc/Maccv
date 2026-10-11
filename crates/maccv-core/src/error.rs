//! Error and result types.

use std::fmt;

/// Errors produced by pasteboard operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The system denied programmatic access to the general pasteboard.
    ///
    /// macOS 27 introduced a per-app pasteboard access policy. When the user (or
    /// policy) selects "always deny", reads and writes fail. The UI should show a
    /// helpful state instead of retrying in a loop.
    AccessDenied,
    /// No pasteboard with that name exists.
    PasteboardNotFound(String),
    /// The named type is absent from the item.
    TypeNotFound(String),
    /// A payload could not be decoded as the expected representation.
    InvalidPayload(String),
    /// A text payload was not valid UTF-8.
    InvalidUtf8,
    /// Writing to the pasteboard was rejected by AppKit.
    WriteFailed,
    /// An operation was attempted from a context that forbids it.
    InvalidState(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessDenied => write!(
                f,
                "pasteboard access denied by the system (macOS 27 access policy)"
            ),
            Self::PasteboardNotFound(name) => write!(f, "pasteboard not found: {name}"),
            Self::TypeNotFound(ty) => write!(f, "pasteboard type not present: {ty}"),
            Self::InvalidPayload(msg) => write!(f, "invalid payload: {msg}"),
            Self::InvalidUtf8 => write!(f, "text payload is not valid UTF-8"),
            Self::WriteFailed => write!(f, "AppKit rejected the pasteboard write"),
            Self::InvalidState(msg) => write!(f, "invalid state: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
