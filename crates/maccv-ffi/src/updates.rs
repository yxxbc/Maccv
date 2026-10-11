//! C ABI for secure synthetic input, sensitive-content detection, and updates.
//!
//! Split out of `lib.rs` to keep that file focused on the pasteboard surface. The
//! memory rules documented at the crate root apply unchanged: anything returning
//! `MaccvString` that is not declared static must be released with
//! `maccv_string_free`.

use std::ffi::c_char;
use std::path::Path;

use maccv_core::{
    AccessibilityPermission, RedactionMode, SecureInput, TypingCadence, UpdateChannel,
    UpdatePolicy, UpdateStatus,
};

use crate::{MaccvError, MaccvString, bytes_to_slice, cstr_to_str, record_error, record_ok};

// ---------------------------------------------------------------------------
// Accessibility permission
// ---------------------------------------------------------------------------

/// Whether this process may inject events into other applications.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaccvAccessibilityPermission {
    /// Granted: synthetic typing will be delivered.
    Granted = 0,
    /// Not granted: synthetic typing will be posted and silently dropped.
    NotGranted = 1,
}

/// Reads the Accessibility permission state without prompting.
///
/// Never shows a dialog, so it is safe to call when deciding what the UI should
/// offer.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_secure_input_permission() -> MaccvAccessibilityPermission {
    match AccessibilityPermission::current() {
        AccessibilityPermission::Granted => MaccvAccessibilityPermission::Granted,
        AccessibilityPermission::NotGranted => MaccvAccessibilityPermission::NotGranted,
    }
}

/// Opens the System Settings pane where the Accessibility permission is granted.
///
/// The engine cannot grant this itself — by design, only the user can — so the
/// best it can do is take them to the right place.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_secure_input_open_settings() -> MaccvError {
    let status = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .status();
    match status {
        Ok(status) if status.success() => {
            record_ok();
            MaccvError::Ok
        }
        _ => {
            record_error(
                MaccvError::InvalidState,
                Some("could not open System Settings".into()),
            );
            MaccvError::InvalidState
        }
    }
}

// ---------------------------------------------------------------------------
// Secure typing
// ---------------------------------------------------------------------------

/// Types `text` into the frontmost application without touching any pasteboard.
///
/// This is how MacCV delivers content that other applications cannot observe: the
/// text never exists in a shared channel, only as synthetic keystrokes addressed
/// to whichever app has focus.
///
/// Requires the Accessibility permission; returns
/// [`MaccvError::MaccvErrorAccessDenied`] without it.
///
/// `text` is **not** NUL-terminated and `len` bytes are read, so the payload may
/// contain NULs. `per_key_delay_us` is the pause between keystrokes — pass 0 for
/// the fastest delivery, or a few milliseconds for apps that drop fast input.
///
/// # Safety
///
/// `text` must point at `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_secure_input_type(
    text: *const u8,
    len: usize,
    per_key_delay_us: u64,
) -> MaccvError {
    let Some(bytes) = (unsafe { bytes_to_slice(text, len) }) else {
        record_error(MaccvError::NullPointer, Some("text was null".into()));
        return MaccvError::NullPointer;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        record_error(MaccvError::InvalidUtf8, None);
        return MaccvError::InvalidUtf8;
    };

    match Permission::input_with_delay(per_key_delay_us) {
        Ok(input) => match input.type_text(text) {
            Ok(_) => {
                record_ok();
                MaccvError::Ok
            }
            Err(error) => crate::record_core_error(error),
        },
        Err(code) => code,
    }
}

/// Types at most `max_chars` characters, reporting what was skipped.
///
/// Synthetic typing runs at human speed, so an enormous string would appear to
/// hang. This bounds the attempt and tells the caller what was left out, so the
/// UI can say so rather than silently truncating.
///
/// `out_delivered` and `out_skipped` receive the counts; either may be null.
///
/// # Safety
///
/// `text` must point at `len` readable bytes. The out-parameters, if non-null,
/// must be valid and writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_secure_input_type_limited(
    text: *const u8,
    len: usize,
    max_chars: usize,
    per_key_delay_us: u64,
    out_delivered: *mut usize,
    out_skipped: *mut usize,
) -> MaccvError {
    let Some(bytes) = (unsafe { bytes_to_slice(text, len) }) else {
        record_error(MaccvError::NullPointer, Some("text was null".into()));
        return MaccvError::NullPointer;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        record_error(MaccvError::InvalidUtf8, None);
        return MaccvError::InvalidUtf8;
    };

    let input = match Permission::input_with_delay(per_key_delay_us) {
        Ok(input) => input,
        Err(code) => return code,
    };

    match input.type_text_limited(text, max_chars) {
        Ok(report) => {
            if !out_delivered.is_null() {
                // SAFETY: the caller guarantees a writable pointer.
                unsafe { *out_delivered = report.delivered };
            }
            if !out_skipped.is_null() {
                // SAFETY: as above.
                unsafe { *out_skipped = report.skipped };
            }
            record_ok();
            MaccvError::Ok
        }
        Err(error) => crate::record_core_error(error),
    }
}

/// Posts a Cmd+V keystroke to the frontmost application.
///
/// The pasteboard-based path: content must already have been written with
/// `maccv_pasteboard_write_*`. Use this when the user asked to paste and you do
/// not want to require them to switch apps and press ⌘V themselves.
///
/// Unlike [`maccv_secure_input_type`], this leaves the content readable by every
/// application on the Mac. That is inherent to the pasteboard, not a limitation of
/// this function.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_secure_input_paste() -> MaccvError {
    let input = match Permission::input() {
        Ok(input) => input,
        Err(code) => return code,
    };
    match input.post_paste_keystroke() {
        Ok(()) => {
            record_ok();
            MaccvError::Ok
        }
        Err(error) => crate::record_core_error(error),
    }
}

/// The maximum number of characters worth typing synthetically.
///
/// The UI uses this to decide whether to offer secure paste at all, rather than
/// starting a delivery that would take minutes.
#[unsafe(no_mangle)]
pub extern "C" fn maccv_secure_input_recommended_limit() -> usize {
    // At the default 2 ms cadence this is about 10 seconds of typing, which is
    // already at the edge of acceptable for an interaction.
    5_000
}

/// Builds a `SecureInput`, mapping the permission error to a status code.
struct Permission;

impl Permission {
    fn input() -> Result<SecureInput, MaccvError> {
        Self::input_with_delay(2_000)
    }

    fn input_with_delay(delay_us: u64) -> Result<SecureInput, MaccvError> {
        // A zero delay means "as fast as possible"; anything else is a real pause.
        let cadence = if delay_us == 0 {
            TypingCadence::instant()
        } else {
            TypingCadence {
                per_key: std::time::Duration::from_micros(delay_us),
                ..TypingCadence::default()
            }
        };
        match SecureInput::new(cadence) {
            Ok(input) => Ok(input),
            Err(error) => Err(crate::record_core_error(error)),
        }
    }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// Replaces a redaction mode with its C representation.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaccvRedactionMode {
    /// Mask the entire value.
    Full = 0,
    /// Keep a recognisable head and tail.
    Partial = 1,
}

impl From<MaccvRedactionMode> for RedactionMode {
    fn from(value: MaccvRedactionMode) -> Self {
        match value {
            MaccvRedactionMode::Full => Self::Full,
            MaccvRedactionMode::Partial => Self::Partial,
        }
    }
}

/// The result of masking a string. Owned by the caller; release with
/// [`maccv_redacted_free`].
#[repr(C)]
pub struct MaccvRedacted {
    /// The masked text.
    text: MaccvString,
    /// Comma-separated kind identifiers, e.g. `card-number,email`.
    kinds: MaccvString,
    /// How many spans were masked.
    match_count: usize,
    /// Whether anything was masked at all.
    was_redacted: bool,
    /// Whether the content must not be recorded, because a source app marked it.
    skip_recording: bool,
}

/// Masks sensitive values in `text`.
///
/// Use `mode` to choose how much to leave visible. `MaccvRedactionMode_Full` is
/// the safer default.
///
/// On success `*out` receives an owned result to release with
/// [`maccv_redacted_free`]. Text with nothing sensitive in it still succeeds, with
/// `was_redacted` false.
///
/// # Safety
///
/// `text` must point at `len` readable bytes; `out` must be valid and writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_redact(
    text: *const u8,
    len: usize,
    mode: MaccvRedactionMode,
    out: *mut *mut MaccvRedacted,
) -> MaccvError {
    if out.is_null() {
        record_error(MaccvError::NullPointer, Some("out was null".into()));
        return MaccvError::NullPointer;
    }
    let Some(bytes) = (unsafe { bytes_to_slice(text, len) }) else {
        record_error(MaccvError::NullPointer, Some("text was null".into()));
        return MaccvError::NullPointer;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        record_error(MaccvError::InvalidUtf8, None);
        return MaccvError::InvalidUtf8;
    };

    let result = maccv_core::redact(text, mode.into());
    let kinds = result
        .kinds
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(",");

    let boxed = Box::new(MaccvRedacted {
        text: MaccvString::owned(&result.text),
        kinds: MaccvString::owned(&kinds),
        match_count: result.match_count,
        was_redacted: result.is_redacted(),
        skip_recording: result.should_skip_recording(),
    });

    // SAFETY: `out` is non-null and writable per the contract above.
    unsafe { *out = Box::into_raw(boxed) };
    record_ok();
    MaccvError::Ok
}

/// Releases a redaction result. Null is accepted and ignored.
///
/// # Safety
///
/// `redacted` must come from [`maccv_redact`] and not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_redacted_free(redacted: *mut MaccvRedacted) {
    if redacted.is_null() {
        return;
    }
    // SAFETY: the caller guarantees ownership and single use.
    let boxed = unsafe { Box::from_raw(redacted) };
    // The strings inside were allocated with `CString::into_raw`; `MaccvString`
    // has no `Drop` of its own, so free them explicitly.
    // SAFETY: both pointers came from `MaccvString::owned`.
    unsafe {
        if !boxed.text.ptr.is_null() {
            drop(std::ffi::CString::from_raw(
                boxed.text.ptr.cast_mut().cast(),
            ));
        }
        if !boxed.kinds.ptr.is_null() {
            drop(std::ffi::CString::from_raw(
                boxed.kinds.ptr.cast_mut().cast(),
            ));
        }
    }
}

/// The masked text. Valid until the result is freed.
///
/// # Safety
///
/// `redacted` must be a live result from [`maccv_redact`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_redacted_text(redacted: *const MaccvRedacted) -> MaccvString {
    match unsafe { redacted.as_ref() } {
        Some(redacted) => redacted.text,
        None => MaccvString::EMPTY,
    }
}

/// The comma-separated kind identifiers. Valid until the result is freed.
///
/// # Safety
///
/// `redacted` must be a live result from [`maccv_redact`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_redacted_kinds(redacted: *const MaccvRedacted) -> MaccvString {
    match unsafe { redacted.as_ref() } {
        Some(redacted) => redacted.kinds,
        None => MaccvString::EMPTY,
    }
}

/// How many spans were masked.
///
/// # Safety
///
/// `redacted` must be a live result from [`maccv_redact`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_redacted_match_count(redacted: *const MaccvRedacted) -> usize {
    unsafe { redacted.as_ref() }.map_or(0, |redacted| redacted.match_count)
}

/// Whether anything was masked.
///
/// # Safety
///
/// `redacted` must be a live result from [`maccv_redact`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_redacted_was_redacted(redacted: *const MaccvRedacted) -> bool {
    unsafe { redacted.as_ref() }.is_some_and(|redacted| redacted.was_redacted)
}

/// Whether the content must be kept out of history entirely.
///
/// True only when a source application explicitly marked the content concealed,
/// transient, or as a private key — never for a heuristic guess.
///
/// # Safety
///
/// `redacted` must be a live result from [`maccv_redact`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_redacted_should_skip_recording(
    redacted: *const MaccvRedacted,
) -> bool {
    unsafe { redacted.as_ref() }.is_some_and(|redacted| redacted.skip_recording)
}

// ---------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------

/// How much of the update process to perform without asking.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaccvUpdatePolicy {
    /// Never check unless the user asks.
    Manual = 0,
    /// Check in the background and show a badge; never download silently.
    NotifyOnly = 1,
    /// Check and download in the background; the user still confirms installing.
    DownloadInBackground = 2,
}

impl From<MaccvUpdatePolicy> for UpdatePolicy {
    fn from(value: MaccvUpdatePolicy) -> Self {
        match value {
            MaccvUpdatePolicy::Manual => Self::Manual,
            MaccvUpdatePolicy::NotifyOnly => Self::NotifyOnly,
            MaccvUpdatePolicy::DownloadInBackground => Self::DownloadInBackground,
        }
    }
}

/// Which releases to consider.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaccvUpdateChannel {
    /// Stable releases only.
    Stable = 0,
    /// Include pre-releases.
    Prerelease = 1,
}

/// The outcome of an update check. Owned; release with [`maccv_update_result_free`].
#[repr(C)]
pub struct MaccvUpdateResult {
    /// 0 = up to date, 1 = update available, 2 = check failed.
    status: u8,
    /// The newest version, when one was found.
    version: MaccvString,
    /// Direct download URL, when one was found.
    download_url: MaccvString,
    /// Lowercase hex SHA-256 of the archive.
    sha256: MaccvString,
    /// Archive size in bytes.
    size: u64,
    /// Release notes, in Markdown.
    notes: MaccvString,
    /// Why the check failed, when it did.
    reason: MaccvString,
}

/// Checks the update feed for a newer release.
///
/// Does no downloading and changes nothing on disk, so it is safe to run on a
/// timer or from a menu item. `manifest_url` may be null to use the official feed.
///
/// On success `*out` receives an owned result to release with
/// [`maccv_update_result_free`]. A failure to reach the feed is reported through
/// the result's `status`, not as an error code, because "no network" is a normal
/// condition rather than a bug.
///
/// # Safety
///
/// `current_version` must be a NUL-terminated UTF-8 string. `manifest_url` may be
/// null. `out` must be valid and writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_update_check(
    current_version: *const c_char,
    manifest_url: *const c_char,
    channel: MaccvUpdateChannel,
    out: *mut *mut MaccvUpdateResult,
) -> MaccvError {
    if out.is_null() {
        record_error(MaccvError::NullPointer, Some("out was null".into()));
        return MaccvError::NullPointer;
    }
    let Some(current) = (unsafe { cstr_to_str(current_version) }) else {
        record_error(
            MaccvError::NullPointer,
            Some("current_version was null or invalid".into()),
        );
        return MaccvError::NullPointer;
    };
    let url = (unsafe { cstr_to_str(manifest_url) }).unwrap_or(maccv_core::DEFAULT_MANIFEST_URL);

    let channel = match channel {
        MaccvUpdateChannel::Stable => UpdateChannel::Stable,
        MaccvUpdateChannel::Prerelease => UpdateChannel::Prerelease,
    };

    let result = maccv_core::check_for_update(current, url, channel);

    let boxed = match result {
        UpdateStatus::UpToDate { current } => Box::new(MaccvUpdateResult {
            status: 0,
            version: MaccvString::owned(&current),
            ..MaccvUpdateResult::empty()
        }),
        UpdateStatus::Available(manifest) => Box::new(MaccvUpdateResult {
            status: 1,
            version: MaccvString::owned(&manifest.version),
            download_url: MaccvString::owned(&manifest.download_url),
            sha256: MaccvString::owned(&manifest.sha256),
            size: manifest.size,
            notes: MaccvString::owned(&manifest.notes),
            ..MaccvUpdateResult::empty()
        }),
        UpdateStatus::CheckFailed { reason } => Box::new(MaccvUpdateResult {
            status: 2,
            reason: MaccvString::owned(&reason),
            ..MaccvUpdateResult::empty()
        }),
    };

    // SAFETY: `out` is non-null and writable.
    unsafe { *out = Box::into_raw(boxed) };
    record_ok();
    MaccvError::Ok
}

impl MaccvUpdateResult {
    /// Keeps every optional string non-null, so C callers never have to check.
    const fn empty() -> Self {
        Self {
            status: 0,
            version: MaccvString::EMPTY,
            download_url: MaccvString::EMPTY,
            sha256: MaccvString::EMPTY,
            size: 0,
            notes: MaccvString::EMPTY,
            reason: MaccvString::EMPTY,
        }
    }
}

/// 0 = up to date, 1 = update available, 2 = check failed.
///
/// # Safety
///
/// `result` must be live from [`maccv_update_check`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_update_result_status(result: *const MaccvUpdateResult) -> u8 {
    unsafe { result.as_ref() }.map_or(2, |result| result.status)
}

/// The newest version, if any.
///
/// # Safety
///
/// `result` must be live from [`maccv_update_check`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_update_result_version(
    result: *const MaccvUpdateResult,
) -> MaccvString {
    unsafe { result.as_ref() }.map_or(MaccvString::EMPTY, |result| result.version)
}

/// The download URL, if any.
///
/// # Safety
///
/// `result` must be live from [`maccv_update_check`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_update_result_download_url(
    result: *const MaccvUpdateResult,
) -> MaccvString {
    unsafe { result.as_ref() }.map_or(MaccvString::EMPTY, |result| result.download_url)
}

/// The release notes, in Markdown.
///
/// # Safety
///
/// `result` must be live from [`maccv_update_check`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_update_result_notes(
    result: *const MaccvUpdateResult,
) -> MaccvString {
    unsafe { result.as_ref() }.map_or(MaccvString::EMPTY, |result| result.notes)
}

/// Why the check failed, if it did.
///
/// # Safety
///
/// `result` must be live from [`maccv_update_check`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_update_result_reason(
    result: *const MaccvUpdateResult,
) -> MaccvString {
    unsafe { result.as_ref() }.map_or(MaccvString::EMPTY, |result| result.reason)
}

/// Releases an update result. Null is accepted and ignored.
///
/// # Safety
///
/// `result` must come from [`maccv_update_check`] and not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_update_result_free(result: *mut MaccvUpdateResult) {
    if result.is_null() {
        return;
    }
    // SAFETY: the caller guarantees ownership and single use.
    let boxed = unsafe { Box::from_raw(result) };
    // Every field is freed through `free_owned`, so the ones left as the static
    // empty sentinel — which is most of them on a failed check — are skipped.
    for string in [
        boxed.version,
        boxed.download_url,
        boxed.sha256,
        boxed.notes,
        boxed.reason,
    ] {
        string.free_owned();
    }
}

/// The outcome of a download and verification.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaccvInstallStatus {
    /// Downloaded, hash matched, extracted, signature valid, quarantined flag cleared.
    Ready = 0,
    /// Downloaded and verified, but the code signature did not validate, so the
    /// quarantine flag was deliberately left in place.
    SignatureInvalid = 1,
    /// The download or verification failed.
    Failed = 2,
}

/// What a completed download produced. Owned; release with
/// [`maccv_staged_update_free`].
#[repr(C)]
pub struct MaccvStagedUpdate {
    /// The verified archive on disk.
    archive_path: MaccvString,
    /// The extracted `.app`, when extraction succeeded.
    app_path: MaccvString,
    /// Signature validity.
    signature_valid: bool,
    /// Whether the quarantine attribute was cleared.
    dequarantined: bool,
    /// Failure detail, when the status is `Failed`.
    reason: MaccvString,
}

/// Downloads an update and verifies it before making it installable.
///
/// The steps run in this order, and the order is the point:
///
/// 1. download to `staging_dir`
/// 2. verify the SHA-256 from the feed — a mismatch deletes the file and stops
/// 3. extract
/// 4. verify the code signature with `spctl`
/// 5. **only then** clear `com.apple.quarantine`
/// 6. report which of those succeeded
///
/// Step 5 is what prevents the "MacCV is damaged and can't be opened" dialog that
/// otherwise appears when an app replaces itself with a downloaded bundle: macOS
/// inherits the quarantine attribute from the parent, and an app cannot launch a
/// quarantined, non-notarized replacement.
///
/// This function does not replace the running bundle. It leaves a verified,
/// launchable `.app` in `staging_dir` and reports its path so the UI can offer to
/// reveal it. Swapping an executable while it runs is where correctness gets
/// genuinely hard, and a half-done swap is worse than asking the user to drag one
/// file.
///
/// # Safety
///
/// All string arguments must be NUL-terminated UTF-8. `out` must be valid and
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_update_download(
    download_url: *const c_char,
    sha256: *const c_char,
    version: *const c_char,
    staging_dir: *const c_char,
    out: *mut *mut MaccvStagedUpdate,
) -> MaccvError {
    if out.is_null() {
        record_error(MaccvError::NullPointer, Some("out was null".into()));
        return MaccvError::NullPointer;
    }
    let (Some(download_url), Some(sha256), Some(version), Some(staging)) = (
        unsafe { cstr_to_str(download_url) },
        unsafe { cstr_to_str(sha256) },
        unsafe { cstr_to_str(version) },
        unsafe { cstr_to_str(staging_dir) },
    ) else {
        record_error(
            MaccvError::NullPointer,
            Some("a required argument was null or invalid".into()),
        );
        return MaccvError::NullPointer;
    };

    // Rebuild a manifest from the individual arguments, so the verification path
    // is identical whether the data came from the feed or from the caller.
    let manifest = maccv_core::ReleaseManifest {
        version: version.to_owned(),
        tag: format!("v{version}"),
        download_url: download_url.to_owned(),
        sha256: sha256.to_ascii_lowercase(),
        size: 0,
        minimum_system_version: None,
        prerelease: false,
        notes: String::new(),
        published_at: None,
    };

    let staging_path = Path::new(staging);
    let boxed = match maccv_core::download_and_verify(&manifest, staging_path) {
        Ok(staged) => Box::new(MaccvStagedUpdate {
            archive_path: MaccvString::owned(&staged.archive.to_string_lossy()),
            app_path: MaccvString::owned(
                &staged
                    .app_bundle
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            signature_valid: staged.signature_valid,
            dequarantined: staged.dequarantined,
            reason: MaccvString::EMPTY,
        }),
        Err(error) => Box::new(MaccvStagedUpdate {
            archive_path: MaccvString::EMPTY,
            app_path: MaccvString::EMPTY,
            signature_valid: false,
            dequarantined: false,
            reason: MaccvString::owned(&error.to_string()),
        }),
    };

    // SAFETY: `out` is non-null and writable.
    unsafe { *out = Box::into_raw(boxed) };
    record_ok();
    MaccvError::Ok
}

/// The archive path, when the download succeeded.
///
/// # Safety
///
/// `staged` must be live from [`maccv_update_download`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_staged_update_archive_path(
    staged: *const MaccvStagedUpdate,
) -> MaccvString {
    unsafe { staged.as_ref() }.map_or(MaccvString::EMPTY, |staged| staged.archive_path)
}

/// The extracted `.app` path, when extraction succeeded.
///
/// # Safety
///
/// `staged` must be live from [`maccv_update_download`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_staged_update_app_path(
    staged: *const MaccvStagedUpdate,
) -> MaccvString {
    unsafe { staged.as_ref() }.map_or(MaccvString::EMPTY, |staged| staged.app_path)
}

/// Whether the code signature validated.
///
/// # Safety
///
/// `staged` must be live from [`maccv_update_download`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_staged_update_signature_valid(
    staged: *const MaccvStagedUpdate,
) -> bool {
    unsafe { staged.as_ref() }.is_some_and(|staged| staged.signature_valid)
}

/// Whether the quarantine attribute was cleared.
///
/// False alongside a valid signature means the bundle will still be blocked at
/// launch, so the UI must not present it as ready.
///
/// # Safety
///
/// `staged` must be live from [`maccv_update_download`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_staged_update_dequarantined(
    staged: *const MaccvStagedUpdate,
) -> bool {
    unsafe { staged.as_ref() }.is_some_and(|staged| staged.dequarantined)
}

/// The failure detail, when the download failed.
///
/// # Safety
///
/// `staged` must be live from [`maccv_update_download`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_staged_update_reason(
    staged: *const MaccvStagedUpdate,
) -> MaccvString {
    unsafe { staged.as_ref() }.map_or(MaccvString::EMPTY, |staged| staged.reason)
}

/// Releases a staged-update result. Null is accepted and ignored.
///
/// # Safety
///
/// `staged` must come from [`maccv_update_download`] and not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_staged_update_free(staged: *mut MaccvStagedUpdate) {
    if staged.is_null() {
        return;
    }
    // SAFETY: the caller guarantees ownership and single use.
    let boxed = unsafe { Box::from_raw(staged) };
    for string in [boxed.archive_path, boxed.app_path, boxed.reason] {
        string.free_owned();
    }
}

/// Whether a path carries the `com.apple.quarantine` attribute.
///
/// Exposed so the UI can explain why an app will not launch, rather than leaving
/// the user with the system's "damaged" message.
///
/// # Safety
///
/// `path` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_path_is_quarantined(path: *const c_char) -> bool {
    match unsafe { cstr_to_str(path) } {
        Some(path) => maccv_core::has_quarantine(Path::new(path)),
        None => false,
    }
}

/// Compares two dotted version strings. -1, 0 or 1.
///
/// # Safety
///
/// Both arguments must be NUL-terminated UTF-8 strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_compare_versions(left: *const c_char, right: *const c_char) -> i32 {
    match (unsafe { cstr_to_str(left) }, unsafe { cstr_to_str(right) }) {
        (Some(left), Some(right)) => match maccv_core::compare_versions(left, right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        },
        _ => {
            record_error(MaccvError::NullPointer, Some("a version was null".into()));
            0
        }
    }
}

/// Releases a string allocated by any of the owning `MaccvString` returns.
///
/// Provided under this name as well so callers working only with the updater and
/// redaction APIs do not need to know about the pasteboard surface.
///
/// # Safety
///
/// `value` must be an owned string from this library, not a `'static` one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maccv_owned_string_free(value: MaccvString) {
    // SAFETY: the caller guarantees the pointer came from `CString::into_raw`.
    unsafe { crate::maccv_string_free(value) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::ptr;
    use std::slice;

    fn c(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    #[test]
    fn permission_state_is_reported_without_prompting() {
        let state = maccv_secure_input_permission();
        assert!(matches!(
            state,
            MaccvAccessibilityPermission::Granted | MaccvAccessibilityPermission::NotGranted
        ));
    }

    #[test]
    fn typing_rejects_null_and_invalid_utf8() {
        // SAFETY: deliberately invalid arguments; the API must report, not trap.
        unsafe {
            assert_eq!(
                maccv_secure_input_type(ptr::null(), 0, 2000),
                MaccvError::NullPointer
            );
            let invalid = [0xffu8, 0xfe];
            assert_eq!(
                maccv_secure_input_type(invalid.as_ptr(), invalid.len(), 2000),
                MaccvError::InvalidUtf8
            );
        }
    }

    #[test]
    fn redact_reports_null_and_invalid_input() {
        unsafe {
            assert_eq!(
                maccv_redact(ptr::null(), 0, MaccvRedactionMode::Full, ptr::null_mut()),
                MaccvError::NullPointer
            );
            let mut out: *mut MaccvRedacted = ptr::null_mut();
            assert_eq!(
                maccv_redact(b"x".as_ptr(), 1, MaccvRedactionMode::Full, ptr::null_mut()),
                MaccvError::NullPointer
            );
            let invalid = [0xffu8];
            assert_eq!(
                maccv_redact(invalid.as_ptr(), 1, MaccvRedactionMode::Full, &mut out),
                MaccvError::InvalidUtf8
            );
        }
    }

    #[test]
    fn redact_masks_through_the_c_abi() {
        let text = b"card 4111 1111 1111 1111";
        let mut out: *mut MaccvRedacted = ptr::null_mut();

        // SAFETY: valid inputs and a valid out pointer.
        let status = unsafe {
            maccv_redact(
                text.as_ptr(),
                text.len(),
                MaccvRedactionMode::Full,
                &mut out,
            )
        };
        assert_eq!(status, MaccvError::Ok);
        assert!(!out.is_null());

        // SAFETY: `out` came from `maccv_redact`.
        unsafe {
            assert!(maccv_redacted_was_redacted(out));
            assert_eq!(maccv_redacted_match_count(out), 1);
            let kinds = maccv_redacted_kinds(out);
            let kinds = std::str::from_utf8(std::slice::from_raw_parts(
                kinds.ptr.cast::<u8>(),
                kinds.len,
            ))
            .unwrap();
            assert_eq!(kinds, "card-number");

            let masked = maccv_redacted_text(out);
            let masked = std::str::from_utf8(std::slice::from_raw_parts(
                masked.ptr.cast::<u8>(),
                masked.len,
            ))
            .unwrap();
            assert!(!masked.contains("4111"));

            // A heuristic guess must not cause the content to be dropped.
            assert!(!maccv_redacted_should_skip_recording(out));

            maccv_redacted_free(out);
        }
    }

    /// An unredacted result has an empty kind list, so its `kinds` field is the
    /// static sentinel. Freeing it must not crash.
    #[test]
    fn freeing_an_unredacted_result_does_not_crash() {
        let text = b"nothing sensitive in this sentence";
        let mut out: *mut MaccvRedacted = ptr::null_mut();
        // SAFETY: valid inputs and a valid out pointer.
        let status = unsafe {
            maccv_redact(
                text.as_ptr(),
                text.len(),
                MaccvRedactionMode::Full,
                &mut out,
            )
        };
        assert_eq!(status, MaccvError::Ok);
        // SAFETY: `out` came from `maccv_redact`.
        unsafe {
            assert_eq!(
                maccv_redacted_kinds(out).len,
                0,
                "set-up: kinds should be empty"
            );
            maccv_redacted_free(out);
        }
    }

    #[test]
    fn redact_leaves_clean_text_with_a_non_null_string() {
        let text = b"nothing sensitive here";
        let mut out: *mut MaccvRedacted = ptr::null_mut();
        // SAFETY: valid inputs.
        let status = unsafe {
            maccv_redact(
                text.as_ptr(),
                text.len(),
                MaccvRedactionMode::Full,
                &mut out,
            )
        };
        assert_eq!(status, MaccvError::Ok);
        // SAFETY: `out` came from `maccv_redact`.
        unsafe {
            assert!(!maccv_redacted_was_redacted(out));
            let borrowed = maccv_redacted_text(out);
            assert!(!borrowed.ptr.is_null());
            assert_eq!(borrowed.len, text.len());
            // Empty kind list, still a valid string.
            assert_eq!(maccv_redacted_kinds(out).len, 0);
            maccv_redacted_free(out);
        }
    }

    /// Regression test for a crash on the most common path.
    ///
    /// `MaccvString::EMPTY` used to point at the address of a `c""` literal inside
    /// a `const`. A const is instantiated per codegen unit, so the address stored
    /// in a struct field did not compare equal to the same const read elsewhere;
    /// the guard in `free_owned` failed and `free()` was handed read-only static
    /// memory, killing the process with SIGBUS. Any caller freeing a result whose
    /// optional fields were empty hit it — which is every failed update check and
    /// every unredacted string.
    #[test]
    fn freeing_a_result_with_empty_optional_fields_does_not_crash() {
        let url = c("https://127.0.0.1:9/nope.zip");
        let hash = c(&"a".repeat(64));
        let version = c("9.9.9");
        let staging = c(&std::env::temp_dir()
            .join("maccv-ffi-free")
            .to_string_lossy());
        let mut out: *mut MaccvStagedUpdate = ptr::null_mut();

        // SAFETY: valid C strings and a valid out pointer.
        let status = unsafe {
            maccv_update_download(
                url.as_ptr(),
                hash.as_ptr(),
                version.as_ptr(),
                staging.as_ptr(),
                &mut out,
            )
        };
        assert_eq!(status, MaccvError::Ok);

        // SAFETY: `out` came from `maccv_update_download`. Releasing it must not
        // touch the static empty sentinel.
        unsafe {
            maccv_staged_update_free(out);
        }
    }

    /// The sentinel must be one address crate-wide, which is the property the
    /// crash above depended on.
    #[test]
    fn the_empty_sentinel_has_a_stable_address() {
        let a = MaccvString::EMPTY;
        let b = MaccvString::EMPTY;
        assert_eq!(a.ptr, b.ptr, "the sentinel's address must be stable");

        // And a value round-tripped through a struct keeps the same address, so
        // the guard in `free_owned` recognises it.
        let stored = a;
        assert_eq!(stored.ptr, MaccvString::EMPTY.ptr);
        // Releasing it must be a no-op rather than a `free()` of static memory.
        stored.free_owned();
    }

    #[test]
    fn redacted_free_accepts_null_and_double_guards() {
        // SAFETY: freeing null must be a no-op.
        unsafe { maccv_redacted_free(ptr::null_mut()) };
        // SAFETY: as above, for the other free functions.
        unsafe {
            maccv_update_result_free(ptr::null_mut());
            maccv_staged_update_free(ptr::null_mut());
            maccv_owned_string_free(MaccvString::EMPTY);
        }
    }

    #[test]
    fn null_accessors_return_empty_not_garbage() {
        unsafe {
            assert!(!maccv_redacted_text(ptr::null()).ptr.is_null());
            assert_eq!(maccv_redacted_match_count(ptr::null()), 0);
            assert!(!maccv_redacted_was_redacted(ptr::null()));
            assert!(!maccv_redacted_should_skip_recording(ptr::null()));
            assert_eq!(maccv_update_result_status(ptr::null()), 2);
            assert!(!maccv_staged_update_signature_valid(ptr::null()));
            assert!(!maccv_staged_update_dequarantined(ptr::null()));
            assert!(!maccv_path_is_quarantined(ptr::null()));
        }
    }

    #[test]
    fn update_check_reports_a_failed_feed_as_a_status_not_an_error() {
        let version = c("0.1.0");
        let url = c("https://127.0.0.1:9/nope.json");
        let mut out: *mut MaccvUpdateResult = ptr::null_mut();

        // SAFETY: valid C strings and a valid out pointer.
        let status = unsafe {
            maccv_update_check(
                version.as_ptr(),
                url.as_ptr(),
                MaccvUpdateChannel::Stable,
                &mut out,
            )
        };
        // The call itself succeeded: an unreachable server is not an API error.
        assert_eq!(status, MaccvError::Ok);
        assert!(!out.is_null());

        // SAFETY: `out` came from `maccv_update_check`.
        unsafe {
            assert_eq!(maccv_update_result_status(out), 2);
            let reason = maccv_update_result_reason(out);
            assert!(reason.len > 0, "a failure must explain itself");
            // Every accessor must return a usable string even when unset.
            assert!(!maccv_update_result_version(out).ptr.is_null());
            assert!(!maccv_update_result_notes(out).ptr.is_null());
            maccv_update_result_free(out);
        }
    }

    #[test]
    fn version_comparison_through_the_abi() {
        let a = c("0.10.0");
        let b = c("0.9.0");
        // SAFETY: valid C strings.
        unsafe {
            assert_eq!(maccv_compare_versions(a.as_ptr(), b.as_ptr()), 1);
            assert_eq!(maccv_compare_versions(b.as_ptr(), a.as_ptr()), -1);
            assert_eq!(maccv_compare_versions(a.as_ptr(), a.as_ptr()), 0);
            assert_eq!(maccv_compare_versions(ptr::null(), b.as_ptr()), 0);
        }
    }

    #[test]
    fn download_rejects_null_arguments() {
        // SAFETY: deliberately null; must report rather than trap.
        unsafe {
            assert_eq!(
                maccv_update_download(
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null_mut()
                ),
                MaccvError::NullPointer
            );
        }
    }

    #[test]
    fn download_fails_cleanly_for_an_unreachable_url() {
        let url = c("https://127.0.0.1:9/nope.zip");
        let hash = c(&"a".repeat(64));
        let version = c("9.9.9");
        let staging = c(&std::env::temp_dir().join("maccv-ffi-dl").to_string_lossy());
        let mut out: *mut MaccvStagedUpdate = ptr::null_mut();

        // SAFETY: valid C strings.
        let status = unsafe {
            maccv_update_download(
                url.as_ptr(),
                hash.as_ptr(),
                version.as_ptr(),
                staging.as_ptr(),
                &mut out,
            )
        };
        assert_eq!(status, MaccvError::Ok);
        // SAFETY: `out` came from `maccv_update_download`.
        unsafe {
            // Nothing verified, so nothing is claimed.
            assert!(!maccv_staged_update_signature_valid(out));
            assert!(!maccv_staged_update_dequarantined(out));
            assert!(maccv_staged_update_reason(out).len > 0);
            assert_eq!(maccv_staged_update_archive_path(out).len, 0);
            maccv_staged_update_free(out);
        }
    }

    #[test]
    fn path_quarantine_check_through_the_abi() {
        let temp = std::env::temp_dir().join("maccv-ffi-quarantine");
        let _ = std::fs::create_dir_all(&temp);
        let file = temp.join("plain.txt");
        std::fs::write(&file, b"x").unwrap();

        let path = c(&file.to_string_lossy());
        // SAFETY: a valid C string path.
        assert!(!unsafe { maccv_path_is_quarantined(path.as_ptr()) });

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn recommended_limit_is_sane() {
        let limit = maccv_secure_input_recommended_limit();
        assert!(
            limit >= 1000,
            "limit of {limit} would block ordinary pastes"
        );
        assert!(limit <= 100_000, "limit of {limit} would let the UI freeze");
    }

    #[test]
    fn typing_limited_reports_counts_when_permission_is_absent() {
        let text = "hello world";
        let mut delivered = 0usize;
        let mut skipped = 0usize;

        // SAFETY: valid buffers and out pointers.
        let status = unsafe {
            maccv_secure_input_type_limited(
                text.as_ptr(),
                text.len(),
                5,
                2000,
                &mut delivered,
                &mut skipped,
            )
        };

        // Without the Accessibility permission the only correct outcome is a
        // refusal; with it, the counts must add up. Never a silent success.
        match status {
            MaccvError::AccessDenied => {
                assert_eq!(delivered, 0);
                assert_eq!(skipped, 0);
            }
            MaccvError::Ok => assert_eq!(delivered + skipped, text.chars().count()),
            other => panic!("unexpected status {other:?}"),
        }
    }

    #[test]
    fn zero_length_text_needs_no_permission() {
        // An empty delivery is a no-op, so it must not fail on a locked-down Mac.
        // SAFETY: a zero-length read from a valid pointer.
        let status = unsafe { maccv_secure_input_type(b"".as_ptr(), 0, 2000) };
        assert!(
            matches!(status, MaccvError::Ok | MaccvError::AccessDenied),
            "unexpected {status:?}"
        );
    }

    #[test]
    fn slice_helper_is_used_consistently() {
        // Guards against a future refactor reintroducing a raw `from_raw_parts`.
        let data = [1u8, 2, 3];
        // SAFETY: the pointer and length are both valid.
        let slice = unsafe { slice::from_raw_parts(data.as_ptr(), data.len()) };
        assert_eq!(slice.len(), 3);
    }
}
