//! Pasteboard type identifiers (UTIs).
//!
//! On macOS a pasteboard "type" is a Uniform Type Identifier string such as
//! `public.utf8-plain-text`. [`PasteboardType`] is a thin, comparable newtype
//! around that string; [`WellKnownType`] lists the identifiers every Mac app
//! agrees on.

use std::borrow::Borrow;
use std::fmt;

use objc2_foundation::NSString;

/// A pasteboard type identifier (a UTI, e.g. `public.png`).
///
/// Comparison is byte-wise on the underlying identifier, so
/// `PasteboardType::new("public.png") == WellKnownType::Png.into()`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PasteboardType(String);

impl PasteboardType {
    /// Creates a pasteboard type from any string-like identifier.
    pub fn new(identifier: impl Into<String>) -> Self {
        Self(identifier.into())
    }

    /// The identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the type, returning the owned identifier.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Whether this identifier denotes a UTI in the given family prefix,
    /// e.g. `conforms_to_family("public.")`.
    pub fn has_prefix(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix)
    }
}

impl fmt::Display for PasteboardType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for PasteboardType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PasteboardType({:?})", self.0)
    }
}

impl From<&str> for PasteboardType {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for PasteboardType {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<WellKnownType> for PasteboardType {
    fn from(value: WellKnownType) -> Self {
        Self(value.as_str().to_owned())
    }
}

impl Borrow<str> for PasteboardType {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PasteboardType {
    /// Wraps an `NSString` returned by AppKit.
    ///
    /// # Safety
    ///
    /// The caller must ensure `value` was produced by AppKit and is still alive.
    pub(crate) unsafe fn from_ns(value: &NSString) -> Self {
        Self(value.to_string())
    }
}

/// The pasteboard types every Mac application understands.
///
/// These are the modern `NSPasteboardType*` constants. Deprecated aliases such
/// as `NSFilenamesPboardType` are intentionally omitted: they are never written
/// by current macOS, and reading them requires property-list decoding that the
/// modern types supersede.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WellKnownType {
    /// UTF-8 plain text (`public.utf8-plain-text`).
    String,
    /// HTML fragment (`public.html`).
    Html,
    /// Rich Text Format (`public.rtf`).
    Rtf,
    /// Rich Text Format with attachments (`public.rtfd`).
    Rtfd,
    /// Portable Network Graphics (`public.png`).
    Png,
    /// Tagged Image File Format (`public.tiff`).
    Tiff,
    /// Portable Document Format (`com.adobe.pdf`).
    Pdf,
    /// A URL (`public.url`).
    Url,
    /// A file URL (`public.file-url`).
    FileUrl,
    /// A tabular (tab/newline separated) text block (`public.utf8-tab-separated-values-text`).
    TabularText,
    /// A font (`com.apple.cocoa.pasteboard.font`).
    Font,
    /// A ruler / paragraph formatting (`com.apple.cocoa.pasteboard.ruler`).
    Ruler,
    /// A color (`com.apple.cocoa.pasteboard.color`).
    Color,
    /// A sound (`com.apple.cocoa.pasteboard.sound`).
    Sound,
    /// Multiple text selections (`com.apple.cocoa.pasteboard.multiple-text-selection`).
    MultipleTextSelection,
    /// Finder's promise to provide files later (`com.apple.NSFilesPromisePboardType`).
    FilesPromise,
    /// PostScript (`com.adobe.postscript`).
    PostScript,
    /// vCard contact data (`public.vcard`).
    VCard,
}

impl WellKnownType {
    /// The canonical UTI string for this type.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "public.utf8-plain-text",
            Self::Html => "public.html",
            Self::Rtf => "public.rtf",
            Self::Rtfd => "com.apple.flat-rtfd",
            Self::Png => "public.png",
            Self::Tiff => "public.tiff",
            Self::Pdf => "com.adobe.pdf",
            Self::Url => "public.url",
            Self::FileUrl => "public.file-url",
            Self::TabularText => "public.utf8-tab-separated-values-text",
            Self::Font => "com.apple.cocoa.pasteboard.font",
            Self::Ruler => "com.apple.cocoa.pasteboard.ruler",
            Self::Color => "com.apple.cocoa.pasteboard.color",
            Self::Sound => "com.apple.cocoa.pasteboard.sound",
            Self::MultipleTextSelection => "com.apple.cocoa.pasteboard.multiple-text-selection",
            Self::FilesPromise => "com.apple.NSFilesPromisePboardType",
            Self::PostScript => "com.adobe.postscript",
            Self::VCard => "public.vcard",
        }
    }

    /// Every well-known type, in a stable order.
    pub const ALL: &'static [WellKnownType] = &[
        Self::String,
        Self::Html,
        Self::Rtf,
        Self::Rtfd,
        Self::Png,
        Self::Tiff,
        Self::Pdf,
        Self::Url,
        Self::FileUrl,
        Self::TabularText,
        Self::Font,
        Self::Ruler,
        Self::Color,
        Self::Sound,
        Self::MultipleTextSelection,
        Self::FilesPromise,
        Self::PostScript,
        Self::VCard,
    ];

    /// Maps a UTI string back to a [`WellKnownType`], if it is one.
    pub fn from_identifier(identifier: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.as_str() == identifier)
    }

    /// Whether this type carries binary rather than textual content.
    pub fn is_binary(self) -> bool {
        !matches!(
            self,
            Self::String | Self::Html | Self::Rtf | Self::Url | Self::FileUrl | Self::TabularText
        )
    }
}

impl fmt::Display for WellKnownType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<WellKnownType> for &'static str {
    fn from(value: WellKnownType) -> Self {
        value.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for ty in WellKnownType::ALL {
            assert!(seen.insert(ty.as_str()), "duplicate UTI: {}", ty.as_str());
        }
        assert_eq!(seen.len(), WellKnownType::ALL.len());
    }

    #[test]
    fn round_trips_through_identifier() {
        for ty in WellKnownType::ALL {
            assert_eq!(WellKnownType::from_identifier(ty.as_str()), Some(*ty));
        }
        assert_eq!(WellKnownType::from_identifier("com.example.custom"), None);
    }

    #[test]
    fn converts_to_pasteboard_type() {
        let ty: PasteboardType = WellKnownType::Png.into();
        assert_eq!(ty.as_str(), "public.png");
        assert_eq!(ty, PasteboardType::new("public.png"));
    }

    #[test]
    fn binary_classification() {
        assert!(!WellKnownType::String.is_binary());
        assert!(!WellKnownType::Rtf.is_binary());
        assert!(WellKnownType::Png.is_binary());
        assert!(WellKnownType::Color.is_binary());
    }
}
