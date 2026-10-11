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
        .searchable(text: $query, placement: .toolbar, prompt: "Search clips")
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
            .help("Re-read the clipboard now")

            Button {
                store.isPaused.toggle()
            } label: {
                Image(systemName: store.isPaused ? "play.fill" : "pause.fill")
            }
            .buttonStyle(.borderless)
            .help(store.isPaused ? "Resume recording" : "Pause recording")
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
            Text("\(store.history.count) clip\(store.history.count == 1 ? "" : "s")")
                .font(.caption)
                .foregroundStyle(.secondary)

            Spacer()

            Text("engine \(store.engineVersion)")
                .font(.caption2)
                .foregroundStyle(.tertiary)

            Button("Clear history", role: .destructive) {
                store.forgetAll()
            }
            .buttonStyle(.borderless)
            .font(.caption)

            Button("Quit") {
                NSApplication.shared.terminate(nil)
            }
            .buttonStyle(.borderless)
            .font(.caption)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }
}

// MARK: - Rows

private struct ClipRow: View {
    @Environment(ClipboardStore.self) private var store
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
                Text("\(snapshot.kind.displayName) · \(snapshot.byteLength.formattedBytes)")
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
            Button("Paste") { store.restore(snapshot) }
            Button("Paste as Plain Text") { store.restoreAsPlainText(snapshot) }
            Divider()
            Button("Forget", role: .destructive) { store.forget(snapshot) }
        }
        .help("Double-click to put this back on the clipboard")
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
                Text(error.errorDescription ?? "Clipboard unavailable")
                    .font(.caption)
                if store.accessBehavior.isDenied {
                    Text("Current policy: never allow.")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer()
        }
        .padding(10)
        .background(.orange.opacity(0.12))
    }
}

private struct EmptyState: View {
    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: "clipboard")
                .font(.system(size: 32))
                .foregroundStyle(.tertiary)
            Text("Nothing copied yet")
                .font(.callout)
            Text("Copy something and it will appear here.")
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
