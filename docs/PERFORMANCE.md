# Performance standards

MacCV's first promise is speed, which makes it falsifiable: either a clipboard read
is fast enough to poll or it is not. This document records the budget, how each
number is measured, and what happens when one regresses.

The rule that makes this more than a wish: **every published number is asserted by
a test**. A measurement taken once by hand is a measurement that is wrong within a
month.

---

## P1. The budgets

Measured on Apple silicon (M-series), release profile, `cargo test -p maccv-core`
on an idle machine.

| Operation | Budget | Current | Where asserted |
|---|---|---|---|
| Change detection (`changeCount`) | < 5 µs | ~0.53 µs | `pasteboard::tests::change_count_is_cheap_enough_to_poll` |
| Read plain text | < 100 µs | ~9.9 µs | release benchmark, below |
| Detect an external copy | < 2 ms | ~116 µs | `updater`-adjacent watcher test |
| Redaction of 1 MB of prose | < 500 ms | well under | `redaction::tests::detection_is_cheap_on_large_ordinary_text` |
| Release bundle | < 4 MB | ~1.9 MB | CI step summary |
| DMG | < 3 MB | ~1.5 MB | `Scripts/make-dmg.sh` |

The budgets are roughly an order of magnitude above what the code does. That gap is
deliberate: a budget set at the current number makes every unrelated change look
like a regression, and people stop trusting the check.

### Why 0.5 µs is the number that matters

It is what makes **polling** viable, and polling is what lets MacCV avoid an event
tap — and therefore avoid demanding the Accessibility permission merely to notice a
copy. If this number regresses past the budget, the architecture is wrong, not just
slow. Treat a failure here as a design problem.

---

## P2. Measurement rules

1. **Assert in a test, not in a document.** If a number appears above, a test
   produces it.
2. **Measure the real thing.** The pasteboard tests run against a live
   `NSPasteboard` (a process-private one). A mock would measure the mock.
3. **Use a generous ceiling.** Assert `per_read < 50 µs` when the code does 0.5 µs,
   so the check catches an order-of-magnitude regression rather than CI noise.
4. **Never assert a floor on speed.** "Faster than X" tests flake on a loaded
   runner.
5. **State the machine.** A number without hardware is not a number. These were
   measured on Apple silicon; an Intel Mac will differ, and the budgets account for
   that.

### What is explicitly not benchmarked

- **App launch time.** It is dominated by the Swift runtime and AppKit before any
  MacCV code runs. A test would measure the OS.
- **Memory footprint.** The engine holds no long-lived clipboard data, so the
  figure is SwiftUI's. Measuring it here would be misleading.
- **First-read latency after a cold start.** Includes filesystem cache effects that
  are not MacCV's.

Recording these as out of scope is the point: an unmeasured claim is a claim someone
will eventually overstate.

---

## P3. Where the cost actually is

Known and accepted, so nobody tries to optimize the wrong layer:

| Cost | Value | Why it cannot be removed |
|---|---|---|
| `writeObjects` | ~215 µs | AppKit notifies every interested process; this is the OS's cost, not the engine's |
| Type expansion on write | part of the above | macOS expands one `public.utf8-plain-text` into several types for compatibility |
| `NSImage` metadata read | only on images | Reading PNG dimensions from the IHDR header avoids it entirely; `NSImage` is the fallback for other formats |
| Redaction | linear in text length | Necessary, and the detectors short-circuit on ordinary text |

The read path is already at the floor: a `changeCount` read is a cached property
load, and a snapshot is one `memcpy` per representation.

---

## P4. Regression policy

When a performance test fails:

1. **Confirm it is real.** Re-run on an idle machine. A CI runner under load fails
   these; that is why the ceilings are an order of magnitude above the code.
2. **Find the cause before raising the budget.** Raising a budget to make a test
   pass is how a performance claim quietly becomes false.
3. **If the budget must rise**, the pull request states the old value, the new
   value, the cause, and why the trade is worth it. The table above and the README
   are updated in the same commit.

A change that makes the documented number worse without touching this document is a
failed review, not a judgement call.

---

## P5. Adding a performance claim

Before any number appears in the README, this document, or the website:

- [ ] There is a test that fails if it regresses.
- [ ] The test measures the real dependency, not a stand-in.
- [ ] The ceiling is loose enough not to flake, tight enough to catch an
      order-of-magnitude change.
- [ ] The hardware and profile are stated.
- [ ] If it is a range or an estimate, it says so rather than implying precision.

`Scripts/verify.sh` runs the tests that back these numbers, so a claim cannot be
published without passing.

---

## P6. Benchmark reference

The suite that produces the table:

```bash
cargo test -p maccv-core --release
```

Individual measurements, when investigating rather than checking:

```bash
# The whole pasteboard path, with timings printed
cargo test -p maccv-core --release -- --nocapture change_count

# What the updater would choose, from a real feed
cargo run -p maccv-core --example feed_check -- docs/latest.json
```
