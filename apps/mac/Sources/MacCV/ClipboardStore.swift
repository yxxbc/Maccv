//
//  ClipboardStore.swift
//  MacCV
//
//  Observable state for the clipboard panel.
//
//  The store owns the Rust engine, watches it for changes, and keeps a bounded
//  in-memory history. Watching is delegated to the engine's own thread, which
//  polls with an adaptive interval: fast right after a copy, backing off to a
//  quarter-second when idle. No Swift timer runs, so an idle MacCV costs nothing.
//

import Foundation
import Observation

@MainActor
@Observable
final class ClipboardStore {
    /// The live clipboard, as of the last change.
    private(set) var current: ClipSnapshot?

    /// Most recent first. Bounded by `historyLimit`.
    private(set) var history: [ClipSnapshot] = []

    /// Set when the engine refuses access, so the UI can explain the situation
    /// rather than silently showing nothing.
    private(set) var accessIssue: ClipboardError?

    /// Whether recording is paused.
    var isPaused = false

    /// How many entries to keep in memory.
    var historyLimit = 200

    /// The engine, if it initialised. Not observable: it never changes.
    @ObservationIgnored private let engine: ClipboardEngine?

    /// Preferences that affect what is recorded and how it is displayed.
    var settings: AppSettings?

    /// Handle on the engine's watcher thread.
    ///
    /// Excluded from observation so it stays a plain stored property, which lets
    /// `deinit` reach it. `nonisolated(unsafe)` is accurate here: the only
    /// cross-thread access is `WatchToken.stop()`, which is idempotent and safe
    /// from any thread.
    @ObservationIgnored private nonisolated(unsafe) var watchToken: WatchToken?

    init() {
        engine = ClipboardEngine()
        readCurrent()
        startWatching()
    }

    deinit {
        // Join the watcher thread so it cannot outlive the engine it polls.
        watchToken?.stop()
    }

    // MARK: - Watching

    private func startWatching() {
        guard let engine, watchToken == nil else { return }

        watchToken = engine.startWatching { [weak self] in
            // Called on the engine's watcher thread.
            Task { @MainActor in self?.handleExternalChange() }
        }
    }

    private func handleExternalChange() {
        guard !isPaused else { return }
        readCurrent(recording: true)
    }

    /// Reads the clipboard into `current`, optionally appending to history.
    ///
    /// Two privacy rules are applied here rather than at display time, because
    /// both are about what the app is allowed to *keep*, not just show:
    ///
    /// - An entry another app marked concealed or transient is never recorded. That
    ///   marking is a statement of intent, unlike the masking heuristics, and it is
    ///   how password managers ask not to be logged.
    /// - When masking is on, the stored preview and payloads are redacted, so a
    ///   screenshot of the panel or a memory dump does not expose the value.
    func readCurrent(recording: Bool = false) {
        guard let engine else { return }
        do {
            var snapshot = try engine.snapshot()
            accessIssue = nil
            current = snapshot

            guard recording, !snapshot.isEmpty else { return }

            // Guard against recording the same change twice: the watcher can fire
            // for a change counter bump that carried identical bytes.
            if history.first?.changeCount == snapshot.changeCount { return }

            if settings?.skipConcealedEntries ?? true {
                // Drop any item the source app asked us not to keep.
                let kept = snapshot.items.filter { !Self.isConcealed($0) }
                if kept.count != snapshot.items.count {
                    // Everything was concealed: record nothing at all rather than an
                    // empty entry.
                    if kept.isEmpty { return }
                    snapshot = ClipSnapshot(
                        changeCount: snapshot.changeCount,
                        byteLength: snapshot.byteLength,
                        items: kept
                    )
                }
            }

            if let mode = settings?.effectiveRedactionMode {
                snapshot = Self.redacting(snapshot, mode: mode)
            }

            history.insert(snapshot, at: 0)
            if history.count > historyLimit {
                history.removeLast(history.count - historyLimit)
            }
        } catch let error as ClipboardError {
            accessIssue = error
        } catch {
            accessIssue = .invalidState
        }
    }

    /// Re-reads the clipboard and re-applies the current privacy settings.
    ///
    /// Called when a setting changes, so the effect is visible immediately rather
    /// than only on the next copy.
    func reload() {
        // Rebuild history under the new rules: entries recorded while masking was
        // off would otherwise keep their unmasked previews.
        let snapshots = history
        history.removeAll()
        if let mode = settings?.effectiveRedactionMode {
            history = snapshots.map { Self.redacting($0, mode: mode) }
        } else {
            history = snapshots
        }
        readCurrent()
    }

    /// The engine's current Accessibility permission state.
    func accessibilityPermission() -> AccessibilityPermission {
        AccessibilityPermission(rawValue: maccv_secure_input_permission().rawValue) ?? .notGranted
    }

    /// Whether `item` carries a marker that means "do not record this".
    ///
    /// The engine reports this on the redaction result; checking the entry's own
    /// type list first avoids running every detector just to ask the question.
    private static func isConcealed(_ item: ClipItem) -> Bool {
        Self.concealedMarkers.contains { marker in
            item.representations.contains { $0.typeIdentifier == marker }
        }
    }

    /// UTIs that third-party apps use to mark a pasteboard entry as not-to-keep.
    ///
    /// These are conventions rather than system API — macOS exposes no such flag —
    /// but they are what password managers actually set, and honouring them costs
    /// nothing.
    private static let concealedMarkers = [
        "org.nspasteboard.ConcealedType",
        "org.nspasteboard.TransientType",
        "com.agilebits.onepassword",
        "de.petermaurer.TransientPasteboardType",
    ]

    /// Applies masking to an item's preview and textual payloads.
    private static func redacting(_ snapshot: ClipSnapshot, mode: RedactionMode) -> ClipSnapshot {
        let items = snapshot.items.map { item -> ClipItem in
            // Only text is worth scanning: a card number cannot appear in a PNG.
            guard item.kind == .text || item.kind == .richText || item.kind == .url else {
                return item
            }

            var representations = item.representations
            var preview = item.preview
            var maskedKinds: [SensitiveKind] = []

            for index in representations.indices {
                guard let text = representations[index].text else { continue }
                let result = RedactionBridge.redact(text, mode: mode)
                guard result.wasRedacted else { continue }
                if let masked = result.text.data(using: .utf8) {
                    representations[index] = ClipRepresentation(
                        typeIdentifier: representations[index].typeIdentifier,
                        data: masked
                    )
                }
                maskedKinds.append(contentsOf: result.kinds)
            }

            if !maskedKinds.isEmpty {
                let result = RedactionBridge.redact(preview, mode: mode)
                preview = result.text
            }

            return ClipItem(
                kind: item.kind,
                preview: preview,
                byteLength: item.byteLength,
                representations: representations,
                maskedKinds: maskedKinds
            )
        }

        return ClipSnapshot(
            changeCount: snapshot.changeCount,
            byteLength: snapshot.byteLength,
            items: items
        )
    }

    // MARK: - Actions

    /// Puts a history entry back on the clipboard, preserving rich formats.
    func restore(_ snapshot: ClipSnapshot) {
        guard let engine else { return }
        do {
            try engine.write(snapshot)
            accessIssue = nil
            // Reflect the write immediately rather than waiting for the watcher,
            // which deliberately ignores our own writes.
            current = snapshot
        } catch let error as ClipboardError {
            accessIssue = error
        } catch {
            accessIssue = .writeFailed
        }
    }

    /// Puts a single item's representations back on the clipboard.
    func restore(_ item: ClipItem) {
        guard let engine else { return }
        do {
            try engine.write(item)
            accessIssue = nil
        } catch let error as ClipboardError {
            accessIssue = error
        } catch {
            accessIssue = .writeFailed
        }
    }

    /// Copies an entry as plain text, discarding formatting.
    func restoreAsPlainText(_ snapshot: ClipSnapshot) {
        guard let engine, let text = snapshot.items.first?.plainText else { return }
        do {
            try engine.writeText(text)
            accessIssue = nil
        } catch let error as ClipboardError {
            accessIssue = error
        } catch {
            accessIssue = .writeFailed
        }
    }

    /// Delivers text to the frontmost app without putting it on the clipboard.
    ///
    /// This is the mechanism that actually keeps other applications from reading
    /// what you paste: the content is typed in as synthetic keystrokes and never
    /// reaches a shared channel. It needs the Accessibility permission and is only
    /// meaningful for text, so the UI offers it as a choice rather than a default.
    func pasteWithoutClipboard(_ text: String) {
        let bytes = Array(text.utf8)
        let status = bytes.withUnsafeBufferPointer { buffer in
            maccv_secure_input_type(buffer.baseAddress, UInt(buffer.count), 2_000)
        }
        if status == MACCV_ERROR_OK {
            accessIssue = nil
        } else {
            accessIssue = ClipboardError(code: status)
        }
    }

    /// Empties the clipboard.
    func clearClipboard() {
        guard let engine else { return }
        do {
            try engine.clear()
            accessIssue = nil
            readCurrent()
        } catch let error as ClipboardError {
            accessIssue = error
        } catch {
            accessIssue = .writeFailed
        }
    }

    /// Removes one entry from history.
    func forget(_ snapshot: ClipSnapshot) {
        history.removeAll { $0.id == snapshot.id }
    }

    /// Removes every history entry.
    func forgetAll() {
        history.removeAll()
    }

    /// Current macOS 27 access policy, for display in the UI.
    var accessBehavior: PasteboardAccess {
        engine?.accessBehavior ?? .default
    }

    /// The engine version, shown in the About row.
    var engineVersion: String {
        ClipboardEngine.engineVersion
    }
}
