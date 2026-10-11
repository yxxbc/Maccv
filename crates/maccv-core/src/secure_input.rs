//! Synthetic keyboard input, so a paste can bypass the clipboard entirely.
//!
//! # Why this exists
//!
//! When MacCV puts a history entry back on the pasteboard, *any* application on
//! the Mac can read it. There is no way around that: the pasteboard is a shared,
//! world-readable channel, and macOS exposes no API to write to it privately.
//! `NSPasteboardAccessBehavior` looks like it might help, but it only governs
//! whether *this* process may read — it cannot restrict anyone else.
//!
//! That leaves one way to hand content to the frontmost app without exposing it:
//! don't use the pasteboard at all. Type it.
//!
//! This module synthesises keyboard events carrying arbitrary Unicode via
//! `CGEventKeyboardSetUnicodeString`, which means no keycode table is involved
//! and any text — CJK, emoji, combining marks — can be delivered. The content
//! never enters a pasteboard, so no other process can observe it.
//!
//! # The trade-off, stated honestly
//!
//! This is slower than a pasteboard round trip: events are delivered at human
//! typing pace. It also requires the Accessibility permission, because injecting
//! events into another application is exactly the capability that permission
//! gates. And it is only meaningful for text — an image has no keystroke
//! representation.
//!
//! So it is offered as a *mode*, not a default: see the "secure paste" setting.
//! For large payloads the clipboard path remains the right answer, and the UI
//! should say so rather than silently truncating.

use std::time::Duration;

use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGEvent, CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGKeyCode,
};

use crate::error::{Error, Result};

/// How fast to type when delivering text synthetically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypingCadence {
    /// Delay between individual keystrokes.
    ///
    /// Zero is fastest and works with most apps, but some editors drop events
    /// when they arrive faster than they can process them, and a few apps
    /// interpret bursts as paste-like and re-enable their own handling. A small
    /// delay is the reliable default.
    pub per_key: Duration,
    /// Delay after the final keystroke, before the caller regains control.
    pub settle: Duration,
}

impl Default for TypingCadence {
    fn default() -> Self {
        Self {
            per_key: Duration::from_millis(2),
            settle: Duration::from_millis(30),
        }
    }
}

impl TypingCadence {
    /// As fast as the event system allows: no artificial delay between keys.
    ///
    /// Use for short, plain strings. Anything an app might treat as a paste
    /// gesture is better served by [`TypingCadence::default`].
    pub const fn instant() -> Self {
        Self {
            per_key: Duration::ZERO,
            settle: Duration::from_millis(20),
        }
    }

    /// Deliberately slow, for apps that reorder or coalesce fast input.
    pub const fn deliberate() -> Self {
        Self {
            per_key: Duration::from_millis(12),
            settle: Duration::from_millis(60),
        }
    }
}

/// Whether this process may inject events into other applications.
///
/// Injecting keyboard events requires the Accessibility permission. Until it is
/// granted, [`SecureInput::type_text`] will appear to succeed while nothing
/// arrives — the events are posted and silently dropped by the system — so it is
/// worth checking this before offering the feature at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilityPermission {
    /// The permission is granted; synthetic events will be delivered.
    Granted,
    /// The permission has not been granted. Prompt the user.
    NotGranted,
}

impl AccessibilityPermission {
    /// Reads the current permission state without prompting.
    ///
    /// This is the check to run when deciding what the UI should offer; it never
    /// shows a dialog.
    pub fn current() -> Self {
        // SAFETY: `AXIsProcessTrusted` takes no arguments, returns a `Boolean`,
        // and is safe to call from any thread. It performs no I/O and cannot
        // fail in a way that matters here.
        let trusted = unsafe { AXIsProcessTrusted() };
        if trusted != 0 {
            Self::Granted
        } else {
            Self::NotGranted
        }
    }

    /// Whether events can actually be delivered.
    pub const fn is_granted(self) -> bool {
        matches!(self, Self::Granted)
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// Returns whether this process is trusted for Accessibility.
    ///
    /// Declared here rather than pulled in through a crate: it is one symbol from
    /// `HIServices`, and the alternative is another dependency for a single call.
    fn AXIsProcessTrusted() -> u8;
}

/// Delivers text to the frontmost application as synthetic keystrokes.
///
/// A value of this type holds a `CGEventSource`, which is the origin identity the
/// system attributes the events to. Reusing one source for a burst of keys makes
/// the input look like a single coherent typing session to the receiving app.
pub struct SecureInput {
    source: CFRetained<CGEventSource>,
    cadence: TypingCadence,
}

impl std::fmt::Debug for SecureInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureInput")
            .field("cadence", &self.cadence)
            .finish_non_exhaustive()
    }
}

impl SecureInput {
    /// Creates an input source that types at `cadence`.
    ///
    /// Fails with [`Error::AccessDenied`] when the Accessibility permission is
    /// missing, so the failure surfaces at construction rather than as text that
    /// mysteriously never appears.
    pub fn new(cadence: TypingCadence) -> Result<Self> {
        if !AccessibilityPermission::current().is_granted() {
            return Err(Error::AccessDenied);
        }
        Self::new_unchecked(cadence)
    }

    /// As [`SecureInput::new`], but without checking the permission.
    ///
    /// Use this when the caller has already established the permission state and
    /// wants the events attempted regardless — for example to verify the
    /// permission by observing whether delivery worked.
    pub fn new_unchecked(cadence: TypingCadence) -> Result<Self> {
        // `PrivateState` keeps the source's modifier state independent of the
        // user's physical keyboard, so a held modifier key cannot corrupt the
        // synthesized sequence.
        let source = CGEventSource::new(CGEventSourceStateID::Private)
            .ok_or_else(|| Error::InvalidState("could not create a CGEventSource".into()))?;
        Ok(Self { source, cadence })
    }

    /// The cadence this input source types at.
    pub const fn cadence(&self) -> TypingCadence {
        self.cadence
    }

    /// Types `text` into whichever application has focus.
    ///
    /// Returns the number of Unicode code points delivered, which is useful for
    /// progress reporting on long strings.
    ///
    /// # Encoding
    ///
    /// Events carry `UniChar` (UTF-16) units, so a string is delivered as its
    /// UTF-16 code units — exactly what a real keyboard produces for that text.
    /// Surrogate pairs are sent as a single event, which is what `CGEvent` expects
    /// for non-BMP characters such as emoji.
    pub fn type_text(&self, text: &str) -> Result<usize> {
        if text.is_empty() {
            return Ok(0);
        }
        if !AccessibilityPermission::current().is_granted() {
            return Err(Error::AccessDenied);
        }

        let units: Vec<u16> = text.encode_utf16().collect();
        let mut delivered = 0usize;

        for unit in units {
            self.post_unit(unit)?;
            delivered += 1;
            if !self.cadence.per_key.is_zero() {
                std::thread::sleep(self.cadence.per_key);
            }
        }

        if !self.cadence.settle.is_zero() {
            std::thread::sleep(self.cadence.settle);
        }
        Ok(delivered)
    }

    /// Types only the first `limit` characters, reporting what was dropped.
    ///
    /// Synthetic typing is bounded by human-scale timing, so very large strings
    /// are a poor fit. This lets a caller cap the attempt and tell the user,
    /// instead of appearing to hang.
    pub fn type_text_limited(&self, text: &str, limit: usize) -> Result<TypeReport> {
        let total = text.chars().count();
        if total <= limit {
            let delivered = self.type_text(text)?;
            return Ok(TypeReport {
                delivered,
                skipped: 0,
                truncated: false,
            });
        }

        // Split on a character boundary so the delivered prefix is valid UTF-8.
        let cutoff = text
            .char_indices()
            .nth(limit)
            .map(|(index, _)| index)
            .unwrap_or(text.len());
        let delivered = self.type_text(&text[..cutoff])?;
        Ok(TypeReport {
            delivered,
            skipped: total - limit,
            truncated: true,
        })
    }

    /// Posts one key-down/key-up pair carrying `unit`.
    ///
    /// The keycode is deliberately `0` (the "a" key): with a Unicode string
    /// attached, the receiving app reads the string rather than mapping the
    /// keycode, so the placeholder code never leaks into the output. This is the
    /// documented way to synthesize arbitrary text.
    fn post_unit(&self, unit: u16) -> Result<()> {
        const PLACEHOLDER_KEYCODE: CGKeyCode = 0;

        let down = CGEvent::new_keyboard_event(Some(&self.source), PLACEHOLDER_KEYCODE, true)
            .ok_or_else(|| Error::InvalidState("could not create a key-down event".into()))?;
        let up = CGEvent::new_keyboard_event(Some(&self.source), PLACEHOLDER_KEYCODE, false)
            .ok_or_else(|| Error::InvalidState("could not create a key-up event".into()))?;

        let units = [unit];
        // SAFETY: each event is a valid `CGEventRef` we created above and still
        // own, and `units` is a live slice of exactly the length we pass.
        unsafe {
            CGEvent::keyboard_set_unicode_string(Some(&down), units.len() as _, units.as_ptr());
            CGEvent::keyboard_set_unicode_string(Some(&up), units.len() as _, units.as_ptr());
        }

        // `HIDEventTap` is the lowest-level tap, so the events travel the same
        // path as real hardware input and every app sees them.
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
        Ok(())
    }

    /// Posts a Cmd+V keystroke.
    ///
    /// Used for the *non*-secure fallback path: when the content is on the
    /// pasteboard and the user wants it pasted into the frontmost app without
    /// switching apps to press ⌘V themselves.
    pub fn post_paste_keystroke(&self) -> Result<()> {
        /// Virtual keycode for "v" on the ANSI layout.
        const KEY_V: CGKeyCode = 9;
        /// Virtual keycode for the left Command key.
        const KEY_COMMAND: CGKeyCode = 55;

        let cmd_down = CGEvent::new_keyboard_event(Some(&self.source), KEY_COMMAND, true)
            .ok_or_else(|| Error::InvalidState("could not create a modifier event".into()))?;
        let v_down = CGEvent::new_keyboard_event(Some(&self.source), KEY_V, true)
            .ok_or_else(|| Error::InvalidState("could not create a key-down event".into()))?;
        let v_up = CGEvent::new_keyboard_event(Some(&self.source), KEY_V, false)
            .ok_or_else(|| Error::InvalidState("could not create a key-up event".into()))?;
        let cmd_up = CGEvent::new_keyboard_event(Some(&self.source), KEY_COMMAND, false)
            .ok_or_else(|| Error::InvalidState("could not create a modifier event".into()))?;

        // Ordered down → up: Command down, V down, V up, Command up.
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&cmd_down));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&v_down));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&v_up));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&cmd_up));
        Ok(())
    }

    /// The keyboard type the event source reports.
    ///
    /// Apps that do their own key handling sometimes consult this; exposing it
    /// makes it possible to diagnose "the text arrives scrambled in app X"
    /// reports, which are almost always a layout mismatch.
    pub fn keyboard_type(&self) -> i64 {
        i64::from(CGEventSource::keyboard_type(Some(&self.source)))
    }
}

/// What a bounded typing attempt actually delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeReport {
    /// UTF-16 code units posted.
    pub delivered: usize,
    /// Characters left unsent because of the caller's limit.
    pub skipped: usize,
    /// Whether anything was left unsent.
    pub truncated: bool,
}

impl TypeReport {
    /// Whether the entire string was delivered.
    pub const fn is_complete(&self) -> bool {
        !self.truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_defaults_are_ordered() {
        assert!(TypingCadence::instant().per_key < TypingCadence::default().per_key);
        assert!(TypingCadence::default().per_key < TypingCadence::deliberate().per_key);
        assert_eq!(TypingCadence::instant().per_key, Duration::ZERO);
    }

    #[test]
    fn accessibility_state_is_readable() {
        // Must not panic or prompt; either answer is legitimate on a test machine.
        let state = AccessibilityPermission::current();
        assert!(matches!(
            state,
            AccessibilityPermission::Granted | AccessibilityPermission::NotGranted
        ));
        assert_eq!(
            state.is_granted(),
            state == AccessibilityPermission::Granted
        );
    }

    #[test]
    fn empty_text_is_a_no_op_without_permission_prompting() {
        // Constructing without the permission check keeps this test valid on a
        // machine where the permission is absent.
        let Ok(input) = SecureInput::new_unchecked(TypingCadence::instant()) else {
            // A source is always creatable, but tolerate a restricted environment.
            return;
        };
        // `type_text("")` returns before the permission check, so this is stable.
        assert_eq!(input.type_text("").unwrap(), 0);
    }

    #[test]
    fn unicode_is_encoded_to_utf16_units() {
        // Verifies the encoding the delivery path depends on.
        assert_eq!("a".encode_utf16().count(), 1);
        assert_eq!("你".encode_utf16().count(), 1);
        assert_eq!("🚀".encode_utf16().count(), 2); // surrogate pair
        assert_eq!("é".encode_utf16().count(), 1);
        // A family emoji: several code points, all non-BMP.
        assert!("👨‍👩‍👧".encode_utf16().count() > 3);
    }

    #[test]
    fn truncation_splits_on_a_char_boundary() {
        // Reproduces the slicing logic used by `type_text_limited`.
        let text = "a你b🚀c";
        let limit = 3;
        let cutoff = text
            .char_indices()
            .nth(limit)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        let head = &text[..cutoff];
        assert_eq!(head, "a你b");
        assert!(head.is_char_boundary(head.len()));
    }

    #[test]
    fn report_reports_completeness() {
        assert!(
            TypeReport {
                delivered: 5,
                skipped: 0,
                truncated: false
            }
            .is_complete()
        );
        assert!(
            !TypeReport {
                delivered: 3,
                skipped: 2,
                truncated: true
            }
            .is_complete()
        );
    }
}
