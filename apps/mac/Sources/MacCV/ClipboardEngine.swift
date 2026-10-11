//
//  ClipboardEngine.swift
//  MacCV
//
//  A Swift-facing wrapper over the Rust engine's C ABI.
//
//  The C API returns owned pointers and length-tagged buffers, which is the right
//  shape for a stable ABI but an awkward one to use directly. This file is the
//  single place that converts that surface into Swift value types and manages the
//  free calls, so no other Swift file needs to think about manual memory.
//
//  Nothing here re-parses clipboard payloads: bytes travel from the Rust core to
//  Swift as `Data` without an intermediate decode.
//

import Foundation

// MARK: - Value types

/// What kind of thing the user copied.
enum ClipKind: String, Sendable, CaseIterable {
    case text
    case richText = "rich-text"
    case image
    case files
    case url
    case color
    case sound
    case other

    /// SF Symbol used in the list rows.
    var symbolName: String {
        switch self {
        case .text: "text.alignleft"
        case .richText: "textformat"
        case .image: "photo"
        case .files: "folder"
        case .url: "link"
        case .color: "paintpalette"
        case .sound: "waveform"
        case .other: "questionmark.square.dashed"
        }
    }

    /// Human-readable label.
    var displayName: String {
        switch self {
        case .text: "Text"
        case .richText: "Rich Text"
        case .image: "Image"
        case .files: "Files"
        case .url: "Link"
        case .color: "Color"
        case .sound: "Sound"
        case .other: "Data"
        }
    }

    /// Whether the payload is typically large enough to matter for caching.
    var isBinary: Bool {
        switch self {
        case .image, .sound, .files: true
        default: false
        }
    }
}

/// One `(UTI, bytes)` pair exactly as the pasteboard stored it.
struct ClipRepresentation: Sendable {
    let typeIdentifier: String
    let data: Data

    /// The payload decoded as UTF-8, when it is textual.
    var text: String? {
        String(data: data, encoding: .utf8)
    }
}

/// One logical thing the user copied, with every representation it carried.
struct ClipItem: Identifiable, Sendable {
    let id = UUID()
    let kind: ClipKind
    let preview: String
    let byteLength: Int
    let representations: [ClipRepresentation]

    /// The payload for `uti`, if present.
    func representation(_ uti: String) -> ClipRepresentation? {
        representations.first { $0.typeIdentifier == uti }
    }

    var plainText: String? {
        representation(PasteboardUTI.string)?.text
    }

    var pngData: Data? {
        representation(PasteboardUTI.png)?.data
    }

    var filePaths: [String] {
        representations
            .filter { $0.typeIdentifier == PasteboardUTI.fileURL }
            .compactMap { $0.text }
            .compactMap { URL(string: $0)?.path }
    }
}

/// A point-in-time capture of the whole pasteboard.
struct ClipSnapshot: Identifiable, Sendable {
    let id = UUID()
    let changeCount: Int64
    let byteLength: Int
    let items: [ClipItem]

    var isEmpty: Bool { items.isEmpty }

    /// A one-line summary of the first item.
    var preview: String { items.first?.preview ?? "" }

    var kind: ClipKind { items.first?.kind ?? .other }
}

/// Well-known pasteboard UTIs, so Swift code never hard-codes a string twice.
enum PasteboardUTI {
    static let string = "public.utf8-plain-text"
    static let html = "public.html"
    static let rtf = "public.rtf"
    static let rtfd = "com.apple.flat-rtfd"
    static let png = "public.png"
    static let tiff = "public.tiff"
    static let pdf = "com.adobe.pdf"
    static let url = "public.url"
    static let fileURL = "public.file-url"
    static let color = "com.apple.cocoa.pasteboard.color"
    static let sound = "com.apple.cocoa.pasteboard.sound"
}

/// macOS 27's per-app pasteboard access policy.
enum PasteboardAccess: Int32, Sendable {
    case `default` = 0
    case ask = 1
    case alwaysAllow = 2
    case alwaysDeny = 3

    var isDenied: Bool { self == .alwaysDeny }
}

/// Errors surfaced by the engine, mirrored from `MaccvError`.
enum ClipboardError: Error, LocalizedError, Equatable {
    case accessDenied
    case pasteboardNotFound
    case typeNotFound
    case invalidPayload
    case invalidUTF8
    case writeFailed
    case invalidState
    case nullPointer
    case invalidCString
    case unknown(Int32)

    init(code: MaccvError) {
        // `MaccvError` is a C enum, so Swift types it as UInt32 with constants.
        switch code {
        case MACCV_ERROR_OK: self = .invalidState
        case MACCV_ERROR_ACCESS_DENIED: self = .accessDenied
        case MACCV_ERROR_PASTEBOARD_NOT_FOUND: self = .pasteboardNotFound
        case MACCV_ERROR_TYPE_NOT_FOUND: self = .typeNotFound
        case MACCV_ERROR_INVALID_PAYLOAD: self = .invalidPayload
        case MACCV_ERROR_INVALID_UTF8: self = .invalidUTF8
        case MACCV_ERROR_WRITE_FAILED: self = .writeFailed
        case MACCV_ERROR_INVALID_STATE: self = .invalidState
        case MACCV_ERROR_NULL_POINTER: self = .nullPointer
        case MACCV_ERROR_INVALID_C_STRING: self = .invalidCString
        default: self = .unknown(Int32(bitPattern: code.rawValue))
        }
    }

    var errorDescription: String? {
        switch self {
        case .accessDenied:
            "MacCV is not allowed to read the clipboard. Allow it in System Settings › Privacy & Security."
        case .pasteboardNotFound: "That pasteboard does not exist."
        case .typeNotFound: "The clipboard does not contain that type."
        case .invalidPayload: "The clipboard contents could not be read."
        case .invalidUTF8: "The clipboard text was not valid UTF-8."
        case .writeFailed: "The pasteboard refused the write."
        case .invalidState: "MacCV tried that in an invalid state."
        case .nullPointer: "Internal error: a null pointer reached the engine."
        case .invalidCString: "Internal error: an argument was not valid UTF-8."
        case .unknown(let code): "Unknown engine error (code \(code))."
        }
    }
}

// MARK: - Engine

/// Owns the Rust pasteboard handle and converts its C ABI into Swift values.
///
/// A single instance drives the whole app. The handle is cheap to hold: it wraps
/// one retained `NSPasteboard`.
final class ClipboardEngine: @unchecked Sendable {
    /// The opaque `NSPasteboard` handle, owned for this engine's lifetime.
    ///
    /// cbindgen emits `MaccvPasteboard` as an incomplete `struct`, which Swift
    /// imports as `OpaquePointer` — the right shape for a handle C never lets us
    /// introspect.
    let handlePointer: OpaquePointer
    private let lock = NSLock()

    /// The version reported by the linked Rust crate.
    static var engineVersion: String {
        stringToSwift(maccv_version())
    }

    /// Opens the system-wide general pasteboard.
    ///
    /// Reading this one may trigger macOS 27's access prompt, and the answer can be
    /// remembered as "never allow".
    convenience init?() {
        self.init(open: { maccv_pasteboard_general() })
    }

    /// Opens a pasteboard private to this process.
    ///
    /// Useful for tests: nothing is read from or written to the user's real
    /// clipboard, so a test run can never disturb it.
    static func privateToProcess() -> ClipboardEngine? {
        ClipboardEngine(open: { maccv_pasteboard_unique() })
    }

    private init?(open: () -> OpaquePointer?) {
        guard let handle = open() else { return nil }
        handlePointer = handle
    }

    deinit {
        maccv_pasteboard_free(handlePointer)
    }

    /// macOS 27's current access policy for the general pasteboard.
    var accessBehavior: PasteboardAccess {
        lock.withLock {
            PasteboardAccess(rawValue: Int32(maccv_pasteboard_access_behavior(handlePointer).rawValue))
                ?? .default
        }
    }

    /// The pasteboard's change counter. Sub-microsecond; safe to poll.
    var changeCount: Int64 {
        lock.withLock { maccv_pasteboard_change_count(handlePointer) }
    }

    var isEmpty: Bool {
        lock.withLock { maccv_pasteboard_is_empty(handlePointer) }
    }

    /// Whether the clipboard currently offers `uti`.
    func canRead(_ uti: String) -> Bool {
        lock.withLock {
            uti.withCString { maccv_pasteboard_can_read(handlePointer, $0) }
        }
    }

    /// Captures the entire clipboard.
    func snapshot() throws -> ClipSnapshot {
        try lock.withLock {
            var out: UnsafeMutablePointer<MaccvSnapshot>?
            let status = maccv_pasteboard_snapshot(handlePointer, &out)
            guard status == MACCV_ERROR_OK, let out else {
                throw error(for: status)
            }
            // The snapshot owns every buffer it hands back; release it exactly
            // once, after all Swift values have been built from it.
            defer { maccv_snapshot_free(out) }
            return Self.convert(out.pointee)
        }
    }

    /// Writes plain UTF-8 text.
    func writeText(_ text: String) throws {
        let bytes = Array(text.utf8)
        try lock.withLock {
            let status: MaccvError = bytes.withUnsafeBufferPointer { buffer in
                maccv_pasteboard_write_string(handlePointer, buffer.baseAddress, UInt(buffer.count))
            }
            guard status == MACCV_ERROR_OK else { throw error(for: status) }
        }
    }

    /// Replays a captured snapshot byte-for-byte, preserving rich formats.
    func write(_ snapshot: ClipSnapshot) throws {
        try write(snapshot.items)
    }

    /// Replays every representation of each item, in order.
    ///
    /// This is the faithful writer: a rich-text paragraph keeps its RTF and HTML,
    /// an image keeps its original PNG bytes, and a multi-file copy stays
    /// multi-file.
    func write(_ items: [ClipItem]) throws {
        guard !items.isEmpty else { throw ClipboardError.invalidPayload }
        // The storage must outlive the engine call: it owns every pointer the
        // engine dereferences.
        let storage = CRepresentationStorage(items: items)
        defer { storage.release() }

        guard let itemBlock = storage.items else { throw ClipboardError.invalidPayload }
        try lock.withLock {
            let status = maccv_pasteboard_write_items(handlePointer, itemBlock, UInt(storage.count))
            guard status == MACCV_ERROR_OK else { throw error(for: status) }
        }
    }

    /// Writes a single item's representations.
    func write(_ item: ClipItem) throws {
        try write([item])
    }

    /// Clears the clipboard.
    func clear() throws {
        try lock.withLock {
            let status = maccv_pasteboard_clear(handlePointer)
            guard status == MACCV_ERROR_OK else { throw error(for: status) }
        }
    }
}

// MARK: - Watching

/// A running change watcher.
///
/// Wraps the engine's watcher thread and marshals its C callback onto the main
/// actor. Releasing the token stops and joins the thread, so holding it as a
/// property is enough to keep the watcher alive and to tear it down.
final class WatchToken: @unchecked Sendable {
    /// Keeps the C callback's captured closure alive and addressable.
    ///
    /// Not private: the engine's `startWatching` extension constructs it, and the
    /// trampoline below casts the `void *` context back to this type.
    final class Context {
        let handler: @Sendable () -> Void
        init(handler: @escaping @Sendable () -> Void) { self.handler = handler }
    }

    /// The C watcher handle. `MaccvWatch` is an incomplete struct in C, so
    /// Swift models it as `OpaquePointer`.
    private var handle: OpaquePointer?
    private let context: Context

    fileprivate init(handle: OpaquePointer?, context: Context) {
        self.handle = handle
        self.context = context
    }

    /// Stops the watcher and joins its thread. Blocks until the thread exits, so
    /// it is safe to release anything the callback captured afterwards.
    func stop() {
        guard let handle else { return }
        maccv_watch_stop(handle)
        self.handle = nil
    }

    deinit { stop() }
}

extension ClipboardEngine {
    /// Starts a watcher that delivers changes on the main actor.
    ///
    /// `skipSelfWrites` should stay `true` for a clipboard manager: otherwise
    /// replaying a history entry is immediately re-recorded as a new entry.
    func startWatching(
        minIntervalMicroseconds: UInt64 = 400,
        maxIntervalMicroseconds: UInt64 = 250_000,
        skipSelfWrites: Bool = true,
        onChange: @escaping @Sendable () -> Void
    ) -> WatchToken? {
        let context = WatchToken.Context(handler: onChange)
        let opaque = Unmanaged.passRetained(context).toOpaque()

        let handle = maccv_watch_start(
            handlePointer,
            minIntervalMicroseconds,
            maxIntervalMicroseconds,
            skipSelfWrites,
            { _, userContext in
                guard let userContext else { return }
                // Balanced by the `release()` in `stop()`; the context is kept
                // alive by the +1 retain taken above.
                Unmanaged<WatchToken.Context>
                    .fromOpaque(userContext)
                    .takeUnretainedValue()
                    .handler()
            },
            opaque
        )

        guard let handle else {
            Unmanaged<WatchToken.Context>.fromOpaque(opaque).release()
            return nil
        }
        return WatchToken(handle: handle, context: context)
    }
}

// MARK: - Transformation

extension ClipboardEngine {
    /// Converts the C snapshot into Swift value types.
    ///
    /// Every byte is copied out of the snapshot here, before the caller frees it,
    /// so no Swift value ever points into engine-owned memory.
    private static func convert(_ snapshot: MaccvSnapshot) -> ClipSnapshot {
        var items: [ClipItem] = []
        items.reserveCapacity(Int(snapshot.items_len))

        for index in 0..<Int(snapshot.items_len) {
            let raw = snapshot.items[index]
            var reps: [ClipRepresentation] = []
            reps.reserveCapacity(Int(raw.representations_len))

            for repIndex in 0..<Int(raw.representations_len) {
                let rep = raw.representations![repIndex]
                let uti = stringToSwift(rep.type_identifier)
                let data: Data
                if rep.data.len == 0 || rep.data.ptr == nil {
                    data = Data()
                } else {
                    data = Data(bytes: rep.data.ptr, count: Int(rep.data.len))
                }
                reps.append(ClipRepresentation(typeIdentifier: uti, data: data))
            }

            items.append(
                ClipItem(
                    kind: ClipKind(rawValue: stringToSwift(raw.kind)) ?? .other,
                    preview: stringToSwift(raw.preview),
                    byteLength: Int(raw.byte_len),
                    representations: reps
                )
            )
        }

        return ClipSnapshot(
            changeCount: snapshot.change_count,
            byteLength: Int(snapshot.byte_len),
            items: items
        )
    }

    /// Maps a C status code to a Swift error.
    private func error(for code: MaccvError) -> ClipboardError {
        ClipboardError(code: code)
    }
}

// MARK: - C interop helpers

/// Converts a length-tagged `MaccvString` into a Swift `String`.
///
/// `MaccvString` is not NUL-safe, so the explicit length is used rather than
/// `strlen`. Static strings returned by the engine are not freed here; owned ones
/// are released by their own call sites.
func stringToSwift(_ value: MaccvString) -> String {
    guard let pointer = value.ptr, value.len > 0 else { return "" }
    let buffer = UnsafeRawBufferPointer(start: pointer, count: Int(value.len))
    return String(decoding: buffer, as: UTF8.self)
}

/// Owns the C-side buffers and strings for a set of items, and frees them together.
///
/// The engine borrows pointers for the duration of a write call, so every buffer
/// it dereferences must outlive that call *and* stay at a fixed address. That
/// rules out pointing at a Swift `Data`'s transient storage: `withUnsafeBytes`
/// only lends a pointer inside its closure.
///
/// This class is the backing store that fixes both problems. Each payload is
/// copied into a manually allocated `UnsafeMutableRawPointer` held for the store's
/// lifetime, and the `MaccvItem` / `MaccvRepresentation` arrays are allocated
/// directly rather than being Swift arrays that might move.
final class CRepresentationStorage {
    /// One allocation per payload handed to the engine.
    private var payloads: [UnsafeMutableRawPointer] = []
    /// One allocation per item's representation array.
    private var representationBlocks: [UnsafeMutablePointer<MaccvRepresentation>] = []
    /// One allocation per item's UTI string.
    private var utiStrings: [UnsafeMutablePointer<CChar>] = []
    /// The item array itself, allocated for the same reason.
    private var itemBlock: UnsafeMutablePointer<MaccvItem>?
    private var itemCount = 0

    private var released = false

    /// The item array to pass to `maccv_pasteboard_write_items`.
    var items: UnsafeMutablePointer<MaccvItem>? { itemBlock }
    /// How many items `items` holds.
    var count: Int { itemCount }

    init(items: [ClipItem]) {
        itemCount = items.count
        guard !items.isEmpty else { return }

        let itemBlock = UnsafeMutablePointer<MaccvItem>.allocate(capacity: items.count)
        itemBlock.initialize(repeating: MaccvItem(
            representations: nil,
            representations_len: 0,
            kind: MaccvString(ptr: nil, len: 0),
            preview: MaccvString(ptr: nil, len: 0),
            byte_len: 0
        ), count: items.count)
        self.itemBlock = itemBlock

        for (itemIndex, item) in items.enumerated() {
            let reps = item.representations

            let repBlock = UnsafeMutablePointer<MaccvRepresentation>.allocate(
                capacity: max(reps.count, 1)
            )
            representationBlocks.append(repBlock)

            for (repIndex, rep) in reps.enumerated() {
                // Copy the UTI into C-owned memory.
                let uti = strdup(rep.typeIdentifier)
                if let uti { utiStrings.append(uti) }

                // Copy the payload into a stable, C-owned buffer.
                let byteCount = rep.data.count
                let payload = UnsafeMutableRawPointer.allocate(
                    byteCount: max(byteCount, 1),
                    alignment: 1
                )
                if byteCount > 0 {
                    rep.data.copyBytes(to: payload.assumingMemoryBound(to: UInt8.self), count: byteCount)
                }
                payloads.append(payload)

                repBlock[repIndex] = MaccvRepresentation(
                    type_identifier: MaccvString(
                        ptr: UnsafePointer(uti),
                        len: UInt(rep.typeIdentifier.utf8.count)
                    ),
                    data: MaccvData(
                        ptr: payload.assumingMemoryBound(to: UInt8.self),
                        len: UInt(byteCount)
                    )
                )
            }

            // Bind the item's pointer last: `repBlock` is now fully populated and
            // its address will not change.
            itemBlock[itemIndex] = MaccvItem(
                representations: UnsafePointer(repBlock),
                representations_len: UInt(reps.count),
                kind: MaccvString(ptr: nil, len: 0),
                preview: MaccvString(ptr: nil, len: 0),
                byte_len: UInt(item.byteLength)
            )
        }
    }

    /// Frees every allocation this store made. Idempotent.
    func release() {
        guard !released else { return }
        released = true

        for pointer in payloads { pointer.deallocate() }
        payloads.removeAll()
        for pointer in representationBlocks { pointer.deallocate() }
        representationBlocks.removeAll()
        for pointer in utiStrings { free(pointer) }
        utiStrings.removeAll()
        itemBlock?.deallocate()
        itemBlock = nil
        itemCount = 0
    }

    deinit { release() }
}

private extension NSLock {
    /// Runs `body` while holding the lock.
    func withLock<T>(_ body: () throws -> T) rethrows -> T {
        lock()
        defer { unlock() }
        return try body()
    }
}
