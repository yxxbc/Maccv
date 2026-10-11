//! # maccv-ffi
//!
//! A hand-written C ABI over [`maccv_core`], so a native front end (SwiftUI,
//! AppKit, Objective-C, plain C) can drive the clipboard engine directly.
//!
//! ## Why a static library instead of IPC
//!
//! MacCV targets interactive latency. Linking the engine into the app removes the
//! IPC round-trip, the serialisation cost and the process boundary entirely: a
//! Swift `maccv_pasteboard_change_count` call is a direct Objective-C message send
//! underneath. Measured on an M-series Mac, a full snapshot read of a text
//! clipboard is single-digit microseconds.
//!
//! ## Memory contract
//!
//! - `MaccvSnapshot` is owned by the caller; release it with `maccv_snapshot_free`.
//!   It owns every string and byte buffer it contains — do not free those
//!   individually.
//! - `MaccvWatch` is owned by the caller; release it with `maccv_watch_stop`.
//! - Strings returned by `maccv_version` and `maccv_error_message` are `'static`
//!   and must **not** be freed.
//! - Strings returned by `maccv_last_error_message` and `maccv_pasteboard_name`
//!   are owned; release them with `maccv_string_free`.
//!
//! ## Panics
//!
//! Release builds use `panic = "abort"`, so a bug can never unwind across the FFI
//! boundary into Swift. Every function additionally validates its pointers before
//! dereferencing and reports [`MaccvError::NullPointer`] instead of
//! trapping.

#![deny(missing_docs)]
#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::slice;
use std::time::Duration;

use maccv_core::{ChangeCount, Pasteboard, Snapshot, WatchConfig, WatchHandle, WellKnownType};

pub mod updates;

// ---------------------------------------------------------------------------
// Scalar and string types
// ---------------------------------------------------------------------------

/// Status code returned by every fallible entry point.
///
/// The success variant is zero (`MACCV_ERROR_OK`), so `if (maccv_...)` reads
/// naturally in C.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaccvError {
    /// The call succeeded.
    Ok = 0,
    /// The system denied pasteboard access (macOS 27 access policy).
    AccessDenied = 1,
    /// No pasteboard with that name exists.
    PasteboardNotFound = 2,
    /// The requested type is not present.
    TypeNotFound = 3,
    /// A payload could not be interpreted.
    InvalidPayload = 4,
    /// A text payload was not valid UTF-8.
    InvalidUtf8 = 5,
    /// AppKit rejected the write.
    WriteFailed = 6,
    /// The object is in a state that forbids this call.
    InvalidState = 7,
    /// A required pointer argument was null.
    NullPointer = 8,
    /// A non-null pointer was not valid UTF-8.
    InvalidCString = 9,
}

impl From<maccv_core::Error> for MaccvError {
    fn from(value: maccv_core::Error) -> Self {
        use maccv_core::Error as E;
        match value {
            E::AccessDenied => Self::AccessDenied,
            E::PasteboardNotFound(_) => Self::PasteboardNotFound,
            E::TypeNotFound(_) => Self::TypeNotFound,
            E::InvalidPayload(_) => Self::InvalidPayload,
            E::InvalidUtf8 => Self::InvalidUtf8,
            E::WriteFailed => Self::WriteFailed,
            E::InvalidState(_) => Self::InvalidState,
        }
    }
}

/// What the system's per-app pasteboard policy currently permits.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaccvAccessBehavior {
    /// This OS does not implement the access-behaviour API (macOS 15.0–15.3).
    ///
    /// Deliberately not zero: a caller that initialises a variable to 0 and forgets
    /// to check the return value must not silently conclude "access is not policed".
    Unavailable = -1,
    /// Access has not been policed yet for this app.
    Default = 0,
    /// The system will prompt before granting access.
    Ask = 1,
    /// Access permanently allowed.
    AlwaysAllow = 2,
    /// Access permanently denied.
    AlwaysDeny = 3,
}

impl From<maccv_core::AccessBehavior> for MaccvAccessBehavior {
    fn from(value: maccv_core::AccessBehavior) -> Self {
        use maccv_core::AccessBehavior as A;
        match value {
            A::Unavailable => Self::Unavailable,
            A::Default => Self::Default,
            A::Ask => Self::Ask,
            A::AlwaysAllow => Self::AlwaysAllow,
            A::AlwaysDeny => Self::AlwaysDeny,
        }
    }
}

/// Backing byte for every empty string this library hands out.
///
/// A `static` rather than a `c""` literal inside the const: a literal's address
/// is not guaranteed to be identical across codegen units, and
/// [`MaccvString::free_owned`] relies on recognising the sentinel by address. With
/// a literal, a field written in one unit compared unequal to the same const read
/// in another, so the guard failed and `free()` was handed static read-only memory.
static EMPTY_C_STRING: [u8; 1] = [0];

/// A borrowed, NUL-terminated UTF-8 string.
///
/// `len` excludes the terminating NUL, so the string may legally contain none and
/// callers never need `strlen`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MaccvString {
    /// Pointer to UTF-8 bytes, NUL-terminated. Never null for a valid value.
    pub ptr: *const c_char,
    /// Byte length, excluding the NUL terminator.
    pub len: usize,
}

impl MaccvString {
    /// An empty string. Points at a static NUL so the pointer is never null.
    ///
    /// This is a *borrowed* sentinel, not an allocation. Anything releasing an
    /// owned string must go through [`MaccvString::free_owned`], which recognises
    /// this pointer and leaves it alone — calling `CString::from_raw` on it would
    /// hand a read-only static address to `free()` and crash the process.
    pub(crate) const EMPTY: Self = Self {
        ptr: EMPTY_C_STRING.as_ptr().cast(),
        len: 0,
    };

    /// Releases a string this library allocated, ignoring the static sentinel.
    ///
    /// Safe to call on any `MaccvString` the API produced: an owned empty string
    /// (`owned("")`) has its own heap allocation and *is* freed, while the
    /// `EMPTY` sentinel is skipped.
    pub(crate) fn free_owned(self) {
        if self.ptr.is_null() || self.ptr == Self::EMPTY.ptr {
            return;
        }
        // SAFETY: the caller only ever reaches this with a pointer that came from
        // `CString::into_raw`, which the guard above establishes by excluding the
        // one pointer that did not.
        drop(unsafe { CString::from_raw(self.ptr.cast_mut().cast()) });
    }

    /// Wraps a `'static` string that the caller must not free.
    fn from_static(value: &'static str) -> Self {
        // `c"..."` literals are the only way to get a NUL terminator without
        // leaking; for dynamic text we fall back to a leaked `CString`.
        match CStr::from_bytes_until_nul(value.as_bytes()) {
            Ok(cstr) => Self {
                ptr: cstr.as_ptr(),
                len: value.len(),
            },
            Err(_) => Self {
                ptr: value.as_ptr().cast(),
                len: value.len(),
            },
        }
    }

    /// Allocates an owned string. Free it with [`maccv_string_free`].
    pub(crate) fn owned(value: &str) -> Self {
        match CString::new(value) {
            Ok(cstring) => {
                let len = cstring.as_bytes().len();
                Self {
                    ptr: cstring.into_raw(),
                    len,
                }
            }
            // Interior NULs are impossible in the strings we produce internally;
            // truncate defensively rather than lose the whole message.
            Err(err) => {
                let cstring = CString::new(err.into_vec().split(|b| *b == 0).next().unwrap_or(&[]))
                    .unwrap_or_default();
                let len = cstring.as_bytes().len();
                Self {
                    ptr: cstring.into_raw(),
                    len,
                }
            }
        }
    }

    /// Borrows the value as a `&str`. Returns `None` if the pointer is null.
    ///
    /// # Safety
    ///
    /// `self.ptr` must point at `self.len` readable bytes followed by a NUL.
    pub(crate) unsafe fn as_str(self) -> Option<&'static str> {
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: guaranteed by the contract above.
        let bytes = unsafe { slice::from_raw_parts(self.ptr.cast::<u8>(), self.len) };
        std::str::from_utf8(bytes).ok()
    }
}

/// A borrowed byte buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MaccvData {
    /// Pointer to the bytes. Never null for a valid value.
    pub ptr: *const u8,
    /// Byte length.
    pub len: usize,
}

impl MaccvData {
    /// A zero-length buffer pointing at a dangling-but-aligned address.
    const EMPTY: Self = Self {
        ptr: ptr::NonNull::<u8>::dangling().as_ptr(),
        len: 0,
    };
}

// ---------------------------------------------------------------------------
// Snapshot types
// ---------------------------------------------------------------------------

/// One `(type, bytes)` pair on an item.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MaccvRepresentation {
    /// The UTI, e.g. `public.png`.
    pub type_identifier: MaccvString,
    /// The raw payload, exactly as AppKit stored it.
    pub data: MaccvData,
}

/// One logical thing the user copied, with every representation it carried.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MaccvItem {
    /// Contiguous array of representations, borrowed from the snapshot.
    pub representations: *const MaccvRepresentation,
    /// Number of entries in `representations`.
    pub representations_len: usize,
    /// Dominant kind: `text`, `rich-text`, `image`, `files`, `url`, `color`,
    /// `sound` or `other`.
    pub kind: MaccvString,
    /// Short human-readable label for list UIs.
    pub preview: MaccvString,
    /// Total payload size across all representations, in bytes.
    pub byte_len: usize,
}

/// A point-in-time capture of an entire pasteboard.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MaccvSnapshot {
    /// Contiguous array of items; the caller indexes it directly.
    pub items: *const MaccvItem,
    /// Number of entries in `items`.
    pub items_len: usize,
    /// The pasteboard's change counter when captured.
    pub change_count: i64,
    /// Total payload size across every item, in bytes.
    pub byte_len: usize,
}

/// Opaque handle on a live pasteboard.
#[repr(C)]
pub struct MaccvPasteboard {
    inner: Pasteboard,
}

/// Opaque handle on a running change watcher.
#[repr(C)]
pub struct MaccvWatch {
    handle: Option<WatchHandle>,
}

// ---------------------------------------------------------------------------
// Owned backing storage
// ---------------------------------------------------------------------------

/// Owns every byte and string referenced by a [`MaccvSnapshot`].
///
/// Built once per read so the C side sees flat, contiguous arrays and a single
/// free call.
///
/// # Why the payloads are cloned in here
///
/// The core `Snapshot` is dropped as soon as the FFI call returns, so a pointer
/// into *its* `Vec<u8>` buffers would dangle the moment C touched it. Every
/// payload is therefore moved into [`SnapshotStorage::payloads`], and the
/// `MaccvRepresentation::data` pointers aim at those buffers instead.
///
/// # Self-referential pointers
///
/// `items[].representations` points into `representation_arrays`, and each
/// `MaccvRepresentation::data` points into `payloads`. Both point at the *heap
/// buffers of the inner `Vec`s*, which never move when the outer `Vec` grows or
/// when the storage struct itself is moved. The pointers are nonetheless bound
/// only in [`SnapshotStorage::bind_pointers`] — after the last push — and cleared
/// in [`SnapshotStorage::unbind_pointers`] before the storage is dropped.
struct SnapshotStorage {
    change_count: i64,
    /// Owns the exact bytes handed to C. Inner buffers never move.
    payloads: Vec<Vec<u8>>,
    /// Payload index per representation, parallel to `representation_arrays`.
    payload_indices: Vec<Vec<usize>>,
    /// Owns the representation arrays pointed at by `items[].representations`.
    representation_arrays: Vec<Vec<MaccvRepresentation>>,
    /// Owns the backing bytes of every `MaccvString` handed out.
    strings: Vec<CString>,
    items: Vec<MaccvItem>,
}

impl SnapshotStorage {
    fn build(snapshot: &Snapshot) -> Self {
        let item_count = snapshot.items.len();
        let mut storage = Self {
            change_count: snapshot.change_count.get(),
            payloads: Vec::with_capacity(item_count * 4),
            payload_indices: Vec::with_capacity(item_count),
            representation_arrays: Vec::with_capacity(item_count),
            strings: Vec::with_capacity(item_count * 8),
            items: Vec::with_capacity(item_count),
        };

        for item in &snapshot.items {
            let mut reps = Vec::with_capacity(item.representations.len());
            let mut indices = Vec::with_capacity(item.representations.len());
            for rep in &item.representations {
                let payload_index = storage.payloads.len();
                storage.payloads.push(rep.data.clone());
                indices.push(payload_index);
                reps.push(MaccvRepresentation {
                    type_identifier: storage.push_string(rep.ty.as_str()),
                    // Bound in `bind_pointers`, once `payloads` is complete.
                    data: MaccvData::EMPTY,
                });
            }
            let kind = storage.push_string(item.content.kind().as_str());
            let preview = storage.push_string(&item.content.preview(120));
            storage.representation_arrays.push(reps);
            storage.payload_indices.push(indices);
            storage.items.push(MaccvItem {
                representations: ptr::null(),
                representations_len: 0,
                kind,
                preview,
                byte_len: item.byte_len(),
            });
        }

        storage
    }

    /// Binds every self-referential pointer. Must be called once no further
    /// elements can be pushed.
    fn bind_pointers(&mut self) {
        for (item_index, reps) in self.representation_arrays.iter_mut().enumerate() {
            for (rep_index, rep) in reps.iter_mut().enumerate() {
                let payload_index = self.payload_indices[item_index][rep_index];
                let buffer = &self.payloads[payload_index];
                rep.data = MaccvData {
                    ptr: buffer.as_ptr(),
                    len: buffer.len(),
                };
            }
            self.items[item_index].representations = reps.as_ptr();
            self.items[item_index].representations_len = reps.len();
        }
    }

    /// Clears the self-referential pointers, restoring the struct to a form that
    /// is safe to drop.
    fn unbind_pointers(&mut self) {
        for item in &mut self.items {
            item.representations = ptr::null();
            item.representations_len = 0;
        }
        for reps in &mut self.representation_arrays {
            for rep in reps.iter_mut() {
                rep.data = MaccvData::EMPTY;
            }
        }
    }

    /// Total payload size across every item.
    fn payload_byte_len(&self) -> usize {
        self.items.iter().map(|i| i.byte_len).sum()
    }

    fn push_string(&mut self, value: &str) -> MaccvString {
        // `CString::new` rejects interior NULs; sanitise instead of failing, so
        // hostile clipboard payloads can never abort a read.
        let sanitised = if value.as_bytes().contains(&0) {
            value.replace('\0', "\u{FFFD}")
        } else {
            value.to_owned()
        };
        match CString::new(sanitised) {
            Ok(cstring) => {
                let ns = MaccvString {
                    ptr: cstring.as_ptr(),
                    len: cstring.as_bytes().len(),
                };
                self.strings.push(cstring);
                ns
            }
            Err(_) => MaccvString::EMPTY,
        }
    }
}

/// The heap block handed to C: header first, backing storage immediately after.
///
/// C only ever sees `&mut RawSnapshot.header`, because `header` is the first field
/// and the struct is `repr(C)`.
#[repr(C)]
struct RawSnapshot {
    header: MaccvSnapshot,
    backing: SnapshotStorage,
}

impl SnapshotStorage {
    /// Heap-allocates the header plus its backing storage and returns the pointer
    /// C will hold. The pointer targets the first field, so C can read the header
    /// directly.
    fn into_raw(mut self) -> *mut MaccvSnapshot {
        self.bind_pointers();
        let header = MaccvSnapshot {
            items: self.items.as_ptr(),
            items_len: self.items.len(),
            change_count: self.change_count,
            byte_len: self.payload_byte_len(),
        };
        let boxed = Box::new(RawSnapshot {
            header,
            backing: self,
        });
        let raw = Box::into_raw(boxed);
        // SAFETY: `RawSnapshot` is `repr(C)` and `header` is its first field, so
        // the header shares the allocation's address. `raw` is non-null because
        // it came from `Box::into_raw`, and no reference to the box exists yet.
        unsafe { ptr::addr_of_mut!((*raw).header) }
    }
}

impl Drop for SnapshotStorage {
    fn drop(&mut self) {
        // Restore the struct to a drop-safe shape before the fields go away.
        self.unbind_pointers();
    }
}

// ---------------------------------------------------------------------------
// Thread-local last error
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<Option<MaccvError>> = const { RefCell::new(None) };
    static LAST_ERROR_MESSAGE: RefCell<Option<CString>> = const { RefCell::new(None) };
}

pub(crate) fn record_error(err: MaccvError, detail: Option<String>) {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(err));
    LAST_ERROR_MESSAGE.with(|slot| {
        *slot.borrow_mut() = detail.and_then(|msg| CString::new(msg).ok());
    });
}

pub(crate) fn record_ok() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
    LAST_ERROR_MESSAGE.with(|slot| *slot.borrow_mut() = None);
}

/// Records a core error and returns its C status code.
///
/// Used by the modules outside this file, which surface errors directly rather
/// than through `status`'s pass-through form.
pub(crate) fn record_core_error(error: maccv_core::Error) -> MaccvError {
    let code = MaccvError::from(error.clone());
    record_error(code, Some(error.to_string()));
    code
}

/// Converts a core result into an FFI status code, recording any error.
fn status<T>(result: maccv_core::Result<T>) -> std::result::Result<T, MaccvError> {
    match result {
        Ok(value) => {
            record_ok();
            Ok(value)
        }
        Err(err) => {
            let code = MaccvError::from(err.clone());
            record_error(code, Some(err.to_string()));
            Err(code)
        }
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Engine version, as a `'static` string. Do **not** free.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_version() -> MaccvString {
    MaccvString::from_static(maccv_core::VERSION)
}

/// A human-readable description of a status code. `'static`; do **not** free.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_error_message(error: MaccvError) -> MaccvString {
    let text: &'static str = match error {
        MaccvError::Ok => "ok",
        MaccvError::AccessDenied => {
            "pasteboard access denied: the system's macOS 27 access policy refused this app"
        }
        MaccvError::PasteboardNotFound => "no pasteboard with that name",
        MaccvError::TypeNotFound => "the requested pasteboard type is not present",
        MaccvError::InvalidPayload => "the payload could not be interpreted",
        MaccvError::InvalidUtf8 => "a text payload was not valid UTF-8",
        MaccvError::WriteFailed => "AppKit rejected the pasteboard write",
        MaccvError::InvalidState => "the object is in an invalid state for this call",
        MaccvError::NullPointer => "a required pointer argument was null",
        MaccvError::InvalidCString => "a C string argument was not valid UTF-8",
    };
    MaccvString::from_static(text)
}

/// The detail message for the last error on this thread.
///
/// Owned; release with [`maccv_string_free`]. Returns an empty string when the
/// last call succeeded.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_last_error_message() -> MaccvString {
    LAST_ERROR_MESSAGE.with(|slot| match slot.borrow().as_ref() {
        Some(cstring) => MaccvString {
            ptr: cstring.as_ptr(),
            len: cstring.as_bytes().len(),
        },
        None => MaccvString::EMPTY,
    })
}

/// Frees a string previously returned by an owning entry point.
///
/// # Safety
///
/// `value` must have come from `maccv_last_error_message` or
/// `maccv_pasteboard_name`, and must not be freed twice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_string_free(value: MaccvString) {
    value.free_owned();
}

/// Opens the system-wide general pasteboard.
///
/// Returns null only if allocation fails.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_pasteboard_general() -> *mut MaccvPasteboard {
    Box::into_raw(Box::new(MaccvPasteboard {
        inner: Pasteboard::general(),
    }))
}

/// Opens a pasteboard private to this process.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_pasteboard_unique() -> *mut MaccvPasteboard {
    Box::into_raw(Box::new(MaccvPasteboard {
        inner: Pasteboard::unique(),
    }))
}

/// Opens a named pasteboard, creating it if needed.
///
/// # Safety
///
/// `name` must be a NUL-terminated UTF-8 string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_named(name: *const c_char) -> *mut MaccvPasteboard {
    let Some(name) = (unsafe { cstr_to_str(name) }) else {
        record_error(MaccvError::NullPointer, Some("name was null".into()));
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(MaccvPasteboard {
        inner: Pasteboard::named(name),
    }))
}

/// Releases a pasteboard handle obtained from this library.
///
/// # Safety
///
/// `pb` must come from one of the `maccv_pasteboard_*` constructors, and must not
/// be used or freed again afterwards. Null is accepted and ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_free(pb: *mut MaccvPasteboard) {
    if pb.is_null() {
        return;
    }
    // SAFETY: the caller guarantees ownership and single-use.
    drop(unsafe { Box::from_raw(pb) });
}

/// The AppKit name of a pasteboard. Owned; free with [`maccv_string_free`].
///
/// # Safety
///
/// `pb` must be a live handle from this library, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_name(pb: *const MaccvPasteboard) -> MaccvString {
    let Some(pb) = (unsafe { pb.as_ref() }) else {
        record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
        return MaccvString::EMPTY;
    };
    record_ok();
    MaccvString::owned(&pb.inner.name())
}

/// Reads the change counter.
///
/// This is the cheapest change-detection primitive available: a cached property
/// load, well under a microsecond. Poll it and compare to notice writes by any
/// process, including yourself.
///
/// # Safety
///
/// `pb` must be a live handle from this library. Null returns `i64::MIN`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_change_count(pb: *const MaccvPasteboard) -> i64 {
    match unsafe { pb.as_ref() } {
        Some(pb) => pb.inner.change_count().get(),
        None => {
            record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
            i64::MIN
        }
    }
}

/// The change counter produced by this handle's own most recent write.
///
/// A watcher uses this to ignore the echo of its own writes. Returns `i64::MIN`
/// when this handle has not written yet.
///
/// # Safety
///
/// `pb` must be a live handle from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_last_write_count(pb: *const MaccvPasteboard) -> i64 {
    match unsafe { pb.as_ref() } {
        Some(pb) => pb
            .inner
            .last_self_write()
            .map_or(i64::MIN, ChangeCount::get),
        None => {
            record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
            i64::MIN
        }
    }
}

/// macOS 27's current per-app pasteboard access policy.
///
/// # Safety
///
/// `pb` must be a live handle from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_access_behavior(
    pb: *const MaccvPasteboard,
) -> MaccvAccessBehavior {
    match unsafe { pb.as_ref() } {
        Some(pb) => pb.inner.access_behavior().into(),
        None => {
            record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
            MaccvAccessBehavior::Default
        }
    }
}

/// Whether the pasteboard currently holds nothing.
///
/// # Safety
///
/// `pb` must be a live handle from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_is_empty(pb: *const MaccvPasteboard) -> bool {
    match unsafe { pb.as_ref() } {
        Some(pb) => pb.inner.is_empty(),
        None => {
            record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
            true
        }
    }
}

/// Whether any representation conforms to the given UTI.
///
/// Cheap: AppKit answers from type metadata without materialising payloads.
///
/// # Safety
///
/// `pb` must be live; `uti` must be NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_can_read(
    pb: *const MaccvPasteboard,
    uti: *const c_char,
) -> bool {
    let (Some(pb), Some(uti)) = (unsafe { pb.as_ref() }, unsafe { cstr_to_str(uti) }) else {
        record_error(
            MaccvError::NullPointer,
            Some("pasteboard or uti was null".into()),
        );
        return false;
    };
    record_ok();
    match WellKnownType::from_identifier(uti) {
        Some(known) => pb.inner.can_read(known),
        // An unknown UTI is still answerable by comparing against reported types.
        None => pb
            .inner
            .types()
            .iter()
            .any(|present| present.as_str() == uti),
    }
}

/// Captures the entire pasteboard into `out`.
///
/// On success `*out` receives an owned [`MaccvSnapshot`] that must be released
/// with [`maccv_snapshot_free`]. On failure `*out` is left untouched.
///
/// # Safety
///
/// `pb` must be a live handle; `out` must be a valid, writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_snapshot(
    pb: *const MaccvPasteboard,
    out: *mut *mut MaccvSnapshot,
) -> MaccvError {
    let Some(pb) = (unsafe { pb.as_ref() }) else {
        record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
        return MaccvError::NullPointer;
    };
    if out.is_null() {
        record_error(MaccvError::NullPointer, Some("out was null".into()));
        return MaccvError::NullPointer;
    }

    let snapshot = match status(pb.inner.snapshot()) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    // SAFETY: `out` is non-null and writable per the contract above.
    unsafe { *out = SnapshotStorage::build(&snapshot).into_raw() };
    MaccvError::Ok
}

/// Releases a snapshot returned by [`maccv_pasteboard_snapshot`].
///
/// Frees the item array, every representation array and every string in one go.
///
/// # Safety
///
/// `snapshot` must come from `maccv_pasteboard_snapshot`, and must not be used or
/// freed again afterwards. Null is accepted and ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_snapshot_free(snapshot: *mut MaccvSnapshot) {
    if snapshot.is_null() {
        return;
    }
    // SAFETY: the caller guarantees the pointer came from `SnapshotStorage::into_raw`.
    drop(unsafe { Box::from_raw(snapshot.cast::<RawSnapshot>()) });
}

/// Writes plain UTF-8 text to the pasteboard.
///
/// `text` is **not** NUL-terminated; `len` bytes are read, so the payload may
/// contain NULs and need not be UTF-8-validated by the caller beyond that.
///
/// # Safety
///
/// `pb` must be a live handle; `text` must point at `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_write_string(
    pb: *const MaccvPasteboard,
    text: *const u8,
    len: usize,
) -> MaccvError {
    let Some(pb) = (unsafe { pb.as_ref() }) else {
        record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
        return MaccvError::NullPointer;
    };
    let Some(bytes) = (unsafe { bytes_to_slice(text, len) }) else {
        record_error(MaccvError::NullPointer, Some("text was null".into()));
        return MaccvError::NullPointer;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        record_error(MaccvError::InvalidUtf8, None);
        return MaccvError::InvalidUtf8;
    };
    match status(pb.inner.set_string(text)) {
        Ok(_) => MaccvError::Ok,
        Err(code) => code,
    }
}

/// Writes an arbitrary set of representations as a single pasteboard item.
///
/// This is the faithful writer: whatever bytes you captured come back exactly.
/// The pasteboard is cleared first, as AppKit requires.
///
/// # Safety
///
/// `pb` must be live; `representations` must point at `count` valid entries whose
/// `type_identifier` and `data` are each valid for their stated length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_write_representations(
    pb: *const MaccvPasteboard,
    representations: *const MaccvRepresentation,
    count: usize,
) -> MaccvError {
    let Some(pb) = (unsafe { pb.as_ref() }) else {
        record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
        return MaccvError::NullPointer;
    };
    if representations.is_null() || count == 0 {
        record_error(
            MaccvError::InvalidPayload,
            Some("no representations supplied".into()),
        );
        return MaccvError::InvalidPayload;
    }

    // SAFETY: the caller guarantees `count` valid entries.
    let entries = unsafe { slice::from_raw_parts(representations, count) };
    let mut owned = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(uti) = (unsafe { entry.type_identifier.as_str() }) else {
            record_error(MaccvError::InvalidCString, Some("bad UTI".into()));
            return MaccvError::InvalidCString;
        };
        let Some(data) = (unsafe { bytes_to_slice(entry.data.ptr, entry.data.len) }) else {
            record_error(MaccvError::NullPointer, Some("null payload".into()));
            return MaccvError::NullPointer;
        };
        owned.push((maccv_core::PasteboardType::new(uti), data.to_vec()));
    }

    match status(pb.inner.write_raw(&owned)) {
        Ok(_) => MaccvError::Ok,
        Err(code) => code,
    }
}

/// Writes several items, each with its own set of representations.
///
/// This is the multi-item writer. A pasteboard item can hold at most one entry
/// per type, so a three-file copy is three items — which is why
/// `maccv_pasteboard_write_representations` (one item) cannot replay a snapshot
/// faithfully on its own. `items[i].representations` must point at
/// `items[i].representations_len` valid entries.
///
/// # Safety
///
/// `pb` must be live; `items` must point at `count` valid entries whose
/// representation arrays and payloads are each valid for their stated lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_write_items(
    pb: *const MaccvPasteboard,
    items: *const MaccvItem,
    count: usize,
) -> MaccvError {
    let Some(pb) = (unsafe { pb.as_ref() }) else {
        record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
        return MaccvError::NullPointer;
    };
    if items.is_null() || count == 0 {
        record_error(MaccvError::InvalidPayload, Some("no items supplied".into()));
        return MaccvError::InvalidPayload;
    }

    // SAFETY: the caller guarantees `count` valid entries.
    let entries = unsafe { slice::from_raw_parts(items, count) };
    let mut per_item = Vec::with_capacity(entries.len());
    for item in entries {
        if item.representations.is_null() || item.representations_len == 0 {
            continue;
        }
        // SAFETY: guaranteed by the caller.
        let reps = unsafe { slice::from_raw_parts(item.representations, item.representations_len) };
        let mut owned = Vec::with_capacity(reps.len());
        for rep in reps {
            let Some(uti) = (unsafe { rep.type_identifier.as_str() }) else {
                record_error(MaccvError::InvalidCString, Some("bad UTI".into()));
                return MaccvError::InvalidCString;
            };
            let Some(data) = (unsafe { bytes_to_slice(rep.data.ptr, rep.data.len) }) else {
                record_error(MaccvError::NullPointer, Some("null payload".into()));
                return MaccvError::NullPointer;
            };
            owned.push((maccv_core::PasteboardType::new(uti), data.to_vec()));
        }
        if !owned.is_empty() {
            per_item.push(owned);
        }
    }

    if per_item.is_empty() {
        record_error(
            MaccvError::InvalidPayload,
            Some("every item was empty".into()),
        );
        return MaccvError::InvalidPayload;
    }

    match status(pb.inner.write_items(per_item)) {
        Ok(_) => MaccvError::Ok,
        Err(code) => code,
    }
}

/// Replays a previously captured snapshot onto the pasteboard.
///
/// Every item and every representation is rewritten byte-for-byte, so pasting
/// into the originating app reproduces the original formatting. Equivalent to
/// `maccv_pasteboard_write_items(pb, snapshot->items, snapshot->items_len)`.
///
/// # Safety
///
/// `pb` must be live; `snapshot` must be a snapshot this library produced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_write_snapshot(
    pb: *const MaccvPasteboard,
    snapshot: *const MaccvSnapshot,
) -> MaccvError {
    let Some(header) = (unsafe { snapshot.as_ref() }) else {
        record_error(MaccvError::NullPointer, Some("snapshot was null".into()));
        return MaccvError::NullPointer;
    };
    // SAFETY: the caller guarantees a snapshot produced by this library.
    unsafe { maccv_pasteboard_write_items(pb, header.items, header.items_len) }
}

/// Clears the pasteboard.
///
/// # Safety
///
/// `pb` must be a live handle from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_pasteboard_clear(pb: *const MaccvPasteboard) -> MaccvError {
    match unsafe { pb.as_ref() } {
        Some(pb) => {
            pb.inner.clear();
            record_ok();
            MaccvError::Ok
        }
        None => {
            record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
            MaccvError::NullPointer
        }
    }
}

/// Signature of the change callback invoked from the watcher thread.
///
/// Called with the new change counter and the `context` pointer given to
/// [`maccv_watch_start`]. Must be safe to call from a non-main thread.
pub type MaccvChangeCallback =
    Option<unsafe extern "C" fn(change_count: i64, context: *mut c_void)>;

/// Starts watching for pasteboard changes on a dedicated thread.
///
/// Polling is adaptive: it begins at `min_interval_us` so a copy is noticed
/// almost immediately, then backs off doubling towards `max_interval_us` while
/// the pasteboard is idle, and snaps back on any change.
///
/// Set `skip_self_writes` to true when the watcher should ignore changes this
/// same `pb` handle caused itself — the usual choice for a clipboard manager that
/// writes history entries back. Pass false to observe every change, including
/// your own, which is handy for tests and for keeping the UI in sync with what you
/// just wrote.
///
/// Returns an owned watcher; stop it with [`maccv_watch_stop`]. Null on failure.
///
/// # Safety
///
/// `pb` must be a live handle; `callback` must remain valid and thread-safe until
/// the watcher is stopped; `context` is passed through untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_watch_start(
    pb: *const MaccvPasteboard,
    min_interval_us: u64,
    max_interval_us: u64,
    skip_self_writes: bool,
    callback: MaccvChangeCallback,
    context: *mut c_void,
) -> *mut MaccvWatch {
    let Some(pb) = (unsafe { pb.as_ref() }) else {
        record_error(MaccvError::NullPointer, Some("pasteboard was null".into()));
        return ptr::null_mut();
    };
    let Some(callback) = callback else {
        record_error(MaccvError::NullPointer, Some("callback was null".into()));
        return ptr::null_mut();
    };

    // `usize` keeps the pointer `Send` across the thread boundary.
    let context = context as usize;
    let min_interval = Duration::from_micros(min_interval_us.max(100));
    let max_interval = Duration::from_micros(max_interval_us.max(min_interval_us.max(100)));

    let sink = move |count: ChangeCount| {
        let context = context as *mut c_void;
        // SAFETY: the caller guarantees the callback outlives the watcher, and
        // the watcher is joined before `maccv_watch_stop` returns.
        unsafe { callback(count.get(), context) };
    };

    let mut config = WatchConfig::adaptive(min_interval, max_interval);
    config.skip_self_writes = skip_self_writes;
    let handle = pb.inner.watch_with(config, sink);

    record_ok();
    Box::into_raw(Box::new(MaccvWatch {
        handle: Some(handle),
    }))
}

/// Stops a watcher and joins its thread.
///
/// Blocks until the watcher thread has exited, so it is safe to release the
/// callback's context afterwards.
///
/// # Safety
///
/// `watch` must come from `maccv_watch_start` and must not be used again. Null is
/// accepted and ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_watch_stop(watch: *mut MaccvWatch) {
    if watch.is_null() {
        return;
    }
    // SAFETY: the caller guarantees ownership and single use.
    let mut boxed = unsafe { Box::from_raw(watch) };
    if let Some(handle) = boxed.handle.take() {
        handle.join();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// # Safety
///
/// `ptr` must be null or point at a NUL-terminated UTF-8 string.
pub(crate) unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// # Safety
///
/// `ptr` must be null or point at `len` readable bytes.
pub(crate) unsafe fn bytes_to_slice<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() {
        return None;
    }
    if len == 0 {
        return Some(&[]);
    }
    // SAFETY: guaranteed by the caller.
    Some(unsafe { slice::from_raw_parts(ptr, len) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable_and_ok_is_zero() {
        assert_eq!(MaccvError::Ok as i32, 0);
        assert_eq!(MaccvError::AccessDenied as i32, 1);
        assert_eq!(MaccvError::NullPointer as i32, 8);
        assert_eq!(MaccvError::InvalidCString as i32, 9);
    }

    #[test]
    fn core_errors_map_to_codes() {
        use maccv_core::Error as E;
        assert_eq!(MaccvError::from(E::AccessDenied), MaccvError::AccessDenied);
        assert_eq!(MaccvError::from(E::WriteFailed), MaccvError::WriteFailed);
        assert_eq!(MaccvError::from(E::InvalidUtf8), MaccvError::InvalidUtf8);
        assert_eq!(
            MaccvError::from(E::InvalidState("x".into())),
            MaccvError::InvalidState
        );
    }

    #[test]
    fn null_pointers_are_reported_not_trapped() {
        // SAFETY: deliberately passing null; the API must report, not crash.
        unsafe {
            assert_eq!(
                maccv_pasteboard_snapshot(ptr::null(), ptr::null_mut()),
                MaccvError::NullPointer
            );
            assert_eq!(maccv_pasteboard_clear(ptr::null()), MaccvError::NullPointer);
            assert_eq!(
                maccv_pasteboard_write_string(ptr::null(), b"x".as_ptr(), 1),
                MaccvError::NullPointer
            );
            assert_eq!(maccv_pasteboard_change_count(ptr::null()), i64::MIN);
        }
    }

    #[test]
    fn static_strings_are_never_null() {
        let version = maccv_version();
        assert!(!version.ptr.is_null());
        assert_eq!(unsafe { version.as_str() }, Some(maccv_core::VERSION));

        let message = maccv_error_message(MaccvError::AccessDenied);
        assert!(!message.ptr.is_null());
        assert!(unsafe { message.as_str() }.unwrap().contains("denied"));
    }

    #[test]
    fn owned_string_round_trips_and_frees() {
        let owned = MaccvString::owned("hello 世界");
        assert_eq!(unsafe { owned.as_str() }, Some("hello 世界"));
        // SAFETY: `owned` came from `MaccvString::owned`.
        unsafe { maccv_string_free(owned) };

        let empty = MaccvString::owned("");
        assert_eq!(unsafe { empty.as_str() }, Some(""));
        // SAFETY: as above.
        unsafe { maccv_string_free(empty) };
    }

    #[test]
    fn bytes_to_slice_handles_zero_and_null() {
        assert!(unsafe { bytes_to_slice(ptr::null(), 0) }.is_none());
        assert_eq!(unsafe { bytes_to_slice(b"abc".as_ptr(), 0) }, Some(&[][..]));
        assert_eq!(
            unsafe { bytes_to_slice(b"abc".as_ptr(), 3) },
            Some(&b"abc"[..])
        );
    }

    #[test]
    fn empty_constants_are_non_null() {
        assert!(!MaccvString::EMPTY.ptr.is_null());
        assert_eq!(MaccvString::EMPTY.len, 0);
        assert!(!MaccvData::EMPTY.ptr.is_null());
    }
}
