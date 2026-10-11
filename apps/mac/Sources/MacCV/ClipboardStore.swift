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
    func readCurrent(recording: Bool = false) {
        guard let engine else { return }
        do {
            let snapshot = try engine.snapshot()
            accessIssue = nil
            current = snapshot

            guard recording, !snapshot.isEmpty else { return }

            // Guard against recording the same change twice: the watcher can fire
            // for a change counter bump that carried identical bytes.
            if history.first?.changeCount == snapshot.changeCount { return }

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
