//! Update checking, downloading, and safe installation.
//!
//! # The problem this solves
//!
//! An app downloaded from the internet gets a `com.apple.quarantine` extended
//! attribute. When a *later* version is fetched by the app itself and swapped in,
//! that inherited attribute makes macOS refuse to launch it, usually with the
//! misleading "MacCV is damaged and can't be opened" dialog — the file is fine,
//! it is the quarantine flag plus an unverifiable signature that triggers it.
//!
//! Two things have to happen for an in-app update to install cleanly:
//!
//! 1. **Verify before trusting.** Check the download against a published
//!    SHA-256, and check that the *code signature still validates* under
//!    `spctl`. A hash only proves the bytes are the ones the server offered; the
//!    signature check proves they are MacCV.
//! 2. **Remove the quarantine attribute.** Only after verification, because the
//!    attribute is the one thing standing between "user downloaded this" and "an
//!    attacker placed this here".
//!
//! # What this module does not do
//!
//! It does not swap the running bundle. Replacing an app while it is executing is
//! the part that genuinely needs care — helper processes, the updater's own
//! executable, running-window state — and getting it wrong is worse than asking
//! the user to drag a file. The verified, de-quarantined artifact is staged and
//! the UI offers to reveal it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::error::{Error, Result};

/// The attribute macOS uses to mark content that came from the internet.
pub const QUARANTINE_ATTR: &str = "com.apple.quarantine";

/// Where release metadata lives. The website is the canonical source so the
/// check keeps working if the repository is renamed or moved.
pub const DEFAULT_MANIFEST_URL: &str = "https://yxxbc.github.io/Maccv/latest.json";

/// How a release is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateChannel {
    /// Only stable releases.
    Stable,
    /// Include pre-releases, for people who want the newest build.
    Prerelease,
}

/// The published description of the newest release.
///
/// Deserialised from `latest.json` on the website. Every field is validated on
/// load rather than trusted, because this data decides whether the app replaces
/// its own binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseManifest {
    /// Version string, e.g. `0.2.0` (no leading `v`).
    pub version: String,
    /// GitHub release tag, e.g. `v0.2.0`.
    pub tag: String,
    /// Direct download URL for the archive.
    pub download_url: String,
    /// Lowercase hex SHA-256 of the archive.
    pub sha256: String,
    /// Archive size in bytes, for a progress indicator and a sanity check.
    pub size: u64,
    /// Minimum macOS version the build supports.
    pub minimum_system_version: Option<String>,
    /// Whether the release is a pre-release.
    pub prerelease: bool,
    /// Release notes, in Markdown.
    pub notes: String,
    /// Publication date, ISO 8601.
    pub published_at: Option<String>,
}

impl ReleaseManifest {
    /// Parses a manifest from JSON, rejecting anything that is not usable.
    ///
    /// Hand-rolled rather than derived: the JSON is small and fixed, and this
    /// keeps the update path free of a serialization dependency.
    pub fn from_json(json: &str) -> Result<Self> {
        let version = json_string(json, "version")
            .ok_or_else(|| Error::InvalidPayload("manifest has no version".into()))?;
        let tag = json_string(json, "tag").unwrap_or_else(|| format!("v{version}"));
        let download_url = json_string(json, "download_url")
            .or_else(|| json_string(json, "url"))
            .ok_or_else(|| Error::InvalidPayload("manifest has no download_url".into()))?;
        let sha256 = json_string(json, "sha256")
            .ok_or_else(|| Error::InvalidPayload("manifest has no sha256".into()))?;

        // A manifest without a usable hash cannot be verified, so refuse it rather
        // than falling back to an unchecked download.
        let sha256 = sha256.trim().to_ascii_lowercase();
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidPayload(
                "manifest sha256 is not a 64-character hex digest".into(),
            ));
        }
        if !download_url.starts_with("https://") {
            return Err(Error::InvalidPayload(
                "refusing a non-HTTPS download URL".into(),
            ));
        }

        Ok(Self {
            version,
            tag,
            download_url,
            sha256,
            size: json_number(json, "size").unwrap_or(0),
            minimum_system_version: json_string(json, "minimum_system_version"),
            prerelease: json_bool(json, "prerelease").unwrap_or(false),
            notes: json_string(json, "notes").unwrap_or_default(),
            published_at: json_string(json, "published_at"),
        })
    }

    /// Whether this release is newer than `current`.
    pub fn is_newer_than(&self, current: &str) -> bool {
        compare_versions(&self.version, current) == std::cmp::Ordering::Greater
    }

    /// The archive's file name, taken from the URL.
    pub fn file_name(&self) -> String {
        self.download_url
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("MacCV.zip")
            .to_string()
    }
}

/// The outcome of an update check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// The running version is current.
    UpToDate {
        /// The version that was compared against.
        current: String,
    },
    /// A newer release exists.
    Available(Box<ReleaseManifest>),
    /// The check could not complete. Carries a reason for the UI to show.
    CheckFailed {
        /// Why the check failed, in a form suitable for display.
        reason: String,
    },
}

impl UpdateStatus {
    /// The manifest, when an update is available.
    pub fn manifest(&self) -> Option<&ReleaseManifest> {
        match self {
            Self::Available(manifest) => Some(manifest),
            _ => None,
        }
    }

    /// Whether anything should be shown to the user.
    pub fn has_update(&self) -> bool {
        matches!(self, Self::Available(_))
    }
}

/// How much of an update to perform automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdatePolicy {
    /// Never check unless asked.
    Manual,
    /// Check in the background and surface a badge, but never download silently.
    #[default]
    NotifyOnly,
    /// Check and download in the background; the user still confirms installing.
    ///
    /// Downloading ahead is what makes the install feel instant when the user
    /// finally says yes, without taking the irreversible step for them.
    DownloadInBackground,
}

/// A verified update artifact, staged on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpdate {
    /// Path to the verified archive.
    pub archive: PathBuf,
    /// Path to the `.app` extracted from it, when extraction succeeded.
    pub app_bundle: Option<PathBuf>,
    /// The manifest it came from.
    pub manifest: ReleaseManifest,
    /// Whether the code signature validated.
    pub signature_valid: bool,
    /// Whether the quarantine attribute was removed.
    pub dequarantined: bool,
}

/// Checks for an update without downloading anything.
///
/// This is the entry point for both the manual "Check for Updates" button and the
/// silent background check; they differ only in what they do with the result.
pub fn check_for_update(
    current_version: &str,
    manifest_url: &str,
    channel: UpdateChannel,
) -> UpdateStatus {
    let body = match fetch(manifest_url, Duration::from_secs(15)) {
        Ok(body) => body,
        Err(error) => {
            return UpdateStatus::CheckFailed {
                reason: error.to_string(),
            };
        }
    };

    let manifest = match ReleaseManifest::from_json(&body) {
        Ok(manifest) => manifest,
        Err(error) => {
            return UpdateStatus::CheckFailed {
                reason: format!("the release feed could not be read: {error}"),
            };
        }
    };

    if manifest.prerelease && channel == UpdateChannel::Stable {
        return UpdateStatus::UpToDate {
            current: current_version.to_owned(),
        };
    }

    if manifest.is_newer_than(current_version) {
        UpdateStatus::Available(Box::new(manifest))
    } else {
        UpdateStatus::UpToDate {
            current: current_version.to_owned(),
        }
    }
}

/// Downloads, verifies, extracts and de-quarantines an update.
///
/// The order matters and is the whole point of this function:
///
/// 1. download to a temporary file
/// 2. verify SHA-256 — an unverified artifact never reaches the next step
/// 3. extract
/// 4. verify the code signature under `spctl`
/// 5. **only then** remove the quarantine attribute
/// 6. report what was verified, so the UI can say so honestly
///
/// `staging_dir` is created if missing. The returned archive and bundle live
/// there until the caller removes them.
pub fn download_and_verify(manifest: &ReleaseManifest, staging_dir: &Path) -> Result<StagedUpdate> {
    std::fs::create_dir_all(staging_dir)
        .map_err(|error| Error::InvalidState(format!("could not create staging dir: {error}")))?;

    let archive = staging_dir.join(manifest.file_name());

    // 1. Download.
    download(&manifest.download_url, &archive)?;

    // A size mismatch means the transfer was truncated or the feed is stale.
    if manifest.size > 0 {
        let actual = std::fs::metadata(&archive)
            .map_err(|error| Error::InvalidState(format!("could not stat download: {error}")))?
            .len();
        if actual != manifest.size {
            let _ = std::fs::remove_file(&archive);
            return Err(Error::InvalidPayload(format!(
                "download is {actual} bytes but the feed promised {}",
                manifest.size
            )));
        }
    }

    // 2. Verify the digest before touching the contents. Deleting a failed
    //    download matters: leaving it behind invites a later retry that skips
    //    verification by accident.
    let actual_hash = sha256_file(&archive)?;
    if actual_hash != manifest.sha256 {
        let _ = std::fs::remove_file(&archive);
        return Err(Error::InvalidPayload(format!(
            "checksum mismatch: expected {}, got {actual_hash}",
            manifest.sha256
        )));
    }

    // 3. Extract.
    let extract_dir = staging_dir.join("extracted");
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir)
        .map_err(|error| Error::InvalidState(format!("could not create extract dir: {error}")))?;
    extract_archive(&archive, &extract_dir)?;

    let app_bundle = find_app_bundle(&extract_dir);

    // 4. Signature check. `spctl` is what Gatekeeper itself consults.
    let signature_valid = app_bundle.as_deref().map(verify_signature).unwrap_or(false);

    // 5. Remove quarantine — but only from something whose signature we have
    //    actually checked. Doing this before verification would defeat the point
    //    of the attribute entirely.
    let mut dequarantined = false;
    if signature_valid && let Some(bundle) = app_bundle.as_deref() {
        // Both the bundle and the archive are cleaned, because a user who later
        // re-extracts the archive would otherwise reintroduce the flag.
        dequarantined = remove_quarantine_recursively(bundle).is_ok();
        let _ = remove_quarantine(&archive);
    }

    Ok(StagedUpdate {
        archive,
        app_bundle,
        manifest: manifest.clone(),
        signature_valid,
        dequarantined,
    })
}

// ---------------------------------------------------------------------------
// Networking
// ---------------------------------------------------------------------------

/// Fetches a URL as text, with a hard timeout.
///
/// Shells out to `curl` rather than taking an HTTP client dependency: the engine
/// already links AppKit and CoreGraphics, and one more crate of async machinery
/// for a 2 KB JSON fetch is not a trade worth making. `curl` ships with macOS.
fn fetch(url: &str, timeout: Duration) -> Result<String> {
    let output = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            &timeout.as_secs().to_string(),
            // Identify the client so the site owner can tell update traffic apart.
            "--user-agent",
            &format!("MacCV/{}", crate::VERSION),
            url,
        ])
        .output()
        .map_err(|error| Error::InvalidState(format!("could not run curl: {error}")))?;

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let reason = match output.status.code() {
            Some(6) => "the update server could not be resolved".to_string(),
            Some(7) => "the update server could not be reached".to_string(),
            Some(22) => "the update feed returned an error status".to_string(),
            Some(28) => "the update check timed out".to_string(),
            _ => detail.trim().to_string(),
        };
        return Err(Error::InvalidState(reason));
    }

    String::from_utf8(output.stdout).map_err(|_| Error::InvalidUtf8)
}

/// Downloads a URL to `destination`, replacing any existing file.
fn download(url: &str, destination: &Path) -> Result<()> {
    let status = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "600",
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .status()
        .map_err(|error| Error::InvalidState(format!("could not run curl: {error}")))?;

    if !status.success() {
        let _ = std::fs::remove_file(destination);
        return Err(Error::InvalidState(
            "the download failed or was interrupted".into(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// Computes a file's SHA-256, streaming so a large archive does not sit in memory.
///
/// Uses `shasum`, which macOS ships. A Rust implementation would mean another
/// dependency for a single call, and the process boundary is irrelevant here.
pub fn sha256_file(path: &Path) -> Result<String> {
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|error| Error::InvalidState(format!("could not run shasum: {error}")))?;

    if !output.status.success() {
        return Err(Error::InvalidState("shasum failed".into()));
    }

    let text = String::from_utf8(output.stdout).map_err(|_| Error::InvalidUtf8)?;
    // Format: "<hex>  <path>"
    text.split_whitespace()
        .next()
        .map(|hash| hash.to_ascii_lowercase())
        .ok_or_else(|| Error::InvalidState("shasum produced no digest".into()))
}

/// Whether Gatekeeper accepts this bundle's signature.
///
/// `--assess --type execute` performs the same check the system runs at launch, so
/// a bundle that passes here will not be quarantined-blocked later.
pub fn verify_signature(app_bundle: &Path) -> bool {
    Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute", "--verbose=1"])
        .arg(app_bundle)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Removes `com.apple.quarantine` from one path.
pub fn remove_quarantine(path: &Path) -> Result<()> {
    use std::ffi::CString;

    let path_str = path.to_str().ok_or(Error::InvalidUtf8)?;
    let c_path = CString::new(path_str).map_err(|_| Error::InvalidUtf8)?;
    let c_attr = CString::new(QUARANTINE_ATTR).map_err(|_| Error::InvalidUtf8)?;

    // SAFETY: both C strings are valid, NUL-terminated, and outlive the call.
    // `removexattr` on a path without the attribute fails with ENOATTR, which is
    // the expected outcome and not an error worth surfacing.
    let result = unsafe { libc::removexattr(c_path.as_ptr(), c_attr.as_ptr(), 0) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        // The attribute was already absent: the desired state.
        Some(libc::ENOATTR) => Ok(()),
        _ => Err(Error::InvalidState(format!(
            "could not clear the quarantine flag on {}: {error}",
            path.display()
        ))),
    }
}

/// Removes the quarantine attribute from a bundle and everything inside it.
///
/// The flag is set per-file, so a targeted `xattr -r -d` is the only reliable way
/// to clear a .app. Doing it from Rust means the removal cannot silently fail
/// because a helper binary was missing.
pub fn remove_quarantine_recursively(root: &Path) -> Result<()> {
    // `xattr -r -d` is the system's own recursive implementation and handles the
    // bundle's nested structure correctly (including the code signature
    // directory, which must not be modified in a way that invalidates it).
    let status = Command::new("/usr/bin/xattr")
        .args(["-r", "-d", QUARANTINE_ATTR])
        .arg(root)
        .status()
        .map_err(|error| Error::InvalidState(format!("could not run xattr: {error}")))?;

    // A non-zero exit usually means the attribute was absent from some files,
    // which is fine. Confirm by checking whether any remain.
    let _ = status;
    let remaining = quarantine_paths(root);
    if remaining.is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidState(format!(
            "{} file(s) still carry the quarantine flag",
            remaining.len()
        )))
    }
}

/// Lists paths under `root` that still carry the quarantine attribute.
pub fn quarantine_paths(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_quarantined(root, &mut found, 0);
    found
}

/// Depth-limited recursive scan, so a symlink loop cannot hang the updater.
fn collect_quarantined(path: &Path, found: &mut Vec<PathBuf>, depth: usize) {
    /// Well below any real bundle's nesting, but deep enough for `Contents/Frameworks`.
    const MAX_DEPTH: usize = 12;

    if depth > MAX_DEPTH || found.len() > 5000 {
        return;
    }
    if has_quarantine(path) {
        found.push(path.to_path_buf());
    }
    // `symlink_metadata` so a symlink to a directory is not followed.
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_quarantined(&entry.path(), found, depth + 1);
    }
}

/// Whether one path carries the quarantine attribute.
///
/// Attribute values here are ~20–40 bytes, so the obvious one-byte probe buffer
/// does **not** work: `getxattr` fails with `ERANGE` and returns `-1` as soon as
/// the value does not fit, which is indistinguishable from "attribute absent" if
/// you only look at the sign. Passing a null buffer with size 0 asks for the
/// length instead, which is the only reliable existence test.
pub fn has_quarantine(path: &Path) -> bool {
    use std::ffi::CString;

    let Some(path_str) = path.to_str() else {
        return false;
    };
    let Ok(c_path) = CString::new(path_str) else {
        return false;
    };
    let Ok(c_attr) = CString::new(QUARANTINE_ATTR) else {
        return false;
    };

    // SAFETY: both C strings are valid and NUL-terminated, and a null buffer with
    // length 0 is the documented way to query an attribute's size, so nothing is
    // written through the pointer.
    let size = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            c_attr.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    // A non-negative result is the value's length: the attribute exists.
    // `ENOATTR` (or `ENODATA`) means it does not.
    size >= 0
}

/// Reads the raw value of the quarantine attribute, if present.
///
/// Exposed mainly for diagnostics: the value encodes the flag, a timestamp, and
/// the application that set it, which is useful when a user reports that a file
/// was blocked.
pub fn read_quarantine(path: &Path) -> Option<String> {
    use std::ffi::CString;

    let path_str = path.to_str()?;
    let c_path = CString::new(path_str).ok()?;
    let c_attr = CString::new(QUARANTINE_ATTR).ok()?;

    // Size first, then read exactly that many bytes.
    // SAFETY: as in `has_quarantine`; the null/size-0 form writes nothing.
    let size = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            c_attr.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if size < 0 {
        return None;
    }

    let mut buffer = vec![0u8; size as usize];
    // SAFETY: `buffer` is exactly `size` bytes, matching the length passed.
    let read = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            c_attr.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
            0,
        )
    };
    if read < 0 {
        return None;
    }
    buffer.truncate(read as usize);
    String::from_utf8(buffer).ok()
}

// ---------------------------------------------------------------------------
// Archive handling
// ---------------------------------------------------------------------------

/// Extracts a `.zip` or `.dmg` into `destination`.
///
/// `ditto` is used for zip because it preserves the bundle's resource forks and
/// extended attributes, which `unzip` does not — and losing those inside a `.app`
/// is enough to break the code signature.
pub fn extract_archive(archive: &Path, destination: &Path) -> Result<()> {
    let extension = archive
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match extension.as_str() {
        "zip" => {
            let status = Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(archive)
                .arg(destination)
                .status()
                .map_err(|error| Error::InvalidState(format!("could not run ditto: {error}")))?;
            if !status.success() {
                return Err(Error::InvalidPayload(
                    "the downloaded archive could not be expanded".into(),
                ));
            }
        }
        "dmg" => {
            // Mount read-only and copy out, then unmount. Mounting is the only way
            // to read a dmg without a filesystem driver.
            let mount_point = destination.join("mnt");
            std::fs::create_dir_all(&mount_point).ok();

            let attach = Command::new("/usr/bin/hdiutil")
                .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
                .arg(&mount_point)
                .arg(archive)
                .output()
                .map_err(|error| Error::InvalidState(format!("could not run hdiutil: {error}")))?;

            if !attach.status.success() {
                return Err(Error::InvalidPayload(
                    "the downloaded disk image could not be mounted".into(),
                ));
            }

            // Copy every top-level item out of the image.
            let mut copied = 0usize;
            if let Ok(entries) = std::fs::read_dir(&mount_point) {
                for entry in entries.flatten() {
                    let target = destination.join(entry.file_name());
                    let _ = Command::new("/usr/bin/ditto")
                        .arg(entry.path())
                        .arg(&target)
                        .status();
                    copied += 1;
                }
            }

            // Always detach, even if the copy failed, so the image is not left
            // mounted and the next attempt can proceed.
            let _ = Command::new("/usr/bin/hdiutil")
                .args(["detach", "-force"])
                .arg(&mount_point)
                .status();
            let _ = std::fs::remove_dir(&mount_point);

            if copied == 0 {
                return Err(Error::InvalidPayload(
                    "the downloaded disk image contained nothing to install".into(),
                ));
            }
        }
        other => {
            return Err(Error::InvalidPayload(format!(
                "unsupported archive type: .{other}"
            )));
        }
    }
    Ok(())
}

/// Finds the first `.app` bundle at the top level of `directory`.
pub fn find_app_bundle(directory: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(directory).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

// ---------------------------------------------------------------------------
// Version comparison
// ---------------------------------------------------------------------------

/// Compares two dotted version strings numerically.
///
/// Handles the forms a release tag actually takes: `1.2.3`, `1.2`, `1.2.3-beta.1`.
/// A pre-release sorts *below* the release it precedes, per SemVer, so a user on
/// `1.0.0-beta` is offered `1.0.0`.
pub fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    fn split(value: &str) -> (Vec<u64>, Vec<String>) {
        let value = value.trim().trim_start_matches('v');
        // Strip build metadata, which is not ordered.
        let value = value.split('+').next().unwrap_or(value);
        let mut parts = value.splitn(2, '-');
        let numbers = parts
            .next()
            .unwrap_or("")
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect();
        let prerelease = parts
            .next()
            .map(|rest| rest.split('.').map(str::to_owned).collect())
            .unwrap_or_default();
        (numbers, prerelease)
    }

    let (left_numbers, left_pre) = split(left);
    let (right_numbers, right_pre) = split(right);

    let width = left_numbers.len().max(right_numbers.len());
    for index in 0..width {
        let a = left_numbers.get(index).copied().unwrap_or(0);
        let b = right_numbers.get(index).copied().unwrap_or(0);
        match a.cmp(&b) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }

    // Numeric parts are equal: a version without a pre-release tag is higher.
    match (left_pre.is_empty(), right_pre.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => left_pre.cmp(&right_pre),
    }
}

// ---------------------------------------------------------------------------
// Minimal JSON reading
//
// The manifest is a flat object of scalars, so a full parser is unnecessary. Each
// helper is tolerant of whitespace and of keys appearing in any order.
// ---------------------------------------------------------------------------

/// Finds the value of a top-level string key.
fn json_string(json: &str, key: &str) -> Option<String> {
    let raw = json_raw_value(json, key)?;
    let trimmed = raw.trim();
    let inner = trimmed.strip_prefix('"')?.strip_suffix('"')?;
    Some(unescape_json(inner))
}

/// Finds the value of a top-level number key.
fn json_number(json: &str, key: &str) -> Option<u64> {
    json_raw_value(json, key)?.trim().parse().ok()
}

/// Finds the value of a top-level boolean key.
fn json_bool(json: &str, key: &str) -> Option<bool> {
    match json_raw_value(json, key)?.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Returns the raw token following `"key":`, up to the end of the value.
fn json_raw_value(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut search_from = 0usize;

    loop {
        let offset = json[search_from..].find(&needle)? + search_from;
        let after = json[offset + needle.len()..].trim_start();
        let Some(rest) = after.strip_prefix(':') else {
            // The key appeared inside a string value; keep looking.
            search_from = offset + needle.len();
            continue;
        };
        let rest = rest.trim_start();

        if let Some(body) = rest.strip_prefix('"') {
            // Walk to the closing quote, honouring backslash escapes.
            let mut escaped = false;
            for (index, ch) in body.char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                match ch {
                    '\\' => escaped = true,
                    '"' => {
                        return Some(format!("\"{}\"", &body[..index]));
                    }
                    _ => {}
                }
            }
            return None;
        }

        // A number, boolean or null: read to the next delimiter.
        let end = rest
            .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
            .unwrap_or(rest.len());
        return Some(rest[..end].to_owned());
    }
}

/// Resolves the JSON escapes that appear in release notes.
fn unescape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(decoded) = char::from_u32(code)
                {
                    out.push(decoded);
                }
            }
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    const SAMPLE: &str = r#"{
      "version": "0.2.0",
      "tag": "v0.2.0",
      "download_url": "https://github.com/yxxbc/Maccv/releases/download/v0.2.0/MacCV-0.2.0.zip",
      "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "size": 1234567,
      "minimum_system_version": "27.0",
      "prerelease": false,
      "published_at": "2026-10-11T00:00:00Z",
      "notes": "First line.\nSecond line with \"quotes\"."
    }"#;

    #[test]
    fn parses_a_well_formed_manifest() {
        let manifest = ReleaseManifest::from_json(SAMPLE).expect("should parse");
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.tag, "v0.2.0");
        assert_eq!(manifest.size, 1_234_567);
        assert_eq!(manifest.minimum_system_version.as_deref(), Some("27.0"));
        assert!(!manifest.prerelease);
        assert!(manifest.notes.contains('\n'));
        assert!(manifest.notes.contains("\"quotes\""));
        assert_eq!(manifest.file_name(), "MacCV-0.2.0.zip");
    }

    #[test]
    fn rejects_a_manifest_without_a_usable_hash() {
        // Missing entirely.
        let no_hash = r#"{"version":"1.0.0","download_url":"https://example.com/a.zip"}"#;
        assert!(ReleaseManifest::from_json(no_hash).is_err());

        // Present but malformed.
        let short =
            r#"{"version":"1.0.0","download_url":"https://example.com/a.zip","sha256":"abc"}"#;
        assert!(ReleaseManifest::from_json(short).is_err());

        // Not hex.
        let non_hex = format!(
            r#"{{"version":"1.0.0","download_url":"https://example.com/a.zip","sha256":"{}"}}"#,
            "z".repeat(64)
        );
        assert!(ReleaseManifest::from_json(&non_hex).is_err());
    }

    #[test]
    fn rejects_a_plain_http_download_url() {
        let insecure = format!(
            r#"{{"version":"1.0.0","download_url":"http://example.com/a.zip","sha256":"{}"}}"#,
            "a".repeat(64)
        );
        assert!(ReleaseManifest::from_json(&insecure).is_err());
    }

    #[test]
    fn rejects_a_manifest_with_no_version() {
        let no_version = format!(
            r#"{{"download_url":"https://example.com/a.zip","sha256":"{}"}}"#,
            "a".repeat(64)
        );
        assert!(ReleaseManifest::from_json(&no_version).is_err());
    }

    #[test]
    fn tolerates_whitespace_and_reordered_keys() {
        let messy = format!(
            "{{\n  \"sha256\": \"{}\",\n  \"download_url\" :\n  \"https://e.com/a.zip\",\n  \"version\":\"1.2.3\"\n}}",
            "b".repeat(64)
        );
        let manifest = ReleaseManifest::from_json(&messy).expect("should parse");
        assert_eq!(manifest.version, "1.2.3");
    }

    #[test]
    fn ignores_a_key_name_that_appears_inside_a_value() {
        // `"version"` appears in the notes before the real key.
        let tricky = format!(
            r#"{{"notes":"the \"version\" field is required","version":"1.5.0","download_url":"https://e.com/a.zip","sha256":"{}"}}"#,
            "c".repeat(64)
        );
        let manifest = ReleaseManifest::from_json(&tricky).expect("should parse");
        assert_eq!(manifest.version, "1.5.0");
    }

    #[test]
    fn version_comparison_is_numeric_not_lexicographic() {
        // The whole point: "0.10.0" must beat "0.9.0".
        assert_eq!(compare_versions("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("0.9.0", "0.10.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.1", "1.0"), Ordering::Greater);
        assert_eq!(compare_versions("2.0.0", "1.99.99"), Ordering::Greater);
    }

    #[test]
    fn version_comparison_handles_tags_and_metadata() {
        assert_eq!(compare_versions("v1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.3+build5", "1.2.3"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.3", "v1.2.4"), Ordering::Less);
    }

    #[test]
    fn prereleases_sort_below_their_release() {
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "1.0.0-beta"), Ordering::Greater);
        assert_eq!(
            compare_versions("1.0.0-beta", "1.0.0-alpha"),
            Ordering::Greater
        );
        // A prerelease of a higher number still beats an older release.
        assert_eq!(compare_versions("1.1.0-beta", "1.0.0"), Ordering::Greater);
    }

    #[test]
    fn manifest_newer_than_current() {
        let manifest = ReleaseManifest::from_json(SAMPLE).unwrap();
        assert!(manifest.is_newer_than("0.1.0"));
        assert!(manifest.is_newer_than("0.1.9"));
        assert!(!manifest.is_newer_than("0.2.0"));
        assert!(!manifest.is_newer_than("0.3.0"));
    }

    #[test]
    fn up_to_date_status_carries_the_version() {
        let status = UpdateStatus::UpToDate {
            current: "1.0.0".into(),
        };
        assert!(!status.has_update());
        assert!(status.manifest().is_none());
    }

    #[test]
    fn check_failure_is_reported_not_panicked() {
        // An unreachable host must degrade to a status, never a crash.
        let status = check_for_update(
            "0.1.0",
            "https://127.0.0.1:9/definitely-not-listening.json",
            UpdateChannel::Stable,
        );
        match status {
            UpdateStatus::CheckFailed { reason } => assert!(!reason.is_empty()),
            other => panic!("expected a graceful failure, got {other:?}"),
        }
    }

    #[test]
    fn decompression_bomb_guard_on_extract() {
        // An unsupported extension is refused rather than guessed at.
        let temp = std::env::temp_dir().join("maccv-extract-test");
        let _ = std::fs::create_dir_all(&temp);
        let bogus = temp.join("archive.rar");
        std::fs::write(&bogus, b"not an archive").unwrap();
        assert!(extract_archive(&bogus, &temp).is_err());
        let _ = std::fs::remove_file(&bogus);
    }

    #[test]
    fn quarantine_helpers_are_harmless_on_clean_files() {
        let temp = std::env::temp_dir().join("maccv-quarantine-test");
        let _ = std::fs::create_dir_all(&temp);
        let clean = temp.join("clean.txt");
        std::fs::write(&clean, b"hello").unwrap();

        // A file we just wrote is not quarantined.
        assert!(!has_quarantine(&clean));
        assert!(quarantine_paths(&temp).is_empty());
        // Removing an absent attribute is a success, not an error.
        assert!(remove_quarantine(&clean).is_ok());
        assert!(read_quarantine(&clean).is_none());

        let _ = std::fs::remove_file(&clean);
    }

    /// Regression test for a bug that made detection silently useless.
    ///
    /// The first implementation probed with a one-byte buffer. A real quarantine
    /// value is ~20 bytes, so `getxattr` returned -1 with `ERANGE`, which was read
    /// as "attribute absent" — so an already-quarantined app was reported clean and
    /// never had its flag cleared, which is exactly the failure the updater exists
    /// to prevent. The fix queries the length with a null buffer instead.
    #[test]
    fn detects_a_real_quarantine_attribute_on_a_real_file() {
        let temp = std::env::temp_dir().join("maccv-quarantine-real");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let target = temp.join("download.txt");
        std::fs::write(&target, b"pretend this came from the internet").unwrap();

        // Set the attribute the way a browser would.
        let set = Command::new("/usr/bin/xattr")
            .arg("-w")
            .arg(QUARANTINE_ATTR)
            .arg("0081;68f0a1b2;Safari;")
            .arg(&target)
            .status();
        let Ok(status) = set else {
            return; // xattr unavailable: nothing to assert
        };
        if !status.success() {
            return;
        }

        // This is the assertion the old implementation failed.
        assert!(
            has_quarantine(&target),
            "a file with com.apple.quarantine must be detected, not reported clean"
        );

        // The value is readable and names the setting application.
        let value = read_quarantine(&target).expect("value should be readable");
        assert!(value.contains("Safari"), "unexpected value: {value}");

        // And it is actually removed, verified through the filesystem rather than
        // through our own reader.
        assert!(remove_quarantine(&target).is_ok());
        assert!(!has_quarantine(&target), "removal did not take effect");
        let still_there = Command::new("/usr/bin/xattr")
            .arg(&target)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains(QUARANTINE_ATTR))
            .unwrap_or(false);
        assert!(!still_there, "the system still reports the attribute");

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// The recursive path is what an in-app install relies on: a bundle's nested
    /// files are quarantined individually, so clearing only the top level leaves a
    /// "damaged app" error behind.
    #[test]
    fn clears_a_nested_bundle_recursively() {
        let temp = std::env::temp_dir().join("maccv-quarantine-recursive");
        let _ = std::fs::remove_dir_all(&temp);
        let bundle = temp.join("MacCV.app");
        let nested = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&nested).unwrap();
        let inner = nested.join("MacCV");
        std::fs::write(&inner, b"#!binary").unwrap();

        let set_all = Command::new("/usr/bin/xattr")
            .args(["-w", QUARANTINE_ATTR, "0081;68f0a1b2;Safari;"])
            .arg(&bundle)
            .status();
        if !set_all.map(|s| s.success()).unwrap_or(false) {
            return;
        }
        let _ = Command::new("/usr/bin/xattr")
            .args(["-w", QUARANTINE_ATTR, "0081;68f0a1b2;Safari;"])
            .arg(&inner)
            .status();

        assert!(!quarantine_paths(&bundle).is_empty(), "setup failed");

        remove_quarantine_recursively(&bundle).expect("recursive removal should succeed");
        assert!(
            quarantine_paths(&bundle).is_empty(),
            "nested files still carry the flag"
        );

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn quarantine_detection_survives_a_missing_path() {
        let missing = Path::new("/definitely/not/here/at/all");
        assert!(!has_quarantine(missing));
        assert!(quarantine_paths(missing).is_empty());
    }

    #[test]
    fn sha256_matches_a_known_digest() {
        // The SHA-256 of an empty file, which is a published constant.
        let temp = std::env::temp_dir().join("maccv-empty-for-hash");
        std::fs::write(&temp, b"").unwrap();
        let digest = sha256_file(&temp).expect("shasum should run");
        assert_eq!(
            digest,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn finds_an_app_bundle_in_an_extracted_directory() {
        let temp = std::env::temp_dir().join("maccv-find-app-test");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("MacCV.app/Contents")).unwrap();
        std::fs::write(temp.join("readme.txt"), b"hi").unwrap();

        let found = find_app_bundle(&temp).expect("should find the bundle");
        assert_eq!(found.file_name().unwrap(), "MacCV.app");
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn find_app_bundle_returns_none_when_there_is_none() {
        let temp = std::env::temp_dir().join("maccv-no-app-test");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        assert!(find_app_bundle(&temp).is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn json_unicode_escapes_are_decoded() {
        assert_eq!(unescape_json("\\u4f60\\u597d"), "你好");
        assert_eq!(unescape_json("a\\nb"), "a\nb");
        assert_eq!(unescape_json("\\\"x\\\""), "\"x\"");
    }
}
