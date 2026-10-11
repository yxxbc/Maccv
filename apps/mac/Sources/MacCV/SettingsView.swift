//
//  SettingsView.swift
//  MacCV
//
//  The Settings window: General, Privacy, Updates and About.
//
//  Every string goes through `String(localized:)` against the catalog in
//  Resources/Localizable.xcstrings, so adding a language means translating that
//  file and nothing else.
//

import SwiftUI

struct SettingsView: View {
    @Environment(AppSettings.self) private var settings
    @Environment(UpdateModel.self) private var updates
    @Environment(ClipboardStore.self) private var store

    var body: some View {
        TabView {
            GeneralSettingsTab()
                .tabItem { Label(String(localized: "settings.tab.general"), systemImage: "gearshape") }
            PrivacySettingsTab()
                .tabItem { Label(String(localized: "settings.tab.privacy"), systemImage: "lock.shield") }
            UpdatesSettingsTab()
                .tabItem { Label(String(localized: "settings.tab.updates"), systemImage: "arrow.triangle.2.circlepath") }
            AboutTab()
                .tabItem { Label(String(localized: "settings.tab.about"), systemImage: "info.circle") }
        }
        .frame(width: 560, height: 460)
    }
}

// MARK: - General

private struct GeneralSettingsTab: View {
    @Environment(AppSettings.self) private var settings
    @Environment(ClipboardStore.self) private var store
    @State private var needsRelaunch = false

    var body: some View {
        @Bindable var settings = settings

        Form {
            Section {
                Picker(String(localized: "settings.general.limit.title"), selection: $settings.historyLimit) {
                    Text("50").tag(50)
                    Text("100").tag(100)
                    Text("200").tag(200)
                    Text("500").tag(500)
                    Text("1000").tag(1000)
                }
                Text(String(localized: "settings.general.limit.detail"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section {
                Picker(String(localized: "settings.general.language.title"), selection: $settings.languageOverride) {
                    Text(AppSettings.systemLanguageLabel).tag(String?.none)
                    ForEach(AppSettings.supportedLanguages, id: \.code) { language in
                        Text(language.name).tag(String?.some(language.code))
                    }
                }

                if needsRelaunch {
                    HStack(spacing: 8) {
                        Image(systemName: "arrow.clockwise.circle")
                            .foregroundStyle(.orange)
                        Text(String(localized: "settings.general.language.restartNeeded"))
                            .font(.caption)
                        Spacer()
                        Button(String(localized: "settings.general.language.restart")) {
                            relaunch()
                        }
                            .controlSize(.small)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .onChange(of: settings.historyLimit) { _, newValue in
            store.historyLimit = newValue
            settings.save()
        }
        .onChange(of: settings.languageOverride) { _, _ in
            needsRelaunch = settings.applyLanguageOverride()
            settings.save()
        }
        .onAppear {
            store.historyLimit = settings.historyLimit
            // On appear this only restores whatever was already stored; the
            // relaunch prompt is for a change the user makes, so the result is
            // discarded here on purpose.
            _ = settings.applyLanguageOverride()
        }
    }

    /// Relaunches the app so a new interface language takes effect.
    ///
    /// `open` on a fresh instance followed by terminating this one, because macOS
    /// resolves `AppleLanguages` only at launch and there is no in-process way to
    /// re-resolve it. macOS also refuses `open -n` for an app already running, so
    /// the bundle is opened by path after this instance exits.
    private func relaunch() {
        let bundlePath = Bundle.main.bundlePath
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/bin/sh")
        // Wait for this process to exit, then reopen the bundle.
        task.arguments = [
            "-c",
            "while kill -0 \(ProcessInfo.processInfo.processIdentifier) 2>/dev/null; do sleep 0.1; done; /usr/bin/open \"\(bundlePath)\"",
        ]
        try? task.run()
        NSApplication.shared.terminate(nil)
    }
}

// MARK: - Privacy

private struct PrivacySettingsTab: View {
    @Environment(AppSettings.self) private var settings
    @Environment(ClipboardStore.self) private var store
    @State private var accessibility: AccessibilityPermission = .notGranted

    var body: some View {
        @Bindable var settings = settings

        Form {
            Section {
                Toggle(isOn: $settings.maskSensitiveContent) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(String(localized: "settings.privacy.masking.title"))
                        Text(String(localized: "settings.privacy.masking.detail"))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .toggleStyle(.switch)

                if settings.maskSensitiveContent {
                    Toggle(isOn: $settings.partialRedaction) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(String(localized: "settings.privacy.partial.title"))
                            Text(String(localized: "settings.privacy.partial.detail"))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .toggleStyle(.switch)
                }

                Toggle(isOn: $settings.skipConcealedEntries) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(String(localized: "settings.privacy.skipConcealed.title"))
                        Text(String(localized: "settings.privacy.skipConcealed.detail"))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .toggleStyle(.switch)
            }

            Section {
                Toggle(isOn: $settings.securePaste) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(String(localized: "settings.privacy.securePaste.title"))
                        Text(String(localized: "settings.privacy.securePaste.detail"))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Text(String(
                            format: String(localized: "settings.privacy.securePaste.limit"),
                            limit
                        ))
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                    }
                }
                .toggleStyle(.switch)
                .disabled(!accessibility.isGranted)

                HStack(spacing: 8) {
                    Image(systemName: accessibility.isGranted
                        ? "checkmark.seal.fill"
                        : "exclamationmark.triangle.fill")
                        .foregroundStyle(accessibility.isGranted ? .green : .orange)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(String(localized: "settings.privacy.permission.title"))
                        Text(accessibility.isGranted
                            ? String(localized: "settings.privacy.permission.granted")
                            : String(localized: "settings.privacy.permission.missing"))
                            .font(.caption)
                        Text(String(localized: "settings.privacy.permission.detail"))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    if !accessibility.isGranted {
                        Button(String(localized: "settings.privacy.permission.action")) {
                            _ = maccv_secure_input_open_settings()
                        }
                        .controlSize(.small)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .onChange(of: settings.maskSensitiveContent) { _, _ in
            settings.save()
            store.reload()
        }
        .onChange(of: settings.partialRedaction) { _, _ in
            settings.save()
            store.reload()
        }
        .onChange(of: settings.skipConcealedEntries) { _, _ in
            settings.save()
            store.reload()
        }
        .onChange(of: settings.securePaste) { _, _ in settings.save() }
        .onAppear { accessibility = store.accessibilityPermission() }
    }

    /// The character limit the engine reports, shown so the restriction is not a
    /// surprise when a long entry is refused.
    private var limit: Int {
        Int(maccv_secure_input_recommended_limit())
    }
}

// MARK: - Updates

private struct UpdatesSettingsTab: View {
    @Environment(AppSettings.self) private var settings
    @Environment(UpdateModel.self) private var updates

    var body: some View {
        @Bindable var settings = settings

        Form {
            Section {
                Picker(String(localized: "updates.policy.title"), selection: $settings.updatePolicy) {
                    ForEach([UpdatePolicy.manual, .notifyOnly, .downloadInBackground], id: \.self) { policy in
                        Text(policy.label).tag(policy)
                    }
                }
                .pickerStyle(.radioGroup)

                Text(String(localized: "updates.policy.detail"))
                    .font(.caption)
                    .foregroundStyle(.secondary)

                Toggle(isOn: $settings.includePrereleases) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(String(localized: "updates.channel.title"))
                        Text(String(localized: "updates.channel.detail"))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .toggleStyle(.switch)
            }

            Section {
                statusRow
                Text(String(localized: "updates.note"))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
        .formStyle(.grouped)
        .onChange(of: settings.updatePolicy) { _, _ in settings.save() }
        .onChange(of: settings.includePrereleases) { _, _ in settings.save() }
    }

    @ViewBuilder
    private var statusRow: some View {
        switch updates.phase {
        case .idle:
            Button(String(localized: "updates.check")) {
                Task { await updates.check() }
            }

        case .checking:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(String(localized: "updates.checking"))
                    .foregroundStyle(.secondary)
            }

        case .upToDate(let current):
            VStack(alignment: .leading, spacing: 6) {
                Label(
                    String(format: String(localized: "updates.current"), current),
                    systemImage: "checkmark.circle.fill"
                )
                .foregroundStyle(.green)
                checkAgainButton
            }

        case .revoked(let update, let current):
            // The running build was withdrawn. The blocking window handles the
            // interaction; this is what the user sees once they ask to update.
            VStack(alignment: .leading, spacing: 8) {
                Label(
                    String(format: String(localized: "updates.revoked"), current, update.version),
                    systemImage: "exclamationmark.octagon.fill"
                )
                .foregroundStyle(.red)
                Text(String(localized: "updates.revoked.detail"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                downloadRow(update)
            }

        case .available(let update):
            VStack(alignment: .leading, spacing: 8) {
                Label(
                    String(format: String(localized: "updates.available"), update.version),
                    systemImage: "arrow.down.circle.fill"
                )
                .foregroundStyle(.tint)

                if !update.notes.isEmpty {
                    ScrollView {
                        Text(update.notes)
                            .font(.caption)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: 120)
                    .padding(6)
                    .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 6))
                }

                downloadRow(update)
            }

        case .failed(let reason):
            VStack(alignment: .leading, spacing: 6) {
                Label(
                    String(format: String(localized: "updates.failed"), reason),
                    systemImage: "exclamationmark.triangle.fill"
                )
                .foregroundStyle(.orange)
                .font(.caption)
                checkAgainButton
            }
        }
    }

    @ViewBuilder
    private func downloadRow(_ update: AvailableUpdate) -> some View {
        switch updates.downloadPhase {
        case .idle, .failed, .signatureRejected, .ready:
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Button(String(localized: "updates.download")) {
                        Task { await updates.downloadAndVerify(update) }
                    }
                    checkAgainButton
                }
                downloadOutcome
            }

        case .downloading(let version):
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(String(localized: "updates.downloading"))
                    .foregroundStyle(.secondary)
            }
            .id(version)
        }
    }

    /// Reports what verification actually concluded, without overstating it.
    @ViewBuilder
    private var downloadOutcome: some View {
        switch updates.downloadPhase {
        case .ready:
            Label(String(localized: "updates.verified"), systemImage: "checkmark.seal.fill")
                .font(.caption)
                .foregroundStyle(.green)

        case .signatureRejected(_, let appPath):
            VStack(alignment: .leading, spacing: 4) {
                Label(String(localized: "updates.signatureInvalid"), systemImage: "xmark.seal.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
                Button(String(localized: "row.reveal")) {
                    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: appPath)])
                }
                .controlSize(.small)
            }

        case .failed(let reason):
            Label(
                String(format: String(localized: "updates.verifyFailed"), reason),
                systemImage: "exclamationmark.triangle.fill"
            )
            .font(.caption)
            .foregroundStyle(.orange)

        case .idle, .downloading:
            EmptyView()
        }
    }

    private var checkAgainButton: some View {
        Button(String(localized: "updates.check")) {
            updates.resetDownloadState()
            Task { await updates.check() }
        }
        .controlSize(.small)
    }
}

// MARK: - About

private struct AboutTab: View {
    var body: some View {
        VStack(spacing: 14) {
            Image(nsImage: NSApplication.shared.applicationIconImage)
                .resizable()
                .frame(width: 96, height: 96)

            VStack(spacing: 4) {
                Text(String(localized: "app.name"))
                    .font(.title2.weight(.semibold))
                Text(String(
                    format: String(localized: "about.version"),
                    UpdateModel.currentVersion,
                    UpdateModel.currentBuild
                ))
                .font(.callout)
                .foregroundStyle(.secondary)
                Text(String(
                    format: String(localized: "about.engine"),
                    ClipboardEngine.engineVersion
                ))
                .font(.caption)
                .foregroundStyle(.tertiary)
            }

            Divider().frame(width: 240)

            VStack(spacing: 6) {
                link(String(localized: "about.website"), AppLinks.website)
                link(String(localized: "about.source"), AppLinks.source)
                link(String(localized: "about.changelog"), AppLinks.changelog)
                link(String(localized: "about.issues"), AppLinks.issues)
            }

            Text(String(localized: "about.license"))
                .font(.caption2)
                .foregroundStyle(.secondary)
            Text(String(localized: "about.copyright"))
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func link(_ title: String, _ url: URL) -> some View {
        Link(destination: url) {
            HStack(spacing: 4) {
                Text(title)
                Image(systemName: "arrow.up.right")
                    .font(.caption2)
            }
        }
        .font(.callout)
    }
}

/// Canonical outbound links.
///
/// The website is the primary destination, not the repository: the project's
/// public address is the site, and the repository is one of the things it links to.
enum AppLinks {
    static let website = URL(string: "https://yxxbc.github.io/Maccv/")!
    static let source = URL(string: "https://yxxbc.github.io/Maccv/source/")!
    static let changelog = URL(string: "https://yxxbc.github.io/Maccv/changelog/")!
    static let issues = URL(string: "https://github.com/yxxbc/Maccv/issues")!
    static let releases = URL(string: "https://github.com/yxxbc/Maccv/releases/latest")!
    static let releaseFeed = "https://yxxbc.github.io/Maccv/latest.json"
    static let previewFeed = "https://yxxbc.github.io/Maccv/preview.json"
}
