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
    /// The single engine instance, shared by every view.
    @State private var store = ClipboardStore()

    init() {
        // `--self-test` runs the Swift ↔ Rust checks and exits without starting
        // the UI. CI uses it; a human never needs to.
        if CommandLine.arguments.contains("--self-test") {
            exit(SelfTest.run())
        }
    }

    var body: some Scene {
        MenuBarExtra {
            ClipboardPanel()
                .environment(store)
        } label: {
            // The label tracks the store so the icon reacts to copies, without a
            // timer in the view layer.
            Image(systemName: store.isPaused ? "clipboard" : "clipboard.fill")
                .accessibilityLabel("MacCV clipboard history")
        }
        .menuBarExtraStyle(.window)
    }
}
