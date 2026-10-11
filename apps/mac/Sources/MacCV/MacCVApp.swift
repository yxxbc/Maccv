//
//  MacCVApp.swift
//  MacCV
//
//  App entry point. A clipboard manager lives in the menu bar, so the app is an
//  accessory (LSUIElement) with a MenuBarExtra rather than a windowed app.
//

import SwiftUI

@main
struct MacCVApp: App {
    /// Persisted preferences, shared by every scene.
    @State private var settings: AppSettings
    /// The single engine instance, shared by every view.
    @State private var store: ClipboardStore
    /// Update state, driven from Settings.
    @State private var updates: UpdateModel

    init() {
        // `--self-test` runs the Swift ↔ Rust checks and exits without starting
        // the UI. CI uses it; a human never needs to.
        if CommandLine.arguments.contains("--self-test") {
            exit(SelfTest.run())
        }

        let settings = AppSettings.load()
        let store = ClipboardStore()
        store.settings = settings
        store.historyLimit = settings.historyLimit

        _settings = State(initialValue: settings)
        _store = State(initialValue: store)
        _updates = State(initialValue: UpdateModel(settings: settings))
    }

    /// Owns the blocking window shown when the running version is revoked.
    @State private var revokedPresenter = RevokedVersionPresenter()

    var body: some Scene {
        MenuBarExtra {
            ClipboardPanel()
                .environment(store)
                .environment(settings)
                .environment(updates)
        } label: {
            // The label tracks the store so the icon reacts to copies, without a
            // timer in the view layer.
            Image(systemName: store.isPaused ? "clipboard" : "clipboard.fill")
                .accessibilityLabel(Text("app.name"))
        }
        .menuBarExtraStyle(.window)
        .onChange(of: updates.isRevoked, initial: true) { _, revoked in
            // Checked at launch and on every later check. If the running build has
            // been withdrawn, the only correct next step is to say so.
            guard revoked, case .revoked(let update, let current) = updates.phase else { return }
            revokedPresenter.present(
                currentVersion: current,
                replacementVersion: update.version,
                model: updates
            )
        }

        // Opening Settings from the menu bar. SwiftUI supplies the ⌘, shortcut and
        // the standard Settings menu item for this scene.
        Settings {
            SettingsView()
                .environment(settings)
                .environment(store)
                .environment(updates)
        }
    }
}
