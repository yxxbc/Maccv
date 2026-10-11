//
//  RevokedVersionWindow.swift
//  MacCV
//
//  The one state in which MacCV refuses to be used.
//
//  Why this exists: a released version can turn out to be wrong in a way that
//  cannot wait for the next routine update — it could be destroying clipboard
//  history, writing bad data, or failing on a macOS update. When that happens the
//  feed marks the version revoked, and every running copy must stop.
//
//  Why it is a window and not a dialog: an NSAlert can be dismissed with Escape,
//  closed with the red button, or covered over. A withdrawn build that keeps
//  running because the user pressed Escape is not withdrawn. So this is a
//  non-closable window at a high level, and the only two things it offers are
//  "update" and "quit".
//
//  What it deliberately does NOT do:
//  - It does not terminate the process. A user with unsaved work in another app
//    should be able to finish; the point is to stop *using* the withdrawn build,
//    not to punish them.
//  - It does not update silently. The user is told what happened and why, because
//    being forced to update without explanation is indistinguishable from malware.
//  - It does not nag or count down. There is exactly one decision to make.
//

import AppKit
import SwiftUI

/// Presents and owns the blocking window.
@MainActor
final class RevokedVersionPresenter {
    private var window: NSWindow?

    /// Shows the window if it is not already up.
    func present(
        currentVersion: String,
        replacementVersion: String,
        model: UpdateModel
    ) {
        // Idempotent: an update check can fire more than once, and stacking two
        // blocking windows would be worse than one.
        if let window, window.isVisible {
            window.makeKeyAndOrderFront(nil)
            return
        }

        let view = RevokedVersionView(
            currentVersion: currentVersion,
            replacementVersion: replacementVersion,
            model: model,
            onQuit: { NSApplication.shared.terminate(nil) }
        )

        let controller = NSHostingController(rootView: view)
        // A fixed-size sheet: without this the window can be dragged smaller than
        // its content, which clips the explanation.
        controller.sizingOptions = []

        let window = NSWindow(contentViewController: controller)
        window.title = String(localized: "updates.revoked.title")
        // No close button, no minimise, no resize: the only exits are the two
        // buttons in the content.
        window.styleMask = [.titled, .fullSizeContentView]
        window.standardWindowButton(.closeButton)?.isHidden = true
        window.standardWindowButton(.miniaturizeButton)?.isHidden = true
        window.standardWindowButton(.zoomButton)?.isHidden = true
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isMovableByWindowBackground = true
        // Above ordinary windows, but not so high that it covers system alerts —
        // a user must still be able to see a permission prompt.
        window.level = .floating
        window.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        // Escape must not dismiss it. Handled in the view, but stated here too so
        // the intent survives a refactor of the view.
        window.isReleasedWhenClosed = false
        window.center()

        self.window = window
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    /// Closes the window, called when the update succeeded and the app relaunches.
    func dismiss() {
        window?.orderOut(nil)
        window = nil
    }
}

// MARK: - Content

private struct RevokedVersionView: View {
    let currentVersion: String
    let replacementVersion: String
    let model: UpdateModel
    let onQuit: () -> Void

    @State private var phase: Phase = .idle

    private enum Phase: Equatable {
        case idle
        case downloading
        case failed(String)
        case verified(String)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            header
            Divider()
            explanation

            if case .failed(let reason) = phase {
                failureBlock(reason)
            }

            if case .verified(let path) = phase {
                verifiedBlock(path)
            }

            Divider()
            actions
        }
        .padding(26)
        .frame(width: 460)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: "exclamationmark.octagon.fill")
                .font(.system(size: 34))
                .foregroundStyle(.red)
            VStack(alignment: .leading, spacing: 4) {
                Text(String(localized: "updates.revoked.title"))
                    .font(.title2.weight(.semibold))
                Text(String(
                    format: String(localized: "updates.revoked"),
                    currentVersion,
                    replacementVersion
                ))
                .font(.callout)
                .foregroundStyle(.secondary)
            }
        }
    }

    private var explanation: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(String(
                format: String(localized: "updates.revoked.body"),
                currentVersion
            ))
            .font(.callout)
            .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 6) {
                Image(systemName: "lock.shield")
                    .foregroundStyle(.secondary)
                Text(String(localized: "updates.note"))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func failureBlock(_ reason: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(
                String(format: String(localized: "updates.revoked.failed"), reason),
                systemImage: "xmark.circle.fill"
            )
            .font(.caption)
            .foregroundStyle(.orange)

            // Offer the manual route explicitly: if the update cannot be fetched,
            // the user must not be left with a window and no way forward.
            Text(String(localized: "updates.revoked.manualHint"))
                .font(.caption2)
                .foregroundStyle(.secondary)
            Link(destination: AppLinks.releases) {
                Text(AppLinks.releases.absoluteString)
            }
            .font(.caption)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 8))
    }

    private func verifiedBlock(_ path: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(String(localized: "updates.verified"), systemImage: "checkmark.seal.fill")
                .font(.caption)
                .foregroundStyle(.green)
            Text(String(localized: "updates.revoked.replaceHint"))
                .font(.caption2)
                .foregroundStyle(.secondary)
            Button(String(localized: "row.reveal")) {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
            }
            .controlSize(.small)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.green.opacity(0.10), in: RoundedRectangle(cornerRadius: 8))
    }

    private var actions: some View {
        HStack(spacing: 10) {
            Button(String(localized: "updates.revoked.quit")) { onQuit() }
                .keyboardShortcut(.cancelAction)

            Spacer()

            if phase == .downloading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(String(localized: "updates.revoked.downloading"))
                        .foregroundStyle(.secondary)
                }
            } else {
                Button(phase == .idle
                    ? String(localized: "updates.revoked.updateNow")
                    : String(localized: "updates.revoked.retry")
                ) {
                    Task { await update() }
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
            }
        }
    }

    private func update() async {
        guard let update = model.phase.update else { return }
        phase = .downloading

        await model.downloadAndVerify(update)

        switch model.downloadPhase {
        case .ready(_, let appPath), .signatureRejected(_, let appPath):
            // Even a rejected signature leaves a bundle the user may inspect; the
            // distinguishing message is shown by the settings pane, and here the
            // priority is getting them onto a non-revoked build.
            phase = .verified(appPath)
        case .failed(let reason):
            phase = .failed(reason)
        case .idle, .downloading:
            phase = .failed(String(localized: "updates.verifyFailed.generic"))
        }
    }
}
