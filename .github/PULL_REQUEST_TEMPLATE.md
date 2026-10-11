## What changed

<!-- One or two sentences. What behaviour is different now? -->

## Why

<!-- The problem this solves. Link the issue if there is one. -->

Closes #

## How to verify

<!--
  Give the reviewer a way to see this working, not just a description of the code.
  A command, a reproduction, or a test name all work.
-->

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] The Xcode build passes
- [ ] New engine behaviour has a test
- [ ] `CHANGELOG.md` updated under `[Unreleased]` (for user-visible changes)

## Notes for the reviewer

<!--
  Anything worth flagging: a trade-off you made, a follow-up you deliberately
  left out, or a part of the diff you would like scrutinised.
-->
