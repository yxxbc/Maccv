# Security standards

This document is the contract MacCV's security claims are held to. It exists because
those claims are easy to state loosely and hard to verify, and because a privacy
claim a user cannot check is worse than no claim at all.

Every statement here is either enforced by code with a test, or explicitly marked as
**not enforced** with the reason. Where macOS imposes a hard limit, that limit is
recorded rather than worked around.

---

## S1. Core invariants

### S1.1 No data leaves the machine

**Enforced.** MacCV performs no network I/O except update checks.

- The only network calls in the codebase are in `crates/maccv-core/src/updater.rs`,
  and all of them go through `curl_to_string` / `curl_to_file`.
- Those two functions are the sole call sites of `/usr/bin/curl`. Verified by
  `Scripts/checks/no_undeclared_network.py`, which greps for process spawns and
  fails on any binary other than the allowlisted ones.
- Clipboard content is never passed to any of those functions. The update path
  handles only URLs, digests and version strings that came from the feed.

**Allowed network destinations**, exhaustively:

| Destination | Purpose | When |
|---|---|---|
| `yxxbc.github.io` | release feeds | update check only |
| `github.com` | release assets | update download only |
| `gh-proxy.com` | mirror of the above | only after the canonical host failed |

An update check happens when the user asks, or on a policy the user chose. The
`manual` policy performs no background check at all.

### S1.2 No data leaves the app

**Partly enforced; the limit is documented rather than hidden.**

Clipboard content reaches exactly one shared channel on macOS: **the system
pasteboard**. It is world-readable by design. There is no API to write to it
privately, and this is a platform limit rather than an implementation choice:

- `NSPasteboardContentsOptionsCurrentHostOnly` restricts *device* synchronisation,
  not other applications.
- AppKit has no `concealed` or `transient` type. `org.nspasteboard.ConcealedType`
  is a third-party convention honoured only voluntarily.
- `NSPasteboardAccessBehavior` (macOS 15.4+) describes whether **this process** may
  read. It cannot restrict anyone else.

Therefore:

- **History** is held in memory only and never written to disk, so other
  applications cannot read the log. Nothing in `maccv-core` opens a file for
  clipboard content.
- **A paste** writes one entry to the pasteboard and leaves it there, exactly like
  ⌘C in any application.
- **Paste Without Clipboard** (`secure_input.rs`) is the only mechanism that keeps
  content out of the shared channel: it delivers the text as synthetic keystrokes
  addressed to the frontmost application, so no other process can observe it. This
  is a real guarantee and it is the reason the feature exists.

**Not enforced:** the content of a clipboard write is visible to other applications.
Nothing can change that. Users who need the guarantee must use Paste Without
Clipboard.

### S1.3 AI services never read user data

**Enforced by absence.** MacCV contains no AI, ML, analytics, telemetry or crash
reporting of any kind.

- No model inference, local or remote.
- No SDK for any such service in `Cargo.toml` or `project.yml`.
- No usage statistics, no identifier, no ping.

Verified by `Scripts/checks/no_undeclared_network.py`, which fails if a dependency
list introduces a known analytics or AI host, and by the network allowlist in S1.1.

This must remain true. It is recorded here so a future contributor adding "just
crash reporting" knows the answer is no, and why.

### S1.4 Clipboard content is never persisted

**Enforced by construction.** History lives in `ClipboardStore.history`, a Swift
array. There is no serialisation of it anywhere.

- No `write(to:)`, no `FileManager` write, no `UserDefaults` entry for content.
- `AppSettings` persists preferences only — booleans, an integer, a language code.
  `Scripts/checks/no_content_persistence.py` asserts that the settings type has no
  field that could hold clipboard content.

When on-disk history arrives, it goes in the app container, and this section gets a
new subsection describing the format, the retention rule and the clearing
behaviour, rather than being quietly relaxed.

---

## S2. Plugin security

> **Status: design phase. No plugin system exists yet, and this section is not a
> description of shipped behaviour.**

The requirement is that a plugin cannot break the host's security. That requirement
constrains the design, so the design is recorded before any code.

### S2.1 Why a plain native library cannot meet the requirement

A plugin loaded as a dynamic library into the app's process is **indistinguishable
from the app itself** to the operating system. It can read the clipboard history,
open a socket, read the Keychain, or crash the host. "Plugins cannot break
security" is not achievable this way — it would be a promise with nothing behind
it, and this document does not record promises.

### S2.2 The intended design

Native code, isolated in an XPC service:

| Aspect | Choice | Why |
|---|---|---|
| Language | Swift or Rust, compiled to a dylib | Performance: this is the requirement that ruled out WASM |
| Execution | loaded into a separate XPC service process | a plugin crash kills the helper, not the app |
| Data access | host passes only what the plugin's manifest declares | capability model, not ambient authority |
| Network | helper has no entitlement for it | enforced by the sandbox, not by convention |
| Filesystem | helper has no writable container | same |
| Time and memory | host enforces a per-invocation budget | a plugin cannot wedge the app |

The capability model is the load-bearing part. A plugin declares what it needs:

```toml
# plugin manifest
[capabilities]
read_clipboard_item = true   # the current entry only
write_clipboard      = false
network              = false  # unavailable regardless; declared for clarity
```

The host grants what is declared and nothing else. A plugin that declares nothing
gets nothing.

### S2.3 What is still not guaranteed

Stated plainly so the eventual documentation does not overclaim:

- **The helper can read what it is given.** A capability to read one clipboard
  entry means that entry is in the helper's memory. A malicious plugin can exfiltrate
  it *if* it also has network access — which the sandbox denies, and that denial is
  the actual control.
- **A plugin cannot be prevented from being slow within its budget.** It can waste
  the budget; it cannot exceed it.
- **Signing is not yet specified.** Without it, installing a plugin is trusting its
  author.
- **XPC isolation is not a security boundary against a kernel exploit.** It is a
  boundary against a plugin doing something the user did not intend.

### S2.4 Review checklist for a plugin API change

Any change that widens what a plugin can reach must answer all of these in the pull
request:

- [ ] Which capability does this expose, and is it opt-in per plugin?
- [ ] What is the smallest data that satisfies it? (Not "the snapshot" when "the
      preview string" would do.)
- [ ] Does the helper gain an entitlement? If yes, this needs review by someone
      other than the author.
- [ ] What happens when the plugin misbehaves — crash, hang, huge allocation?
- [ ] Is there a test that a plugin *without* the capability cannot reach the data?

---

## S3. Update integrity

**Enforced.** An update is installed only after, in this order:

1. **SHA-256** of the download matches the feed. A mismatch deletes the file.
2. **Code signature** validates under `spctl --assess --type execute`.
3. **Only then** is `com.apple.quarantine` cleared.

The order is the control. Clearing the attribute before verification would defeat
it, since the attribute is the one thing distinguishing "the user downloaded this"
from "something placed this here".

**Deliberate limits:**

- The feed is fetched over HTTPS from the project's own host. A hash proves the
  bytes match what the feed promised; it does not prove the feed was honest. The
  mirror is therefore tried **second**, never first.
- The mirror is used only for GitHub hosts. A feed pointing anywhere else is never
  mirrored, so its traffic cannot be routed through a third party.
- MacCV does not replace its own running bundle. A verified, de-quarantined bundle is
  revealed in Finder and the user completes the swap.
- **Not notarized yet.** `spctl` will accept a Developer ID signature but Gatekeeper
  still prompts on first launch. Recorded in the roadmap.

### S3.1 Revocation

A released version can be withdrawn by adding it to the feed's `revoked` array.
When the running version appears there, `UpdateStatus::Revoked` is returned,
`maccv_update_result_is_forced` is true, and the app shows a window that cannot be
dismissed except by updating or quitting.

It does not terminate the process — a user with unsaved work elsewhere should be
able to finish — and it does not update silently, because being forced to update
with no explanation is indistinguishable from malware. See
[RELEASING.md](RELEASING.md) for the emergency procedure.

---

## S4. Input handling

### S4.1 The C ABI never traps

**Enforced.** Every entry point validates its pointers and returns a status code.
A panic across the FFI boundary would be undefined behaviour, and release builds use
`panic = "abort"`, so this is a crash-prevention requirement rather than a style
preference.

- Null pointers return `MACCV_ERROR_NULL_POINTER`, never dereference.
- Invalid UTF-8 returns `MACCV_ERROR_INVALID_UTF8`.
- `tests::null_pointers_are_reported_not_trapped` covers each entry point.

### S4.2 Clipboard content is untrusted input

**Enforced.** Anything on the pasteboard was written by another process and may be
hostile.

- Type identifiers and payloads are length-tagged, never NUL-terminated, so an
  embedded NUL cannot truncate a value.
- Strings handed to C have interior NULs replaced rather than being dropped.
- Every detector in `redaction.rs` walks characters, not bytes, so multi-byte text
  cannot cause a slice to panic. Four separate bugs of exactly this kind were found
  by the test suite before release.
- The single-free contract is tested: a snapshot with empty optional fields releases
  without touching static memory. A `const`-based sentinel previously made that
  comparison fail and crashed on every failed update check.

### S4.3 Synthetic input requires consent

**Enforced.** `CGEventPost` to the HID tap requires the Accessibility permission.
Without it the engine returns `AccessDenied` rather than appearing to succeed, and the
UI disables the feature instead of offering an action that silently does nothing.

---

## S5. What is deliberately not done

Recorded so these are decisions rather than oversights.

| Not done | Why |
|---|---|
| Storing history on disk (yet) | Removes a whole class of exposure until the format and retention rule are designed |
| Sandboxing the app | The app must read the pasteboard; the sandbox would not change what it can see |
| `NSVisualEffectView` behind content | Visual only, but it also renders clipboard content into a composited layer |
| Any analytics | S1.3 |
| Auto-replacing the running bundle | A half-completed swap is worse than a manual drag |
| Silent forced updates | S3.1 |
| Claiming redaction is a security boundary | It is a display and storage measure; text detection is inherently incomplete |

---

## Verification

These invariants are checked by:

```bash
Scripts/verify.sh                      # everything
python3 Scripts/checks/no_undeclared_network.py
python3 Scripts/checks/no_content_persistence.py
cargo test -p maccv-core               # redaction, updater, pasteboard
cargo test -p maccv-ffi                # pointer validation, single-free
```

A change that weakens any invariant in this document requires updating the document
in the same pull request, and saying why.
