//
//  UpdateModel.swift
//  MacCV
//
//  Drives the update UI on top of the Rust updater.
//
//  The engine does the work that has to be correct — checksum, signature,
//  quarantine — and this layer only decides when to ask and how to report. Nothing
//  here re-implements verification, deliberately: two implementations of a
//  security check is one too many.
//

import AppKit
import Foundation
import Observation

/// A newer release the engine found.
struct AvailableUpdate: Identifiable, Equatable {
    let id = UUID()
    let version: String
    let downloadURL: String
    let sha256: String
    let notes: String
}

/// Where an update check currently stands.
enum UpdatePhase: Equatable {
    case idle
    case checking
    /// The running version is current.
    case upToDate(current: String)
    case available(AvailableUpdate)
    case failed(reason: String)

    var isBusy: Bool { self == .checking }

    /// Whether a newer release is waiting.
    var hasUpdate: Bool {
        if case .available = self { return true }
        return false
    }

    /// The available update, if there is one.
    var update: AvailableUpdate? {
        if case .available(let update) = self { return update }
        return nil
    }
}

/// What happened to a download and verification attempt.
enum DownloadPhase: Equatable {
    case idle
    case downloading(version: String)
    /// Verified and extracted; the bundle is on disk and de-quarantined.
    case ready(version: String, appPath: String)
    /// Verified the bytes but not the signature, so nothing was de-quarantined.
    case signatureRejected(version: String, appPath: String)
    case failed(reason: String)
}

@MainActor
@Observable
final class UpdateModel {
    private(set) var phase: UpdatePhase = .idle
    private(set) var downloadPhase: DownloadPhase = .idle

    /// When the last successful check ran, for the settings row.
    private(set) var lastChecked: Date?

    private let settings: AppSettings

    init(settings: AppSettings) {
        self.settings = settings
    }

    /// The version of the running bundle, from `Info.plist`.
    static var currentVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
            ?? ClipboardEngine.engineVersion
    }

    /// The build number, for the About row.
    static var currentBuild: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "1"
    }

    /// Runs a check. Uses the official feed unless overridden.
    ///
    /// Safe to call from a timer: a check downloads nothing and writes nothing.
    func check(manifestURL: String? = nil) async {
        guard !phase.isBusy else { return }
        phase = .checking

        let current = Self.currentVersion
        let channel = settings.updateChannel

        // The engine call shells out to curl, so keep it off the main actor.
        let result = await Task.detached(priority: .utility) {
            UpdateCheckBridge.run(
                currentVersion: current,
                manifestURL: manifestURL,
                channel: channel
            )
        }.value

        lastChecked = Date()
        phase = result
    }

    /// Downloads, verifies, extracts and de-quarantines the available update.
    ///
    /// On success the panel reveals the verified bundle in Finder. MacCV does not
    /// replace its own running bundle: a half-completed swap is worse than asking
    /// the user to drag one file.
    func downloadAndVerify(_ update: AvailableUpdate) async {
        if case .downloading = downloadPhase { return }
        downloadPhase = .downloading(version: update.version)

        let staging = Self.stagingDirectory()
        let result = await Task.detached(priority: .utility) {
            UpdateDownloadBridge.run(update: update, stagingDirectory: staging)
        }.value

        downloadPhase = result

        if case .ready(_, let appPath) = result {
            // Show the user where it landed, so installing is one drag.
            let url = URL(fileURLWithPath: appPath)
            NSWorkspace.shared.activateFileViewerSelecting([url])
        }
    }

    /// Clears the download state so the button returns to its normal form.
    func resetDownloadState() {
        downloadPhase = .idle
    }

    /// Where verified downloads are staged.
    ///
    /// Inside the user's caches directory: the system may reclaim it, which is the
    /// right lifecycle for an artifact the user might never install.
    private static func stagingDirectory() -> String {
        let base = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        let directory = base.appendingPathComponent("MacCV/Updates", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.path
    }
}

// MARK: - Engine bridges
//
// Casting the Rust C ABI into Swift values happens here rather than in the view,
// so the memory rules live next to the code that must obey them.

/// Wraps `maccv_update_check`.
enum UpdateCheckBridge {
    static func run(
        currentVersion: String,
        manifestURL: String?,
        channel: UpdateChannel
    ) -> UpdatePhase {
        var out: UnsafeMutablePointer<MaccvUpdateResult>?
        let status = currentVersion.withCString { version in
            if let manifestURL {
                return manifestURL.withCString { url in
                    maccv_update_check(version, url, channel.cValue, &out)
                }
            }
            return maccv_update_check(version, nil, channel.cValue, &out)
        }

        guard status == MACCV_ERROR_OK, let out else {
            return .failed(reason: ClipboardError(code: status).localizedDescription)
        }
        // The result owns every string it hands back; release it once the Swift
        // values have been copied out.
        defer { maccv_update_result_free(out) }

        switch maccv_update_result_status(out) {
        case 1:
            return .available(
                AvailableUpdate(
                    version: stringToSwift(maccv_update_result_version(out)),
                    downloadURL: stringToSwift(maccv_update_result_download_url(out)),
                    sha256: stringToSwift(maccv_update_result_sha256(out)),
                    notes: stringToSwift(maccv_update_result_notes(out))
                )
            )
        case 0:
            return .upToDate(current: currentVersion)
        default:
            let reason = stringToSwift(maccv_update_result_reason(out))
            return .failed(reason: reason.isEmpty ? "unknown error" : reason)
        }
    }
}

/// Wraps `maccv_update_download`.
enum UpdateDownloadBridge {
    static func run(update: AvailableUpdate, stagingDirectory: String) -> DownloadPhase {
        var out: UnsafeMutablePointer<MaccvStagedUpdate>?
        let status = update.downloadURL.withCString { url in
            update.sha256.withCString { sha in
                update.version.withCString { version in
                    stagingDirectory.withCString { staging in
                        maccv_update_download(url, sha, version, staging, &out)
                    }
                }
            }
        }

        guard status == MACCV_ERROR_OK, let out else {
            return .failed(reason: ClipboardError(code: status).localizedDescription)
        }
        defer { maccv_staged_update_free(out) }

        let appPath = stringToSwift(maccv_staged_update_app_path(out))
        let reason = stringToSwift(maccv_staged_update_reason(out))

        if !reason.isEmpty || appPath.isEmpty {
            return .failed(reason: reason.isEmpty ? "the download could not be staged" : reason)
        }
        // A verified hash with an invalid signature means macOS would refuse to
        // launch it, so report that plainly instead of pretending it is ready.
        if !maccv_staged_update_signature_valid(out) {
            return .signatureRejected(version: update.version, appPath: appPath)
        }
        return .ready(version: update.version, appPath: appPath)
    }
}

// MARK: - C enum bridging

extension UpdateChannel {
    var cValue: MaccvUpdateChannel {
        switch self {
        case .prerelease: MACCV_UPDATE_CHANNEL_PRERELEASE
        default: MACCV_UPDATE_CHANNEL_STABLE
        }
    }
}

extension UpdatePolicy {
    var label: String {
        switch self {
        case .manual: String(localized: "updates.policy.manual", comment: "Update policy: manual")
        case .notifyOnly: String(localized: "updates.policy.notify", comment: "Update policy: notify")
        case .downloadInBackground:
            String(localized: "updates.policy.background", comment: "Update policy: background")
        }
    }
}

extension RedactionMode {
    var cValue: MaccvRedactionMode {
        switch self {
        case .partial: MACCV_REDACTION_MODE_PARTIAL
        default: MACCV_REDACTION_MODE_FULL
        }
    }
}
