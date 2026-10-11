//
//  ClipboardPanel.swift
//  MacCV
//
//  The menu-bar popover: a list of recent clips with live previews, plus actions
//  for restoring, copying as plain text, and clearing.
//

import SwiftUI

struct ClipboardPanel: View {
    @Environment(ClipboardStore.self) private var store
    @Environment(AppSettings.self) private var settings
    @Environment(UpdateModel.self) private var updates
    @Environment(\.openSettings) private var openSettings
    @State private var query = ""

    private var filtered: [ClipSnapshot] {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return store.history }
        return store.history.filter { snapshot in
            snapshot.items.contains { item in
                item.preview.localizedCaseInsensitiveContains(trimmed)
                    || (item.plainText?.localizedCaseInsensitiveContains(trimmed) ?? false)
                    || item.filePaths.contains { $0.localizedCaseInsensitiveContains(trimmed) }
            }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()

            if let issue = store.accessIssue {
                AccessBanner(error: issue)
                Divider()
            }

            if store.history.isEmpty {
                EmptyState()
            } else if filtered.isEmpty {
                NoResults(query: query)
            } else {
                historyList
            }

            Divider()
            footer
        }
        .frame(width: 380, height: 480)
        .searchable(
            text: $query,
            placement: .toolbar,
            prompt: Text(String(localized: "panel.search.prompt"))
        )
    }

    // MARK: - Sections

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "clipboard.fill")
                .foregroundStyle(.tint)
            Text("MacCV")
                .font(.headline)

            Spacer()

            Button {
                store.readCurrent(recording: true)
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .help(String(localized: "panel.refresh.help"))

            Button {
                store.isPaused.toggle()
            } label: {
                Image(systemName: store.isPaused ? "play.fill" : "pause.fill")
            }
            .buttonStyle(.borderless)
            .help(store.isPaused
                ? String(localized: "panel.resume.help")
                : String(localized: "panel.pause.help"))

            Button {
                openSettings()
            } label: {
                Image(systemName: "gearshape")
            }
            .buttonStyle(.borderless)
            .help(String(localized: "panel.settings"))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var historyList: some View {
        ScrollView {
            LazyVStack(spacing: 0) {
                ForEach(filtered) { snapshot in
                    ClipRow(snapshot: snapshot)
                    Divider()
                }
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Text(clipCountLabel)
                .font(.caption)
                .foregroundStyle(.secondary)

            // A pending update is worth surfacing here, where it is seen.
            if updates.phase.isBusy || updates.phase.hasUpdate {
                Image(systemName: updates.phase.hasUpdate
                    ? "arrow.down.circle.fill"
                    : "arrow.triangle.2.circlepath")
                    .font(.caption)
                    .foregroundStyle(.tint)
                    .accessibilityLabel(Text("settings.tab.updates"))
            }

            Spacer()

            Text(String(format: String(localized: "panel.engineVersion"), store.engineVersion))
                .font(.caption2)
                .foregroundStyle(.tertiary)

            Button(String(localized: "panel.clearHistory"), role: .destructive) {
                store.forgetAll()
            }
            .buttonStyle(.borderless)
            .font(.caption)

            Button(String(localized: "panel.quit")) {
                NSApplication.shared.terminate(nil)
            }
            .buttonStyle(.borderless)
            .font(.caption)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }

    /// Pluralised with a real plural rule rather than string concatenation, so a
    /// language with more than two forms is not baked into the call site.
    private var clipCountLabel: String {
        String(
            format: String(localized: "panel.clipCount.one"),
            store.history.count
        )
    }
}

// MARK: - Rows

/// The sensitive kinds found in an entry, as a tooltip.
private func redactionTooltip(_ snapshot: ClipSnapshot) -> String {
    let kinds = Set(snapshot.items.flatMap(\.maskedKinds))
    guard !kinds.isEmpty else { return "" }
    return kinds.map(\.label).sorted().joined(separator: "\n")
}

private struct ClipRow: View {
    @Environment(ClipboardStore.self) private var store
    @Environment(AppSettings.self) private var settings
    let snapshot: ClipSnapshot

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: snapshot.kind.symbolName)
                .frame(width: 18)
                .foregroundStyle(.secondary)

            VStack(alignment: .leading, spacing: 2) {
                Text(snapshot.preview.isEmpty ? snapshot.kind.displayName : snapshot.preview)
                    .lineLimit(2)
                    .font(.system(size: 12))
                HStack(spacing: 5) {
                    Text("\(snapshot.kind.displayName) · \(snapshot.byteLength.formattedBytes)")
                    // Say when values were hidden, so a masked entry is not
                    // mistaken for the original content.
                    if snapshot.isRedacted {
                        Text("·")
                        Label(
                            String(localized: "row.redacted.badge"),
                            systemImage: "eye.slash"
                        )
                        .labelStyle(.titleAndIcon)
                        .foregroundStyle(.orange)
                        .help(redactionTooltip(snapshot))
                    }
                }
                .font(.caption2)
                .foregroundStyle(.tertiary)
            }

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .contentShape(Rectangle())
        .onTapGesture(count: 2) { store.restore(snapshot) }
        .contextMenu {
            Button(String(localized: "row.paste")) { store.restore(snapshot) }
            Button(String(localized: "row.pastePlain")) { store.restoreAsPlainText(snapshot) }

            // Only offered when it can actually work: an action that silently does
            // nothing is worse than no action at all.
            if settings.securePaste, store.accessibilityPermission().isGranted,
               let text = snapshot.items.first?.plainText, !text.isEmpty {
                Button(String(localized: "row.pasteSecure")) {
                    store.pasteWithoutClipboard(text)
                }
            }

            if let firstPath = snapshot.items.first?.filePaths.first {
                Divider()
                Button(String(localized: "row.reveal")) {
                    NSWorkspace.shared.activateFileViewerSelecting([
                        URL(fileURLWithPath: firstPath)
                    ])
                }
            }

            Divider()
            Button(String(localized: "row.forget"), role: .destructive) {
                store.forget(snapshot)
            }
        }
        .help(String(localized: "row.hint.doubleClick"))
    }
}

// MARK: - Placeholders

private struct AccessBanner: View {
    let error: ClipboardError
    @Environment(ClipboardStore.self) private var store

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.caption)
                // Only meaningful where the OS implements the policy API; on
                // macOS 15.0–15.3 there is no policy to describe.
                if store.accessBehavior == .alwaysDeny {
                    Text(String(localized: "error.accessDenied.policy"))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if case .accessDenied = error {
                    Button(String(localized: "error.openSettings")) {
                        _ = maccv_secure_input_open_settings()
                    }
                    .controlSize(.small)
                }
            }
            Spacer()
        }
        .padding(10)
        .background(.orange.opacity(0.12))
    }

    /// The engine's own description, with the denial case given a fuller
    /// explanation than a one-liner can carry.
    private var title: String {
        if case .accessDenied = error {
            return String(localized: "error.accessDenied.body")
        }
        return error.errorDescription ?? String(localized: "error.accessDenied.title")
    }
}

private struct EmptyState: View {
    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: "clipboard")
                .font(.system(size: 32))
                .foregroundStyle(.tertiary)
            Text(String(localized: "empty.title"))
                .font(.callout)
            Text(String(localized: "empty.subtitle"))
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct NoResults: View {
    let query: String
    var body: some View {
        ContentUnavailableView.search(text: query)
    }
}

// MARK: - Formatting

private extension Int {
    /// Renders a byte count compactly, e.g. `1.2 MB`.
    var formattedBytes: String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        formatter.allowedUnits = [.useKB, .useMB]
        formatter.isAdaptive = true
        return formatter.string(fromByteCount: Int64(self))
    }
}
