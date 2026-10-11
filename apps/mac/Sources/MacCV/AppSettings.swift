//
//  AppSettings.swift
//  MacCV
//
//  Persisted preferences.
//
//  A single `@Observable` object rather than `@AppStorage` scattered through the
//  views: several settings interact (masking implies a redaction mode, secure
//  paste implies a permission check), and keeping the rules in one place makes
//  those relationships testable instead of emergent.
//

import Foundation
import Observation
import SwiftUI

// MARK: - Swift-side mirrors of the engine's enums
//
// The Rust enums are exposed through C as `MaccvRedactionMode` and friends. The
// app deliberately does not use those types directly: they are `Int32`-backed
// without `Codable`, so they cannot be persisted or pattern-matched idiomatically.
// These mirrors are the app's vocabulary, and the `cValue` bridges in
// `UpdateModel.swift` convert at the single point where the engine is called.

/// How much of a sensitive value to leave visible.
enum RedactionMode: String, Codable, Sendable, CaseIterable {
    /// Mask the entire value. The safer default.
    case full
    /// Keep a recognisable head and tail.
    case partial
}

/// How much of the update process to perform without asking.
enum UpdatePolicy: String, Codable, Sendable, CaseIterable {
    /// Never check unless the user asks.
    case manual
    /// Check in the background and show a badge; never download silently.
    case notifyOnly
    /// Check and download in the background; the user still confirms installing.
    case downloadInBackground
}

/// Which releases to consider.
enum UpdateChannel: String, Codable, Sendable, CaseIterable {
    /// Stable releases only.
    case stable
    /// Include pre-releases.
    case prerelease
}

/// What double-clicking a history entry does.
///
/// Configurable because the right answer depends on how someone works: copying back
/// to the clipboard is safe everywhere, while pasting into the frontmost app is what
/// people who live in the panel want — and it needs Accessibility access.
///
/// A single click always copies. That is the affordance that has to be discoverable
/// for history to be usable at all, so it is not configurable.
enum RowDoubleClickAction: String, Codable, Sendable, CaseIterable {
    /// Put the entry back on the clipboard.
    case copy
    /// Paste it into the frontmost app using ⌘V.
    case paste
    /// Do nothing on a double-click; the copy button and menu still work.
    case nothing

    /// Label for the settings picker.
    var label: String {
        switch self {
        case .copy: String(localized: "row.action.copy")
        case .paste: String(localized: "row.action.paste")
        case .nothing: String(localized: "row.action.nothing")
        }
    }
}

@MainActor
@Observable
final class AppSettings {
    /// How many history entries to keep. In-memory only; see `ClipboardStore`.
    var historyLimit: Int = 200

    /// Whether to mask detected sensitive values in the list.
    var maskSensitiveContent: Bool = true

    /// How much of a masked value to leave visible.
    var redactionMode: RedactionMode = .full

    /// A `Bool` view of the redaction mode, for a toggle.
    ///
    /// The underlying model is an enum so more modes can be added without
    /// disturbing the stored format; the settings UI only needs to distinguish
    /// "mask everything" from "keep a hint".
    var partialRedaction: Bool {
        get { redactionMode == .partial }
        set { redactionMode = newValue ? .partial : .full }
    }

    /// Whether entries another app marked concealed are skipped entirely.
    ///
    /// On by default because it is the only signal that represents intent rather
    /// than a guess — password managers set it precisely so their contents are not
    /// retained.
    var skipConcealedEntries: Bool = true

    /// Whether pasting should bypass the clipboard using synthetic input.
    ///
    /// Off by default: it needs the Accessibility permission and is slower, so it
    /// should be a deliberate choice rather than a surprise.
    var securePaste: Bool = false

    /// How aggressively to check for updates.
    var updatePolicy: UpdatePolicy = .notifyOnly

    /// Whether to consider pre-releases.
    var includePrereleases: Bool = false

    /// What a double-click on a history entry does.
    var rowDoubleClickAction: RowDoubleClickAction = .copy

    /// Overrides the interface language. `nil` follows the system.
    var languageOverride: String?

    // MARK: - Derived

    /// The redaction mode to actually apply, honouring the master switch.
    var effectiveRedactionMode: RedactionMode? {
        maskSensitiveContent ? redactionMode : nil
    }

    /// The update channel implied by the pre-release preference.
    var updateChannel: UpdateChannel {
        includePrereleases ? .prerelease : .stable
    }

    // MARK: - Persistence

    private static let storageKey = "MacCV.AppSettings.v1"

    /// Loads from `UserDefaults`, falling back to defaults for anything absent.
    ///
    /// Stores one JSON blob rather than a key per field so a future rename cannot
    /// leave a half-read configuration behind.
    static func load() -> AppSettings {
        let settings = AppSettings()
        guard
            let data = UserDefaults.standard.data(forKey: storageKey),
            let stored = try? JSONDecoder().decode(Stored.self, from: data)
        else {
            return settings
        }
        settings.apply(stored)
        return settings
    }

    /// Writes to `UserDefaults`.
    func save() {
        let stored = Stored(
            historyLimit: historyLimit,
            maskSensitiveContent: maskSensitiveContent,
            partialRedaction: redactionMode == .partial,
            skipConcealedEntries: skipConcealedEntries,
            securePaste: securePaste,
            updatePolicy: updatePolicy,
            includePrereleases: includePrereleases,
            rowDoubleClickAction: rowDoubleClickAction,
            languageOverride: languageOverride
        )
        guard let data = try? JSONEncoder().encode(stored) else { return }
        UserDefaults.standard.set(data, forKey: Self.storageKey)
    }

    private func apply(_ stored: Stored) {
        historyLimit = max(10, min(5_000, stored.historyLimit))
        maskSensitiveContent = stored.maskSensitiveContent
        redactionMode = stored.partialRedaction ? .partial : .full
        skipConcealedEntries = stored.skipConcealedEntries
        securePaste = stored.securePaste
        updatePolicy = stored.updatePolicy
        includePrereleases = stored.includePrereleases
        rowDoubleClickAction = stored.rowDoubleClickAction
        languageOverride = stored.languageOverride
    }

    /// The on-disk shape. `Bool` for the redaction mode so an unknown string in a
    /// future version cannot fail the whole decode.
    private struct Stored: Codable {
        var historyLimit: Int
        var maskSensitiveContent: Bool
        var partialRedaction: Bool
        var skipConcealedEntries: Bool
        var securePaste: Bool
        var updatePolicy: UpdatePolicy
        var includePrereleases: Bool
        /// Defaulted rather than required, so a settings blob written by an earlier
        /// version still decodes instead of being discarded wholesale.
        var rowDoubleClickAction: RowDoubleClickAction = .copy
        var languageOverride: String?
    }
}

// MARK: - Language

extension AppSettings {
    /// The languages the interface is translated into.
    ///
    /// Must match the `.xcstrings` catalog's localizations; a code listed here
    /// with no translations would show an empty UI.
    static let supportedLanguages: [(code: String, name: String)] = [
        ("en", "English"),
        ("zh-Hans", "简体中文"),
    ]

    /// What to show for the language picker's "follow the system" row.
    static var systemLanguageLabel: String {
        String(localized: "settings.general.language.system", comment: "Language picker: follow the OS")
    }

    /// Applies the language choice.
    ///
    /// macOS resolves the interface language once at launch, so this writes
    /// `AppleLanguages` and tells the caller whether a relaunch is needed. The UI
    /// offers that restart rather than performing it silently.
    /// - Returns: `true` when the change needs a relaunch to take effect.
    func applyLanguageOverride() -> Bool {
        let defaults = UserDefaults.standard
        let key = "AppleLanguages"
        let current = defaults.stringArray(forKey: key)

        let desired: [String]?
        switch languageOverride {
        case .none:
            // Removing the key restores the system's own choice.
            desired = nil
        case .some(let code):
            desired = [code]
        }

        if desired == current || (desired == nil && current == nil) {
            return false
        }
        if let desired {
            defaults.set(desired, forKey: key)
        } else {
            defaults.removeObject(forKey: key)
        }
        return true
    }
}
