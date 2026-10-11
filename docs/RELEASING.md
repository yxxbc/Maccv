# Releasing

This document is the release procedure. It is written down because the two things
that have gone wrong in projects like this are both procedural: a version that
disagrees with itself, and an emergency handled badly under time pressure.

For the security properties of the update path, see [SECURITY.md](SECURITY.md) §S3.

---

## The channel model

Two channels, two feed documents. This is the mechanism that keeps a test build away
from ordinary users.

| | Stable | Preview |
|---|---|---|
| Tag | `v0.2.0` | `v0.2.0-rc.1` |
| GitHub | normal release | pre-release |
| Feed | `docs/latest.json` | `docs/preview.json` |
| Who gets it | everyone | only users who enable pre-releases |
| Purpose | what users run | testing something before users see it |

A hyphen in the version is what selects the channel — there is no separate switch to
forget. A preview build goes into a **different document**, not behind a flag in one
document, because a flag is one bug away from reaching everyone.

**The rule:** a public release must be a version that has already run as a preview
for a reasonable period. The only exception is the emergency path below.

---

## Normal release

### 1. Prepare

```bash
Scripts/bump-version.sh --bump minor      # or an explicit 0.2.0, or --bump patch
```

This updates `Cargo.toml`, `apps/mac/project.yml` (both the marketing version and
the generated `CFBundleShortVersionString`), rolls the build number, and opens a
`[Unreleased]` → `[0.2.0]` section in `CHANGELOG.md`. It then **verifies** that all
records agree and fails if they do not, because a version that disagrees with itself
only shows up after publishing, when auto-update has already stopped working for
everyone who installed.

`--dry-run` shows what would change.

### 2. Write the notes

Move the user-visible entries from `[Unreleased]` into the new version's section.
These notes become the release body, the update feed's `notes` field, and the
website — all three read `CHANGELOG.md`, so they cannot disagree.

Write for a user, not for the diff:

- ❌ `refactor(pasteboard): extract classify()`
- ✅ **Masking now applies when an entry is recorded, not just when it is drawn.**

### 3. Verify

```bash
Scripts/verify.sh
```

All 17 stages must pass. This is the same command CI runs, so there is no gap
between "works locally" and "works in CI".

The DMG is checked by mounting it read-only and running the app inside it, which is
how a user first launches it.

### 4. Commit and tag

```bash
git commit -am 'chore(release): 0.2.0'
git push origin main
git tag -a v0.2.0 -m 'MacCV 0.2.0'
git push origin v0.2.0
```

The tag is the only trigger. Nothing publishes from a branch, so what is released is
exactly what was tagged and verified.

### 5. What the workflow does

1. Refuses if `CHANGELOG.md` has no section for the version — a release with no
   notes is a release nobody can read, and it fails *before* the expensive build.
2. Refuses if any manifest disagrees with the tag.
3. Builds one app **per architecture**, pinned via `ARCHS` because the destination
   alone can narrow it to the host.
4. Packages each as its own DMG, with the generated background and icon layout.
5. Verifies each image by mounting it and running the app inside.
6. Writes `SHA256SUMS` and a per-file `.sha256`.
7. Publishes the release with a download table naming each architecture.
8. Writes the channel's feed into `docs/`, then deploys the site.

Two DMGs rather than one universal: a universal image is roughly twice the size for
every user, with the dead slice on both kinds of Mac. The feed tells each machine
which to fetch — without that, an Intel user would download a build that will not
launch.

---

## Preview release

Identical, with a hyphenated version:

```bash
Scripts/bump-version.sh 0.2.0-rc.1
git commit -am 'chore(release): 0.2.0-rc.1' && git push
git tag -a v0.2.0-rc.1 -m 'MacCV 0.2.0-rc.1' && git push origin v0.2.0-rc.1
```

The workflow detects the hyphen, marks the GitHub release as a pre-release, and
writes `docs/preview.json`. Users on the stable channel never see it.

Promoting a preview is not a build step: cut a new version without the suffix and
the stable feed points at it.

### Testing the update path

The path most worth testing is "does an old version actually update", which requires
two published versions:

1. Publish a stable release, install it on a spare machine or account.
2. Publish the next version as a preview.
3. On the machine running the old stable build, enable pre-releases in Settings.
4. Check for updates, download, and confirm the DMG mounts and the app launches.

Until this has been done at least once, the update path is **tested in pieces but
never end to end** — which is recorded in [TESTING.md](TESTING.md) §T3 rather than
assumed to work.

---

## Emergency: a bad public release

The situation: a stable version is in the wild and is wrong — destroying history,
corrupting data, or failing to launch on a macOS update. Ordinary users have it.

### Decide first

Ask whether this can wait for a routine release. A forced update interrupts every
user's work and is indistinguishable from malware if it is not explained well. It is
justified when the running version is actively harmful, not merely imperfect.

If it can wait — fix it in the next routine release, with a changelog entry.

### Fix and release

1. Fix the defect on `main`.
2. `Scripts/bump-version.sh --bump patch` and write the changelog entry. Say what
   was wrong plainly; users who were affected need to recognise it.
3. `Scripts/verify.sh`, then tag and push as usual.
4. Wait for the release to publish — the revocation step below points at an
   artifact that must exist.

### Revoke

The feed's `revoked` array is a denylist:

```json
{
  "version": "0.2.1",
  "revoked": ["0.2.0"],
  "downloads": [ … ]
}
```

Add the bad version and push. The array **accumulates** — `write_feed.py` carries it
forward on every release, so it never needs re-adding and cannot be accidentally
cleared.

What a revoked version does:

- `UpdateStatus::Revoked` is returned, and `maccv_update_result_is_forced` is true.
- The app shows a window with no close button and no Escape handling. The only exits
  are **Update Now** and **Quit MacCV**.
- If the update cannot be fetched, the window says why and links to the releases
  page, so a user is never left with a blocked app and no route forward.

What it deliberately does not do:

- **Terminate the process.** A user with unsaved work elsewhere should finish.
- **Update silently.** Being forced to update with no explanation is what malware
  does.
- **Ask twice.** There is one decision to make.

### Optionally withdraw the artifact

Deleting the bad release's assets stops new installations of it. It does not help
existing users — only revocation does — and it breaks anyone who genuinely needs
that specific version. A judgement call, not a default.

### Afterwards

Add a `### Security` or `### Fixed` entry naming the cause, and consider whether a
test would have caught it. If it would have, add that test in the same cycle.

---

## Adding a revocation to the checklist

Because revocation is a promise that can be broken by inaction, these are checked
before treating a release as done:

- [ ] `docs/latest.json` is reachable and its `sha256` matches the published artifact
- [ ] Both DMGs download and mount
- [ ] The app inside each DMG passes `--self-test`
- [ ] An installed older version offers the update
- [ ] `revoked` is empty, or contains only versions that should be

---

## Rollback

There is no "unpublish". Once a version is on the feed, users have it.

- **A bad *feed*** — a wrong digest or URL — is fixed by pushing a corrected feed.
  Users whose check failed simply failed; nothing was installed.
- **A bad *release*** — the artifact itself is wrong — is handled by revocation
  above. Deleting the release does not undo installations.
- **A leak of signing material** is a different problem: rotate the key, publish a
  new version, and revoke every version signed with the old one.

This is why the release procedure verifies before publishing rather than after.
