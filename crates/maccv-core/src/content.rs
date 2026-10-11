//! Typed views over pasteboard payloads.
//!
//! AppKit hands back raw `NSData` per type. These types group the raw bytes into
//! the shapes an application actually wants to reason about, while never
//! discarding the original bytes — so a snapshot can always be replayed exactly.
//!
//! Every variant keeps its binary payloads as `Vec<u8>`. Nothing is decoded
//! unless the caller asks for it, which keeps reads at the speed of a `memcpy`.

use std::path::PathBuf;

/// The dominant nature of a pasteboard item, used for indexing and UI.
///
/// This is a lossy, cheap classification: it picks the "best" representation for
/// display purposes. The full raw payload set stays available on the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContentKind {
    /// Plain text.
    Text,
    /// Rich text (RTF, RTFD or HTML, usually with a plain-text fallback).
    RichText,
    /// A raster or vector image.
    Image,
    /// One or more file URLs.
    Files,
    /// A web or general URL.
    Url,
    /// A color.
    Color,
    /// A sound.
    Sound,
    /// Something with no well-known interpretation.
    Other,
}

impl ContentKind {
    /// A short, stable, machine-friendly name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::RichText => "rich-text",
            Self::Image => "image",
            Self::Files => "files",
            Self::Url => "url",
            Self::Color => "color",
            Self::Sound => "sound",
            Self::Other => "other",
        }
    }

    /// Whether this kind typically has a heavy binary payload worth caching.
    pub const fn is_binary(self) -> bool {
        matches!(self, Self::Image | Self::Sound | Self::Files)
    }
}

/// Plain or lightly-formatted text, with any richer variants that accompanied it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Text {
    /// The plain-text representation. Always UTF-8.
    pub text: String,
    /// Raw RTF bytes, when the source also provided them.
    pub rtf: Option<Vec<u8>>,
    /// HTML source, when the source also provided it.
    pub html: Option<String>,
}

/// Rich text with every representation the source provided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RichText {
    /// Plain-text fallback, when present. Always decoded to UTF-8.
    pub plain: Option<String>,
    /// Raw RTF bytes.
    pub rtf: Option<Vec<u8>>,
    /// Raw RTFD (RTF + attachments) bytes.
    pub rtfd: Option<Vec<u8>>,
    /// HTML source.
    pub html: Option<String>,
}

impl RichText {
    /// Whether any representation is present.
    pub fn is_empty(&self) -> bool {
        self.plain.is_none() && self.rtf.is_none() && self.rtfd.is_none() && self.html.is_none()
    }

    /// The best plain-text approximation of this rich text.
    pub fn best_plain(&self) -> Option<&str> {
        self.plain.as_deref()
    }
}

/// An image payload in whichever encodings the source supplied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageData {
    /// PNG bytes, if the source provided lossless PNG.
    pub png: Option<Vec<u8>>,
    /// TIFF bytes, if the source provided raw TIFF.
    pub tiff: Option<Vec<u8>>,
    /// PDF bytes, for vector content such as copied PDF selections.
    pub pdf: Option<Vec<u8>>,
    /// Pixel width, resolved from the image header or AppKit.
    pub pixel_width: Option<u32>,
    /// Pixel height, resolved from the image header or AppKit.
    pub pixel_height: Option<u32>,
}

impl ImageData {
    /// Whether any image representation is present.
    pub fn is_empty(&self) -> bool {
        self.png.is_none() && self.tiff.is_none() && self.pdf.is_none()
    }

    /// The largest available encoded payload, in bytes.
    pub fn byte_len(&self) -> usize {
        [self.png.as_ref(), self.tiff.as_ref(), self.pdf.as_ref()]
            .into_iter()
            .flatten()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
    }

    /// The most portable encoding to hand to a preview surface.
    ///
    /// Prefers PNG (universally decodable and lossless), then PDF, then TIFF.
    pub fn preferred(&self) -> Option<(&'static str, &[u8])> {
        if let Some(bytes) = &self.png {
            return Some(("public.png", bytes));
        }
        if let Some(bytes) = &self.pdf {
            return Some(("com.adobe.pdf", bytes));
        }
        if let Some(bytes) = &self.tiff {
            return Some(("public.tiff", bytes));
        }
        None
    }
}

/// One or more file URLs copied from Finder or another file-aware app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileList {
    /// Absolute paths, in the order the source provided them.
    pub paths: Vec<PathBuf>,
}

impl FileList {
    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// How many files were copied.
    pub fn len(&self) -> usize {
        self.paths.len()
    }
}

/// A web or general URL, plus Finder-style file URLs when both were provided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebLoc {
    /// The URL string, e.g. `https://example.com/` or `file:///tmp/`.
    pub url: String,
    /// Files referenced by this item, if the URL was a file URL.
    pub files: FileList,
}

/// The interpreted content of a single pasteboard item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// Plain text, possibly with rich companions.
    Text(Text),
    /// Rich text.
    RichText(RichText),
    /// An image.
    Image(ImageData),
    /// One or more files.
    Files(FileList),
    /// A URL.
    Url(WebLoc),
    /// A color, as raw pasteboard bytes plus a resolved RGBA value when available.
    Color(ColorValue),
    /// A sound.
    Sound(SoundData),
    /// Unrecognised payloads, kept raw so they can still be replayed.
    Other,
}

impl Content {
    /// Cheaply classify this content.
    pub fn kind(&self) -> ContentKind {
        match self {
            Self::Text(_) => ContentKind::Text,
            Self::RichText(_) => ContentKind::RichText,
            Self::Image(_) => ContentKind::Image,
            Self::Files(_) => ContentKind::Files,
            Self::Url(_) => ContentKind::Url,
            Self::Color(_) => ContentKind::Color,
            Self::Sound(_) => ContentKind::Sound,
            Self::Other => ContentKind::Other,
        }
    }

    /// A one-line human-readable summary, suitable for a list row.
    ///
    /// Truncation happens on a character boundary; the returned string never
    /// splits a multi-byte code point.
    pub fn preview(&self, max_chars: usize) -> String {
        let raw = match self {
            Self::Text(text) => text.text.clone(),
            Self::RichText(rich) => rich.plain.clone().unwrap_or_else(|| "<rich text>".into()),
            Self::Image(image) => match (image.pixel_width, image.pixel_height) {
                (Some(w), Some(h)) => format!("<image {w}×{h}>"),
                _ => "<image>".to_string(),
            },
            Self::Files(files) => {
                let first = files
                    .paths
                    .first()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                if files.len() > 1 {
                    format!("{first} (+{})", files.len() - 1)
                } else {
                    first
                }
            }
            Self::Url(loc) => loc.url.clone(),
            Self::Color(color) => color
                .rgba
                .map(|(r, g, b, a)| format!("#{r:02X}{g:02X}{b:02X}{a:02X}"))
                .unwrap_or_else(|| "<color>".to_string()),
            Self::Sound(sound) => format!("<sound {} B>", sound.data.len()),
            Self::Other => "<data>".to_string(),
        };
        truncate_chars(&raw, max_chars)
    }

    /// Byte length of the heaviest payload carried by this content.
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Text(text) => text.text.len(),
            Self::RichText(rich) => {
                let candidates = [
                    rich.plain.as_ref().map(|s| s.len()),
                    rich.rtf.as_ref().map(Vec::len),
                    rich.rtfd.as_ref().map(Vec::len),
                    rich.html.as_ref().map(|s| s.len()),
                ];
                candidates.into_iter().flatten().max().unwrap_or(0)
            }
            Self::Image(image) => image.byte_len(),
            Self::Files(files) => files.paths.iter().map(|p| p.as_os_str().len()).sum(),
            Self::Url(loc) => loc.url.len(),
            Self::Color(color) => color.raw.len(),
            Self::Sound(sound) => sound.data.len(),
            Self::Other => 0,
        }
    }
}

/// A color as stored on the pasteboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorValue {
    /// The archived `NSColor` bytes exactly as AppKit produced them.
    pub raw: Vec<u8>,
    /// Resolved sRGB components, when the color could be converted.
    pub rgba: Option<(u8, u8, u8, u8)>,
}

/// A sound payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundData {
    /// The sound data in the encoding AppKit provided (usually AIFF or CAF).
    pub data: Vec<u8>,
    /// The UTI the bytes were stored under.
    pub uti: Option<String>,
}

/// Truncates `input` to at most `max_chars` characters without splitting a
/// code point or allocating when the input already fits.
pub(crate) fn truncate_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    match input.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => {
            let mut out = String::with_capacity(byte_idx + 1);
            out.push_str(&input[..byte_idx]);
            out.push('…');
            out
        }
        None => input.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel…");
        assert_eq!(truncate_chars("你好世界", 2), "你好…");
        assert_eq!(truncate_chars("anything", 0), "");
    }

    #[test]
    fn image_prefers_png_then_pdf_then_tiff() {
        let mut image = ImageData::default();
        assert!(image.preferred().is_none());
        image.tiff = Some(vec![1, 2, 3]);
        assert_eq!(image.preferred().map(|(u, _)| u), Some("public.tiff"));
        image.pdf = Some(vec![1]);
        assert_eq!(image.preferred().map(|(u, _)| u), Some("com.adobe.pdf"));
        image.png = Some(vec![9]);
        assert_eq!(image.preferred().map(|(u, _)| u), Some("public.png"));
        assert_eq!(image.byte_len(), 3);
    }

    #[test]
    fn content_preview_variants() {
        let text = Content::Text(Text {
            text: "a".repeat(100),
            ..Default::default()
        });
        assert_eq!(text.kind(), ContentKind::Text);
        assert_eq!(text.preview(5), "aaaaa…");

        let files = Content::Files(FileList {
            paths: vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
        });
        assert_eq!(files.preview(50), "/a (+2)");

        let image = Content::Image(ImageData {
            pixel_width: Some(64),
            pixel_height: Some(48),
            ..Default::default()
        });
        assert_eq!(image.preview(50), "<image 64×48>");
    }
}
