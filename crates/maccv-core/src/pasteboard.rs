//! The pasteboard engine: reading, writing, snapshots and change watching.
//!
//! # Threading
//!
//! Every AppKit type this module touches (`NSPasteboard`, `NSPasteboardItem`,
//! `NSImage`, `NSColor`) is `objc2`'s default "any thread" kind, and all of the
//! methods used here are documented by Apple as usable from background threads.
//! `Pasteboard` is therefore `Send + Sync` and safe to share. `NSImage` decoding
//! for pixel dimensions is done in a purely metadata-only way that never
//! rasterises, so it stays off the main-thread-only rendering paths.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, Message};
use objc2_app_kit::{
    NSColor, NSColorSpace, NSImage, NSPasteboard, NSPasteboardAccessBehavior,
    NSPasteboardContentsOptions, NSPasteboardItem, NSPasteboardTypeColor, NSPasteboardTypeFileURL,
    NSPasteboardTypeHTML, NSPasteboardTypePDF, NSPasteboardTypePNG, NSPasteboardTypeRTF,
    NSPasteboardTypeRTFD, NSPasteboardTypeSound, NSPasteboardTypeString, NSPasteboardTypeTIFF,
    NSPasteboardTypeURL, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use crate::content::{ColorValue, Content, FileList, ImageData, RichText, SoundData, Text, WebLoc};
use crate::error::{Error, Result};
use crate::types::{PasteboardType, WellKnownType};

/// Which pasteboard to operate on.
///
/// Most applications only ever want [`PasteboardName::General`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PasteboardName {
    /// The system-wide general pasteboard (⌘C/⌘V). Triggers macOS 27's access policy.
    #[default]
    General,
    /// A pasteboard private to this process.
    Unique,
    /// The drag-and-drop pasteboard.
    Drag,
    /// The Find panel's search text pasteboard.
    Find,
    /// The font pasteboard.
    Font,
    /// The ruler pasteboard.
    Ruler,
    /// A named pasteboard, for sharing between cooperating processes.
    Named(String),
}

impl PasteboardName {
    /// The AppKit name constant for this pasteboard, when it is a system one.
    pub fn as_system_name(&self) -> Option<&'static str> {
        match self {
            Self::General => Some("Apple CFPasteboard general"),
            Self::Drag => Some("Apple CFPasteboard drag"),
            Self::Find => Some("Apple CFPasteboard find"),
            Self::Font => Some("Apple CFPasteboard font"),
            Self::Ruler => Some("Apple CFPasteboard ruler"),
            Self::Unique | Self::Named(_) => None,
        }
    }
}

/// The general pasteboard's change counter.
///
/// `changeCount` increments on every write, by anyone. It is the cheapest
/// possible way to notice a change: a cached read, well under a microsecond.
/// Comparing two `ChangeCount`s is exact — there are no false positives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChangeCount(i64);

impl ChangeCount {
    /// Wraps a raw `NSInteger`.
    pub const fn new(raw: i64) -> Self {
        Self(raw)
    }

    /// The raw `NSInteger` value.
    pub const fn get(self) -> i64 {
        self.0
    }

    /// How many writes happened between two observations.
    ///
    /// Never negative: if `earlier` is the larger value the pasteboard was reset
    /// or wrapped, which counts as zero elapsed writes rather than a negative
    /// count.
    pub fn delta_since(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0).max(0)
    }
}

impl std::fmt::Display for ChangeCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What macOS 27's per-app policy currently allows this process to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessBehavior {
    /// No prompt has ever been shown for this app; access is not yet policed.
    Default,
    /// The system will prompt before granting pasteboard access.
    Ask,
    /// The user has permanently allowed access.
    AlwaysAllow,
    /// The user has permanently denied access.
    AlwaysDeny,
}

impl AccessBehavior {
    /// Whether reads are expected to succeed without a prompt.
    pub const fn is_unrestricted(self) -> bool {
        matches!(self, Self::Default | Self::AlwaysAllow)
    }

    /// Whether the user has denied access outright.
    pub const fn is_denied(self) -> bool {
        matches!(self, Self::AlwaysDeny)
    }

    fn from_raw(raw: NSPasteboardAccessBehavior) -> Self {
        match raw {
            NSPasteboardAccessBehavior::Ask => Self::Ask,
            NSPasteboardAccessBehavior::AlwaysAllow => Self::AlwaysAllow,
            NSPasteboardAccessBehavior::AlwaysDeny => Self::AlwaysDeny,
            _ => Self::Default,
        }
    }
}

/// A single item on the pasteboard: one logical thing the user copied.
///
/// A copy of three files is *one* item with a file list; a copy of a rich-text
/// paragraph is *one* item carrying text, RTF and HTML representations at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotItem {
    /// Every representation present, in the order AppKit reported them.
    pub representations: Vec<Representation>,
    /// The interpreted content, for display and indexing.
    pub content: Content,
}

impl SnapshotItem {
    /// The representation stored under `ty`, if present.
    pub fn get(&self, ty: impl Into<PasteboardType>) -> Option<&Representation> {
        let ty = ty.into();
        self.representations.iter().find(|r| r.ty == ty)
    }

    /// Whether this item carries the given type.
    pub fn has(&self, ty: impl Into<PasteboardType>) -> bool {
        self.get(ty).is_some()
    }

    /// Total payload size, in bytes.
    pub fn byte_len(&self) -> usize {
        self.representations.iter().map(|r| r.data.len()).sum()
    }

    /// Only the representations a "paste as plain text" action needs.
    pub fn plain_text(&self) -> Option<&str> {
        match &self.content {
            Content::Text(text) => Some(&text.text),
            Content::RichText(rich) => rich.plain.as_deref(),
            Content::Url(loc) => Some(&loc.url),
            Content::Files(files) => files.paths.first().and_then(|p| p.to_str()),
            _ => None,
        }
    }
}

/// One raw `(type, bytes)` pair as stored by AppKit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Representation {
    /// The UTI this payload was stored under.
    pub ty: PasteboardType,
    /// The exact bytes, undecoded. Replaying these reproduces the original copy.
    pub data: Vec<u8>,
}

impl Representation {
    /// The payload as UTF-8 text, if it is valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }
}

/// An immutable point-in-time capture of the whole pasteboard.
///
/// Snapshots are the unit of history: they can be stored, diffed, seralised and
/// replayed byte-for-byte via [`Pasteboard::write_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The `changeCount` this snapshot was taken at.
    pub change_count: ChangeCount,
    /// One entry per pasteboard item, in order.
    pub items: Vec<SnapshotItem>,
}

impl Snapshot {
    /// Whether the pasteboard held nothing when captured.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many items were captured.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Total payload size across every representation, in bytes.
    pub fn byte_len(&self) -> usize {
        self.items.iter().map(SnapshotItem::byte_len).sum()
    }

    /// The dominant kind of the first item, if any.
    pub fn kind(&self) -> Option<crate::content::ContentKind> {
        self.items.first().map(|i| i.content.kind())
    }

    /// A short label for the first item, for list UIs.
    pub fn preview(&self, max_chars: usize) -> String {
        match self.items.first() {
            Some(item) => item.content.preview(max_chars),
            None => String::new(),
        }
    }

    /// Every file path across every item, in order.
    ///
    /// A multi-file copy arrives as one item per file, so a UI that wants "what
    /// did the user copy?" needs this flattened view rather than
    /// `items[0]`.
    pub fn files(&self) -> Vec<std::path::PathBuf> {
        self.items
            .iter()
            .filter_map(|item| match &item.content {
                Content::Files(files) => Some(files.paths.iter()),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect()
    }

    /// The first item's text, if it has any.
    pub fn text(&self) -> Option<&str> {
        self.items.first().and_then(SnapshotItem::plain_text)
    }
}

/// Tuning for a pasteboard watcher.
///
/// A clipboard manager wants two contradictory things: notice a copy the instant
/// it happens, and burn no CPU while the user works in another app. Polling with
/// a fixed fast interval fails the second; polling slowly fails the first. The
/// adaptive back-off here satisfies both — it polls hard right after a change and
/// then decays to `max_interval` while the pasteboard stays quiet.
///
/// A `changeCount` read costs well under a microsecond on Apple silicon, so even
/// the aggressive end of this range is cheap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchConfig {
    /// Poll interval when a change is recent. Keep this small (sub-millisecond) to
    /// make the UI feel instant.
    pub min_interval: Duration,
    /// Poll interval once the pasteboard has been quiet for a while. This is what
    /// keeps an idle MacCV at effectively zero CPU.
    pub max_interval: Duration,
    /// Whether changes caused by this handle's own writes are reported.
    ///
    /// `false` avoids a feedback loop where replaying a history entry is
    /// immediately re-recorded as a new entry.
    pub skip_self_writes: bool,
    /// Thread name, visible in Instruments and `sample`.
    pub thread_name: &'static str,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_micros(400),
            max_interval: Duration::from_millis(250),
            skip_self_writes: true,
            thread_name: "maccv-watch",
        }
    }
}

impl WatchConfig {
    /// A fixed-interval watcher, ignoring this handle's own writes.
    pub fn fixed(interval: Duration) -> Self {
        Self {
            min_interval: interval,
            max_interval: interval,
            ..Self::default()
        }
    }

    /// An adaptive watcher over the given interval range.
    pub fn adaptive(min_interval: Duration, max_interval: Duration) -> Self {
        Self {
            min_interval,
            max_interval,
            ..Self::default()
        }
    }

    /// Reports this handle's own writes as well as external ones.
    pub fn including_self_writes(mut self) -> Self {
        self.skip_self_writes = false;
        self
    }

    /// Clamps the intervals into a sane range so a zero interval cannot spin a
    /// core and an inverted range cannot panic.
    fn normalised(mut self) -> Self {
        const FLOOR: Duration = Duration::from_micros(100);
        self.min_interval = self.min_interval.max(FLOOR);
        self.max_interval = self.max_interval.max(self.min_interval);
        self
    }
}

/// Handle to a running change watcher. Dropping it stops the thread.
#[derive(Debug)]
pub struct WatchHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WatchHandle {
    /// Requests that the watcher stop, without waiting for the thread to exit.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Whether the stop flag has been set.
    pub fn is_stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Waits for the watcher thread to finish. Called automatically on drop.
    pub fn join(mut self) {
        self.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Handle on a live NSPasteboard.
///
/// Cloning is cheap: it retains the same underlying Objective-C object and shares
/// the self-write bookkeeping.
#[derive(Debug)]
pub struct Pasteboard {
    inner: Retained<NSPasteboard>,
    /// The `changeCount` produced by this handle's own most recent write, so a
    /// watcher can skip changes it caused itself.
    last_self_write: Arc<AtomicI64>,
}

impl Clone for Pasteboard {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            last_self_write: Arc::clone(&self.last_self_write),
        }
    }
}

impl Pasteboard {
    /// Opens the system-wide general pasteboard.
    pub fn general() -> Self {
        Self::from_retained(NSPasteboard::generalPasteboard())
    }

    /// Opens the drag-and-drop pasteboard.
    pub fn drag() -> Self {
        Self::from_retained(NSPasteboard::pasteboardWithName(unsafe {
            objc2_app_kit::NSPasteboardNameDrag
        }))
    }

    /// Creates a pasteboard private to this process.
    pub fn unique() -> Self {
        Self::from_retained(NSPasteboard::pasteboardWithUniqueName())
    }

    /// Opens a named pasteboard, creating it if needed.
    pub fn named(name: &str) -> Self {
        let ns_name = NSString::from_str(name);
        Self::from_retained(NSPasteboard::pasteboardWithName(&ns_name))
    }

    /// Reopens a pasteboard from the name reported by [`Pasteboard::name`].
    ///
    /// This is how a watcher thread reattaches to the same pasteboard without
    /// carrying an Objective-C object across the thread boundary. It works for
    /// system pasteboards too, since they are addressed by the same names.
    pub fn by_name(name: &str) -> Self {
        Self::named(name)
    }

    /// Opens an arbitrary pasteboard.
    pub fn with_name(name: &PasteboardName) -> Self {
        match name {
            PasteboardName::General => Self::general(),
            PasteboardName::Unique => Self::unique(),
            PasteboardName::Drag => Self::drag(),
            PasteboardName::Named(n) => Self::named(n),
            PasteboardName::Find => Self::from_retained(NSPasteboard::pasteboardWithName(unsafe {
                objc2_app_kit::NSPasteboardNameFind
            })),
            PasteboardName::Font => Self::from_retained(NSPasteboard::pasteboardWithName(unsafe {
                objc2_app_kit::NSPasteboardNameFont
            })),
            PasteboardName::Ruler => {
                Self::from_retained(NSPasteboard::pasteboardWithName(unsafe {
                    objc2_app_kit::NSPasteboardNameRuler
                }))
            }
        }
    }

    fn from_retained(inner: Retained<NSPasteboard>) -> Self {
        Self {
            inner,
            last_self_write: Arc::new(AtomicI64::new(i64::MIN)),
        }
    }

    /// The AppKit name of this pasteboard.
    pub fn name(&self) -> String {
        self.inner.name().to_string()
    }

    /// Reads the change counter. Costs a cached property load.
    #[inline]
    pub fn change_count(&self) -> ChangeCount {
        ChangeCount(self.inner.changeCount() as i64)
    }

    /// The change counter this handle last produced itself, if any.
    pub fn last_self_write(&self) -> Option<ChangeCount> {
        match self.last_self_write.load(Ordering::Relaxed) {
            i64::MIN => None,
            raw => Some(ChangeCount(raw)),
        }
    }

    /// Whether `count` was produced by a write from this handle.
    ///
    /// A watcher uses this to avoid re-indexing the item it just wrote back.
    pub fn is_self_write(&self, count: ChangeCount) -> bool {
        self.last_self_write() == Some(count)
    }

    /// macOS 27's current access policy for this pasteboard.
    pub fn access_behavior(&self) -> AccessBehavior {
        AccessBehavior::from_raw(self.inner.accessBehavior())
    }

    /// The types present on the pasteboard right now, in AppKit's order.
    pub fn types(&self) -> Vec<PasteboardType> {
        self.inner
            .types()
            .map(|ns| {
                ns.iter()
                    .map(|t| unsafe { PasteboardType::from_ns(&t) })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether any representation on the pasteboard conforms to `ty`.
    ///
    /// Cheap: AppKit answers from type metadata.
    pub fn can_read(&self, ty: WellKnownType) -> bool {
        let list = NSArray::from_retained_slice(&[ns_type(ty)]);
        self.inner.canReadItemWithDataConformingToTypes(&list)
    }

    /// Convenience: the plain-text content of the first item, if any.
    pub fn get_string(&self) -> Option<String> {
        self.snapshot()
            .ok()?
            .items
            .first()
            .and_then(|i| i.plain_text().map(ToOwned::to_owned))
    }

    /// Captures the entire pasteboard, every item and every representation.
    ///
    /// This is the primary read API. It copies payload bytes exactly once; no
    /// decoding, transcoding or re-encoding happens on this path.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let change_count = self.change_count();
        let Some(items) = self.inner.pasteboardItems() else {
            // An absent item array means either "empty" or "refused". Only the
            // access policy distinguishes them, so consult it rather than
            // reporting an empty clipboard the user never had.
            if self.access_behavior().is_denied() {
                return Err(Error::AccessDenied);
            }
            return Ok(Snapshot {
                change_count,
                items: Vec::new(),
            });
        };

        let mut out = Vec::with_capacity(items.len());
        for item in items.iter() {
            out.push(self.capture_item(&item));
        }

        Ok(Snapshot {
            change_count,
            items: out,
        })
    }

    /// Captures one item's representations and classifies its content.
    fn capture_item(&self, item: &NSPasteboardItem) -> SnapshotItem {
        let ns_types = item.types();
        let mut representations = Vec::with_capacity(ns_types.len());
        for ty in ns_types.iter() {
            let ty = unsafe { PasteboardType::from_ns(&ty) };
            // `dataForType:` returns the bytes without conversion — the most
            // faithful accessor. `stringForType:` would coerce encodings.
            let data = item.dataForType(&ty_ns(ty.as_str())).map(|d| d.to_vec());
            if let Some(data) = data {
                representations.push(Representation { ty, data });
            }
        }

        let content = classify(&representations);
        SnapshotItem {
            representations,
            content,
        }
    }

    /// Writes raw representations as a single item.
    ///
    /// This is the faithful reverse of [`Pasteboard::snapshot`]. The pasteboard is
    /// cleared first — `writeObjects:` requires it.
    pub fn write_raw(&self, representations: &[(PasteboardType, Vec<u8>)]) -> Result<ChangeCount> {
        if representations.is_empty() {
            return Err(Error::InvalidPayload("no representations to write".into()));
        }

        let objects = build_item_objects(vec![representations.to_vec()])?;
        self.commit(&objects)
    }

    /// Writes several items at once, each item being its own set of representations.
    ///
    /// This is the multi-item writer behind [`Pasteboard::write_snapshot`] and is
    /// what makes a three-file Finder copy land as three items again.
    pub fn write_items<I>(&self, items: I) -> Result<ChangeCount>
    where
        I: IntoIterator<Item = Vec<(PasteboardType, Vec<u8>)>>,
    {
        let objects = build_item_objects(items)?;
        self.commit(&objects)
    }

    /// Writes an entire captured snapshot back to the pasteboard.
    ///
    /// Every representation of every item is replayed exactly as captured, so
    /// pasting into the original app reproduces the original formatting.
    pub fn write_snapshot(&self, snapshot: &Snapshot) -> Result<ChangeCount> {
        if snapshot.items.is_empty() {
            return Err(Error::InvalidPayload("snapshot has no items".into()));
        }
        self.write_items(snapshot.items.iter().map(|item| {
            item.representations
                .iter()
                .map(|r| (r.ty.clone(), r.data.clone()))
                .collect::<Vec<_>>()
        }))
    }

    /// Writes plain text to the pasteboard.
    pub fn set_string(&self, text: &str) -> Result<ChangeCount> {
        self.write_raw(&[(WellKnownType::String.into(), text.as_bytes().to_vec())])
    }

    /// Writes rich text, supplying as many representations as the caller has.
    pub fn set_rich_text(&self, rich: &RichText) -> Result<ChangeCount> {
        let mut reps: Vec<(PasteboardType, Vec<u8>)> = Vec::new();
        if let Some(plain) = &rich.plain {
            reps.push((WellKnownType::String.into(), plain.as_bytes().to_vec()));
        }
        if let Some(html) = &rich.html {
            reps.push((WellKnownType::Html.into(), html.as_bytes().to_vec()));
        }
        if let Some(rtf) = &rich.rtf {
            reps.push((WellKnownType::Rtf.into(), rtf.clone()));
        }
        if let Some(rtfd) = &rich.rtfd {
            reps.push((WellKnownType::Rtfd.into(), rtfd.clone()));
        }
        self.write_raw(&reps)
    }

    /// Writes an image, preferring PNG and falling back to TIFF.
    pub fn set_image(&self, image: &ImageData) -> Result<ChangeCount> {
        let mut reps: Vec<(PasteboardType, Vec<u8>)> = Vec::new();
        if let Some(tiff) = &image.tiff {
            reps.push((WellKnownType::Tiff.into(), tiff.clone()));
        }
        if let Some(png) = &image.png {
            reps.push((WellKnownType::Png.into(), png.clone()));
        }
        if let Some(pdf) = &image.pdf {
            reps.push((WellKnownType::Pdf.into(), pdf.clone()));
        }
        self.write_raw(&reps)
    }

    /// Writes file URLs, so Finder and other apps see a real multi-file copy.
    pub fn set_files(&self, files: &FileList) -> Result<ChangeCount> {
        if files.is_empty() {
            return Err(Error::InvalidPayload("file list is empty".into()));
        }
        let mut objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
            Vec::with_capacity(files.len());
        for path in &files.paths {
            let Some(path_str) = path.to_str() else {
                return Err(Error::InvalidUtf8);
            };
            let ns_path = NSString::from_str(path_str);
            let url = NSURL::fileURLWithPath(&ns_path);
            objects.push(ProtocolObject::from_retained(url));
        }
        self.commit(&objects)
    }

    /// Writes a color, by archiving an `NSColor` so other apps see a real swatch.
    pub fn set_color(&self, color: &ColorValue) -> Result<ChangeCount> {
        let Some((r, g, b, a)) = color.rgba else {
            // Fall back to replaying the archived bytes verbatim.
            return self.write_raw(&[(WellKnownType::Color.into(), color.raw.clone())]);
        };
        let ns_color = NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
            f64::from(a) / 255.0,
        );
        let objects = vec![ProtocolObject::from_retained(ns_color)];
        self.commit(&objects)
    }

    /// Clears the pasteboard without writing anything.
    pub fn clear(&self) -> ChangeCount {
        let count = self.inner.clearContents() as i64;
        self.last_self_write.store(count, Ordering::Relaxed);
        ChangeCount(count)
    }

    /// Whether the pasteboard currently holds no items at all.
    pub fn is_empty(&self) -> bool {
        self.inner
            .pasteboardItems()
            .map(|items| items.is_empty())
            .unwrap_or(true)
    }

    /// Clears and then writes `objects`, recording the resulting change count.
    ///
    /// `prepareForNewContentsWithOptions:` is called first because `writeObjects:`
    /// is documented as requiring the pasteboard to be invalidated beforehand.
    /// Its return value is deliberately *not* used as an error signal — it is a
    /// change count, so treating `0` as failure would be a guess. The authoritative
    /// signal is `writeObjects:` itself, and the access policy explains *why* a
    /// write was refused.
    fn commit(
        &self,
        objects: &[Retained<ProtocolObject<dyn NSPasteboardWriting>>],
    ) -> Result<ChangeCount> {
        self.inner
            .prepareForNewContentsWithOptions(NSPasteboardContentsOptions(0));

        let array = NSArray::from_retained_slice(objects);
        if !self.inner.writeObjects(&array) {
            return Err(if self.access_behavior().is_denied() {
                Error::AccessDenied
            } else {
                Error::WriteFailed
            });
        }

        let count = self.change_count();
        self.last_self_write.store(count.get(), Ordering::Relaxed);
        Ok(count)
    }

    /// Watches for changes using an explicit [`WatchConfig`].
    ///
    /// This is the primitive every other watcher method delegates to.
    ///
    /// # Threading
    ///
    /// The watcher runs on its own thread and opens **its own** handle rather than
    /// borrowing `self`. `objc2` deliberately does not mark `NSObject` subclasses
    /// `Send`, and moving an `NSPasteboard` across threads to satisfy a
    /// convenience API would be a silent lie about AppKit's threading contract.
    /// Only `Send` data crosses the boundary: the pasteboard's name and the
    /// shared self-write counter.
    pub fn watch_with<F>(&self, config: WatchConfig, mut on_change: F) -> WatchHandle
    where
        F: FnMut(ChangeCount) + Send + 'static,
    {
        let config = config.normalised();
        let pasteboard_name = self.name();
        let last_self_write = Arc::clone(&self.last_self_write);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name(config.thread_name.to_owned())
            .spawn(move || {
                // Opened inside the thread, so it never needs to be `Send`.
                let pb = Pasteboard::by_name(&pasteboard_name);
                let mut last = pb.change_count();
                let mut interval = config.min_interval;

                while !stop_flag.load(Ordering::Relaxed) {
                    thread::sleep(interval);
                    let now = pb.change_count();

                    if now == last {
                        // Quiet: back off towards `max_interval`.
                        interval = (interval * 2).min(config.max_interval);
                        continue;
                    }
                    last = now;
                    interval = config.min_interval;

                    if config.skip_self_writes
                        && last_self_write.load(Ordering::Relaxed) == now.get()
                    {
                        continue;
                    }
                    on_change(now);
                }
            })
            .expect("failed to spawn maccv watch thread");

        WatchHandle {
            stop,
            thread: Some(handle),
        }
    }

    /// Watches for changes at a fixed interval, ignoring this handle's own writes.
    pub fn watch<F>(&self, interval: Duration, on_change: F) -> WatchHandle
    where
        F: FnMut(ChangeCount) + Send + 'static,
    {
        self.watch_with(WatchConfig::fixed(interval), on_change)
    }

    /// Watches at a fixed interval, reporting this handle's own writes too.
    pub fn raw_watch<F>(&self, interval: Duration, on_change: F) -> WatchHandle
    where
        F: FnMut(ChangeCount) + Send + 'static,
    {
        self.watch_with(
            WatchConfig::fixed(interval).including_self_writes(),
            on_change,
        )
    }

    /// Watches with a self-tuning poll interval, ignoring this handle's own writes.
    ///
    /// Starts at `min_interval` so a burst of copies is picked up almost
    /// instantly, then backs off geometrically towards `max_interval` while the
    /// pasteboard is quiet. On any change the interval snaps back to
    /// `min_interval`. This gives interactive latency without burning idle CPU.
    pub fn watch_adaptive<F>(
        &self,
        min_interval: Duration,
        max_interval: Duration,
        on_change: F,
    ) -> WatchHandle
    where
        F: FnMut(ChangeCount) + Send + 'static,
    {
        self.watch_with(WatchConfig::adaptive(min_interval, max_interval), on_change)
    }

    /// Measures the round-trip cost of one `changeCount` read.
    ///
    /// Useful for sanity checks and for choosing a poll interval at runtime.
    pub fn benchmark_change_count(&self, iterations: u32) -> Duration {
        let start = Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(self.change_count());
        }
        start.elapsed()
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Maps a well-known type to the AppKit constant, avoiding a string round-trip.
fn ns_type(ty: WellKnownType) -> Retained<NSString> {
    // SAFETY: these are immutable constants vended by AppKit, valid for the
    // lifetime of the process. `retain` yields a `Retained` whose ownership
    // matches the fallback path, so callers can treat both branches alike.
    unsafe {
        match ty {
            WellKnownType::String => NSPasteboardTypeString.retain(),
            WellKnownType::Html => NSPasteboardTypeHTML.retain(),
            WellKnownType::Rtf => NSPasteboardTypeRTF.retain(),
            WellKnownType::Rtfd => NSPasteboardTypeRTFD.retain(),
            WellKnownType::Png => NSPasteboardTypePNG.retain(),
            WellKnownType::Tiff => NSPasteboardTypeTIFF.retain(),
            WellKnownType::Pdf => NSPasteboardTypePDF.retain(),
            WellKnownType::Url => NSPasteboardTypeURL.retain(),
            WellKnownType::FileUrl => NSPasteboardTypeFileURL.retain(),
            WellKnownType::Color => NSPasteboardTypeColor.retain(),
            WellKnownType::Sound => NSPasteboardTypeSound.retain(),
            // Types without an AppKit constant fall back to their raw UTI.
            other => NSString::from_str(other.as_str()),
        }
    }
}

/// Builds an `NSString` for an arbitrary UTI.
fn ty_ns(identifier: &str) -> Retained<NSString> {
    // AppKit's callbacks only compare the pointer identity for well-known
    // constants; for custom types a fresh NSString is correct because AppKit
    // compares by string value.
    NSString::from_str(identifier)
}

/// Turns raw representations into one `NSPasteboardItem` per entry.
fn build_item_objects<I>(items: I) -> Result<Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>>>
where
    I: IntoIterator<Item = Vec<(PasteboardType, Vec<u8>)>>,
{
    let mut objects = Vec::new();
    for reps in items {
        if reps.is_empty() {
            continue;
        }
        let item = NSPasteboardItem::new();
        for (ty, data) in reps {
            let ns_data = NSData::with_bytes(&data);
            let ns_ty = ty_ns(ty.as_str());
            if !item.setData_forType(&ns_data, &ns_ty) {
                return Err(Error::WriteFailed);
            }
        }
        objects.push(ProtocolObject::from_retained(item));
    }
    if objects.is_empty() {
        return Err(Error::InvalidPayload("nothing writable".into()));
    }
    Ok(objects)
}

/// Interprets a set of raw representations as the most specific content type.
///
/// Ordering matters: file lists beat URLs, images beat text, rich text beats
/// plain text — while every raw representation is preserved regardless.
fn classify(reps: &[Representation]) -> Content {
    let find = |ty: WellKnownType| -> Option<&Representation> {
        reps.iter().find(|r| r.ty.as_str() == ty.as_str())
    };

    // Files: `public.file-url` entries, in order.
    let file_reps: Vec<&Representation> = reps
        .iter()
        .filter(|r| r.ty.as_str() == WellKnownType::FileUrl.as_str())
        .collect();
    if !file_reps.is_empty() {
        let mut paths = Vec::with_capacity(file_reps.len());
        for rep in &file_reps {
            if let Some(path) = file_url_to_path(rep) {
                paths.push(path);
            }
        }
        if !paths.is_empty() {
            return Content::Files(FileList { paths });
        }
    }

    // Images.
    let png = find(WellKnownType::Png).map(|r| r.data.clone());
    let tiff = find(WellKnownType::Tiff).map(|r| r.data.clone());
    let pdf = find(WellKnownType::Pdf).map(|r| r.data.clone());
    let mut image = ImageData {
        png,
        tiff,
        pdf,
        pixel_width: None,
        pixel_height: None,
    };
    if !image.is_empty() {
        let (w, h) = image_dimensions(&image);
        image.pixel_width = w;
        image.pixel_height = h;
        return Content::Image(image);
    }

    // Colors.
    if let Some(rep) = find(WellKnownType::Color) {
        return Content::Color(ColorValue {
            raw: rep.data.clone(),
            rgba: decode_color(&rep.data),
        });
    }

    // Sounds.
    if let Some(rep) = find(WellKnownType::Sound) {
        return Content::Sound(SoundData {
            data: rep.data.clone(),
            uti: Some(rep.ty.as_str().to_owned()),
        });
    }

    // Rich text.
    let plain_rep = find(WellKnownType::String);
    let rtf = find(WellKnownType::Rtf).map(|r| r.data.clone());
    let rtfd = find(WellKnownType::Rtfd).map(|r| r.data.clone());
    let html = find(WellKnownType::Html)
        .and_then(|r| std::str::from_utf8(&r.data).ok())
        .map(ToOwned::to_owned);
    let plain = plain_rep
        .and_then(|r| std::str::from_utf8(&r.data).ok())
        .map(ToOwned::to_owned);

    if rtf.is_some() || rtfd.is_some() {
        return Content::RichText(RichText {
            plain,
            rtf,
            rtfd,
            html,
        });
    }
    if let Some(html) = html {
        // HTML with no RTF is still rich text; keep the plain fallback.
        return Content::RichText(RichText {
            plain,
            rtf: None,
            rtfd: None,
            html: Some(html),
        });
    }

    // Plain text.
    if let Some(text) = plain {
        return Content::Text(Text {
            text,
            rtf: None,
            html: None,
        });
    }

    // A bare URL with no file scheme.
    if let Some(rep) = find(WellKnownType::Url) {
        if let Some(url) = rep.as_str() {
            return Content::Url(WebLoc {
                url: url.to_owned(),
                files: FileList::default(),
            });
        }
    }

    Content::Other
}

/// Extracts a filesystem path from a `public.file-url` payload.
fn file_url_to_path(rep: &Representation) -> Option<std::path::PathBuf> {
    let raw = rep.as_str()?;
    // AppKit stores these as UTF-8 percent-encoded `file://` URLs.
    let stripped = raw.strip_prefix("file://")?;
    let decoded = percent_decode(stripped);
    Some(std::path::PathBuf::from(decoded))
}

/// Decodes `%XX` escapes. Pasteboard file URLs are the only place this is needed,
/// so a tiny dedicated decoder beats pulling in a URL crate.
fn percent_decode(input: &str) -> String {
    if !input.contains('%') {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_owned())
}

/// Resolves pixel dimensions, cheapest source first.
///
/// PNG dimensions come from the IHDR header — a 24-byte read, no decode. Other
/// formats fall back to `NSImage`, which reads metadata without rasterising.
fn image_dimensions(image: &ImageData) -> (Option<u32>, Option<u32>) {
    if let Some(png) = &image.png {
        if let Some(dims) = png_dimensions(png) {
            return dims;
        }
    }
    for candidate in [
        image.tiff.as_deref(),
        image.png.as_deref(),
        image.pdf.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let data = NSData::with_bytes(candidate);
        if let Some(ns_image) = NSImage::initWithData(NSImage::alloc(), &data) {
            let size = ns_image.size();
            if size.width > 0.0 && size.height > 0.0 {
                return (Some(size.width as u32), Some(size.height as u32));
            }
        }
    }
    (None, None)
}

/// Reads width/height straight out of a PNG's IHDR chunk.
fn png_dimensions(bytes: &[u8]) -> Option<(Option<u32>, Option<u32>)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE {
        return None;
    }
    // Bytes 16..20 and 20..24 are the IHDR width and height, big-endian.
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((Some(width), Some(height)))
}

/// Resolves an archived `NSColor` to sRGB bytes, when it can be unarchived.
fn decode_color(raw: &[u8]) -> Option<(u8, u8, u8, u8)> {
    use objc2_foundation::NSKeyedUnarchiver;

    let data = NSData::with_bytes(raw);
    // SAFETY: `NSColor` is the class the archive is expected to contain; a
    // mismatched archive returns an `Err` rather than misbehaving.
    let object = unsafe {
        NSKeyedUnarchiver::unarchivedObjectOfClass_fromData_error(objc2::class!(NSColor), &data)
    }
    .ok()?;

    let color: &NSColor = object.downcast_ref()?;
    // Convert into a known space before asking for components, so exotic or
    // pattern colors do not produce garbage.
    let srgb = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;

    let mut red: f64 = 0.0;
    let mut green: f64 = 0.0;
    let mut blue: f64 = 0.0;
    let mut alpha: f64 = 1.0;
    // SAFETY: the four out-parameters are valid, writable, correctly typed
    // locals that outlive the call.
    unsafe {
        srgb.getRed_green_blue_alpha(&raw mut red, &raw mut green, &raw mut blue, &raw mut alpha);
    }
    Some((
        (red.clamp(0.0, 1.0) * 255.0).round() as u8,
        (green.clamp(0.0, 1.0) * 255.0).round() as u8,
        (blue.clamp(0.0, 1.0) * 255.0).round() as u8,
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_plain_and_escaped() {
        assert_eq!(percent_decode("/tmp"), "/tmp");
        assert_eq!(percent_decode("/tmp/a%20b"), "/tmp/a b");
        assert_eq!(percent_decode("%E4%BD%A0"), "你");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("bad%ZZ"), "bad%ZZ");
    }

    #[test]
    fn png_header_is_parsed() {
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02,
        ];
        assert_eq!(png_dimensions(PNG), Some((Some(1), Some(1))));
        assert_eq!(png_dimensions(b"not a png at all........"), None);
        assert_eq!(png_dimensions(&[]), None);
    }

    #[test]
    fn classify_text() {
        let reps = vec![Representation {
            ty: WellKnownType::String.into(),
            data: b"hello".to_vec(),
        }];
        assert!(matches!(classify(&reps), Content::Text(t) if t.text == "hello"));
    }

    #[test]
    fn classify_prefers_rtf_over_plain() {
        let reps = vec![
            Representation {
                ty: WellKnownType::String.into(),
                data: b"hello".to_vec(),
            },
            Representation {
                ty: WellKnownType::Rtf.into(),
                data: b"{\\rtf1 hello}".to_vec(),
            },
        ];
        match classify(&reps) {
            Content::RichText(rich) => {
                assert_eq!(rich.plain.as_deref(), Some("hello"));
                assert!(rich.rtf.is_some());
            }
            other => panic!("expected rich text, got {other:?}"),
        }
    }

    #[test]
    fn classify_files_takes_priority() {
        let reps = vec![
            Representation {
                ty: WellKnownType::String.into(),
                data: b"/tmp".to_vec(),
            },
            Representation {
                ty: WellKnownType::FileUrl.into(),
                data: b"file:///tmp/a%20b.txt".to_vec(),
            },
        ];
        match classify(&reps) {
            Content::Files(files) => {
                assert_eq!(files.paths, vec![std::path::PathBuf::from("/tmp/a b.txt")]);
            }
            other => panic!("expected files, got {other:?}"),
        }
    }

    #[test]
    fn classify_image_from_png() {
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x30, 0x08, 0x02,
        ];
        let reps = vec![Representation {
            ty: WellKnownType::Png.into(),
            data: PNG.to_vec(),
        }];
        match classify(&reps) {
            Content::Image(image) => {
                assert_eq!(image.pixel_width, Some(64));
                assert_eq!(image.pixel_height, Some(48));
            }
            other => panic!("expected image, got {other:?}"),
        }
    }

    #[test]
    fn classify_unknown_is_other() {
        let reps = vec![Representation {
            ty: PasteboardType::new("com.example.mystery"),
            data: vec![1, 2, 3],
        }];
        assert!(matches!(classify(&reps), Content::Other));
    }

    #[test]
    fn change_count_delta() {
        assert_eq!(ChangeCount::new(10).delta_since(ChangeCount::new(4)), 6);
        assert_eq!(ChangeCount::new(4).delta_since(ChangeCount::new(10)), 0);
    }

    #[test]
    fn access_behavior_mapping() {
        assert!(AccessBehavior::AlwaysAllow.is_unrestricted());
        assert!(AccessBehavior::Default.is_unrestricted());
        assert!(!AccessBehavior::Ask.is_unrestricted());
        assert!(AccessBehavior::AlwaysDeny.is_denied());
    }
}
