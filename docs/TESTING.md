# Testing standards

MacCV makes claims that are easy to state and hard to substantiate: that it is fast,
that it is faithful, that it does not leak. This document records what is tested,
how, and — just as importantly — **what is not tested and why**.

The rule behind all of it: **a claim without a test is a claim that will eventually
be false**. Where a claim genuinely cannot be tested, it is marked as such rather
than left to look verified.

---

## T1. Layers, and what each one proves

| Layer | Location | Proves |
|---|---|---|
| Unit | `crates/maccv-core/src/**` `#[cfg(test)]` | pure logic: parsing, detection, ordering, encoding |
| Integration (live) | `crates/maccv-core/tests/` | the real `NSPasteboard` behaves as documented |
| C ABI | `crates/maccv-ffi/src/` `#[cfg(test)]` | pointer validation, single-free, no traps |
| C smoke | `crates/maccv-ffi/tests/smoke.c` | the ABI works from an actual C compiler |
| App self-test | `--self-test` | Swift → C ABI → Rust, end to end |
| Pipeline | `Scripts/checks/` | the security and consistency invariants hold |
| Release | `Scripts/verify.sh` | everything, in the order that fails fastest |

Each layer exists because the one below it cannot see the failure. A Rust unit test
cannot catch a stale generated header; a C smoke test cannot catch a Swift bridging
mistake; only the app self-test exercises the whole chain.

---

## T2. Non-negotiable rules

### T2.1 Test the real framework

Live pasteboard tests run against an actual `NSPasteboard`, because the failure
modes worth catching are AppKit's, not ours:

- `writeObjects:` requires `prepareForNewContents` first. A mock would not have
  reproduced that, and the bug where writes silently returned `false` would have
  shipped.
- Multi-file copies arrive as *n* separate items. A mock built from an assumption
  would have encoded the wrong assumption.

### T2.2 Never touch the developer's clipboard

Every live test uses `Pasteboard::unique()` — a process-private pasteboard — via the
`scratch()` helper. The app self-test does the same. So:

- A test run cannot destroy what the developer had copied.
- CI does not depend on what happens to be on the clipboard.
- The suite is safe to run at any moment, which is what makes people run it.

Any test that must use the general pasteboard is a design smell and needs a reason
recorded in the test.

### T2.3 Name tests as sentences describing the guarantee

Not `test_write` but:

```rust
fn writes_files_as_file_urls()
fn change_count_is_cheap_enough_to_poll()
fn unavailable_access_behavior_is_not_treated_as_permissive()
fn freeing_a_result_with_empty_optional_fields_does_not_crash()
```

The name is the specification. A test whose name cannot be read as a claim about
behaviour is usually testing the implementation rather than the contract.

### T2.4 Assert documented numbers

Performance claims in `docs/PERFORMANCE.md` are asserted, not measured by hand. T2.5
below explains why this matters more than it looks.

### T2.5 Reproduce a bug as a test before fixing it

Every non-trivial bug fixed in this project has a named regression test with a
comment explaining the original failure. The reason is not ceremony: the four
byte-walking panics in `redaction.rs` were the *same* mistake made four times, and
only a test per detector stopped it recurring.

---

## T3. What is not tested, and why

Recorded so these are decisions rather than gaps someone assumes are covered.

| Not tested | Why | What would change this |
|---|---|---|
| Notarization acceptance | Requires an Apple Developer account and a signed release | Onboarding into the Developer Program |
| Behaviour on macOS 15.0–15.3 specifically | No such machine available; the version gate is tested by probing the selector | A VM or CI image at 15.0 |
| Intel execution | No Intel Mac and no Rosetta on the dev machine. The x86_64 slice is verified structurally — `lipo`, `vtool`, and all 35 symbols resolve | A runner with Rosetta, or an Intel machine |
| Gatekeeper's first-launch prompt | Requires a downloaded-and-quarantined copy; would need a real user | An end-to-end install test on a quarantine-carrying download |
| Real UI interaction | No UI test target. Views are thin over tested models | A UI test target, if the views grow logic |
| Update against a live release | Would need a published older version to update *from* | A fixture release |
| Plugin sandbox | Not implemented | The plugin phase |

Every row is a real limit. None of them is a claim that has been quietly softened.

---

## T4. Test data rules

- **No binary fixtures.** The 1×1 PNG used across the suite is written out as bytes
  in the test file, so the repository carries no opaque blobs and a reader can see
  exactly what is being asserted.
- **No network in tests.** The one update-check test points at `127.0.0.1:9`, which
  cannot listen, and asserts a graceful `CheckFailed` rather than an error. Tests
  must pass with the network off.
- **No dependence on machine state.** No test reads the real clipboard, the user's
  home directory, or the clock beyond a monotonic duration.
- **Clean up.** Tests that create files remove them. `verify.sh` can be run twice
  with the same result.

---

## T5. Adding a test

For new engine behaviour:

1. **Live, if it touches AppKit.** Extend `crates/maccv-core/tests/clipboard.rs`,
   use `scratch()`, clean up.
2. **Unit, if it is pure.** Put it next to the code it tests.
3. **A sentence for a name.** If you cannot phrase it as a guarantee, work out what
   the guarantee actually is first.
4. **A comment for a regression.** Explain the original failure, so the test cannot
   be "simplified" away by someone who does not know why it exists.

For new C ABI surface:

1. Add a null-pointer test — the API must report, never trap.
2. Add a round-trip test proving the value survives.
3. Extend `smoke.c`, and run it, because that is the only check that compiles the
   header as C.

For new security-relevant behaviour:

1. Add a check under `Scripts/checks/` if it is a property of the source.
2. **Verify the check catches a violation.** A check that has never failed is not
   known to work. Each existing check was validated by deliberately introducing the
   violation it is meant to catch.

---

## T6. Running the suite

```bash
Scripts/verify.sh                # everything, 17 stages
Scripts/verify.sh --quick        # skip the app build
Scripts/verify.sh --no-network   # skip the one check that reaches out

cargo test --workspace           # Rust only
cargo test -p maccv-core --release   # timings without debug-build noise
```

Current totals: **79** core unit tests, **13** live pasteboard tests, **26** C ABI
tests, **1** doc test, plus the C smoke test and the app self-test.

### Why the suite is ordered as it is

Cheapest first, so a formatting error does not wait on a full Xcode build. Each
stage prints its own verdict and the script stops at the first failure, because a
list of four failures caused by one formatting error is noise.
