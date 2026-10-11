/*
 * C smoke test for the MacCV C ABI.
 *
 * Links against the release static library and exercises the full
 * read -> inspect -> write -> replay -> watch cycle on a process-private
 * pasteboard, so running it never disturbs a real clipboard.
 */

#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "maccv.h"

static void fail(const char *what) {
    fprintf(stderr, "FAIL: %s\n", what);
    exit(1);
}

static void expect_ok(enum MaccvError err, const char *what) {
    if (err != MACCV_ERROR_OK) {
        struct MaccvString detail = maccv_last_error_message();
        fprintf(stderr, "FAIL: %s -> %d (%.*s)\n", what, (int)err, (int)detail.len, detail.ptr);
        exit(1);
    }
}

/* Copies an MaccvString into a NUL-terminated buffer for printf. */
static void copy_out(struct MaccvString s, char *out, size_t cap) {
    size_t n = s.len < cap - 1 ? s.len : cap - 1;
    memcpy(out, s.ptr, n);
    out[n] = '\0';
}

static int watch_hits = 0;

static void on_change(int64_t change_count, void *context) {
    (void)change_count;
    (void)context;
    watch_hits++;
}

int main(void) {
    char buf[512];

    /* --- version + static strings ------------------------------------- */
    struct MaccvString version = maccv_version();
    copy_out(version, buf, sizeof buf);
    printf("version            = %s\n", buf);
    if (version.len == 0) fail("version empty");

    struct MaccvString denied = maccv_error_message(MACCV_ERROR_ACCESS_DENIED);
    copy_out(denied, buf, sizeof buf);
    printf("access denied msg  = %.60s...\n", buf);

    /* --- null-safety --------------------------------------------------- */
    if (maccv_pasteboard_change_count(NULL) != INT64_MIN) fail("null pasteboard not guarded");
    if (maccv_pasteboard_clear(NULL) != MACCV_ERROR_NULL_POINTER) fail("null clear not guarded");
    printf("null guards        = ok\n");

    /* --- create + identity -------------------------------------------- */
    struct MaccvPasteboard *pb = maccv_pasteboard_unique();
    if (!pb) fail("could not create pasteboard");

    struct MaccvString name = maccv_pasteboard_name(pb);
    copy_out(name, buf, sizeof buf);
    printf("pasteboard name    = %s\n", buf);
    maccv_string_free(name);

    printf("access behavior    = %d\n", (int)maccv_pasteboard_access_behavior(pb));
    int64_t before = maccv_pasteboard_change_count(pb);

    /* --- write text ---------------------------------------------------- */
    const char *text = "hello from C 你好 🚀";
    expect_ok(maccv_pasteboard_write_string(pb, (const uint8_t *)text, strlen(text)),
              "write_string");

    int64_t after = maccv_pasteboard_change_count(pb);
    if (after <= before) fail("change count did not advance");
    printf("changeCount        = %lld -> %lld\n", (long long)before, (long long)after);

    if (maccv_pasteboard_change_count(pb) != after) fail("change count unstable");
    if (maccv_pasteboard_last_write_count(pb) != after) fail("last_write_count wrong");
    printf("last_write_count   = %lld (matches)\n", (long long)after);

    /* --- read it back -------------------------------------------------- */
    struct MaccvSnapshot *snap = NULL;
    expect_ok(maccv_pasteboard_snapshot(pb, &snap), "snapshot");
    if (!snap) fail("snapshot pointer null");
    if (snap->items_len != 1) fail("expected exactly one item");

    struct MaccvItem *item = (struct MaccvItem *)(void *)snap->items;
    copy_out(item->kind, buf, sizeof buf);
    printf("item kind          = %s\n", buf);
    if (strcmp(buf, "text") != 0) fail("item not classified as text");

    copy_out(item->preview, buf, sizeof buf);
    printf("item preview       = %s\n", buf);
    printf("item byte_len      = %zu\n", item->byte_len);

    struct MaccvRepresentation *reps =
        (struct MaccvRepresentation *)(void *)item->representations;
    if (item->representations_len < 1) fail("no representations");
    for (size_t i = 0; i < item->representations_len; i++) {
        copy_out(reps[i].type_identifier, buf, sizeof buf);
        printf("  repr[%zu] %-28s %zu B\n", i, buf, reps[i].data.len);
    }

    /* The payload must still be alive and exact — this is the pointer that a
       dangling reference would have corrupted. */
    bool found = false;
    for (size_t i = 0; i < item->representations_len; i++) {
        copy_out(reps[i].type_identifier, buf, sizeof buf);
        if (strcmp(buf, "public.utf8-plain-text") != 0) continue;
        if (reps[i].data.len != strlen(text)) fail("payload length mismatch");
        if (memcmp(reps[i].data.ptr, text, strlen(text)) != 0) fail("payload bytes mismatch");
        found = true;
    }
    if (!found) fail("plain text representation missing");
    printf("payload byte-exact  = yes\n");

    /* --- replay the snapshot ------------------------------------------- */
    expect_ok(maccv_pasteboard_write_snapshot(pb, snap), "write_snapshot");
    struct MaccvSnapshot *replayed = NULL;
    expect_ok(maccv_pasteboard_snapshot(pb, &replayed), "snapshot after replay");
    if (replayed->items_len != 1) fail("replay lost the item");
    maccv_snapshot_free(replayed);

    /* --- watcher ------------------------------------------------------- */
    /* skip_self_writes=false: we want to observe the write we are about to make. */
    struct MaccvWatch *watch = maccv_watch_start(pb, 500, 2000, false, on_change, NULL);
    if (!watch) fail("watch_start returned null");
    usleep(30 * 1000);
    expect_ok(maccv_pasteboard_write_string(pb, (const uint8_t *)"watched", 7), "watch write");
    usleep(60 * 1000);
    maccv_watch_stop(watch);
    printf("watcher fired      = %d time(s)\n", watch_hits);
    if (watch_hits < 1) fail("watcher never fired");

    /* --- cleanup ------------------------------------------------------- */
    maccv_snapshot_free(snap);
    expect_ok(maccv_pasteboard_clear(pb), "clear");
    maccv_pasteboard_free(pb);

    /* Double-free safety: freeing NULL must be a no-op. */
    maccv_snapshot_free(NULL);
    maccv_pasteboard_free(NULL);
    maccv_watch_stop(NULL);
    maccv_string_free((struct MaccvString){NULL, 0});

    /* The general pasteboard's own access behavior, reported honestly. */
    struct MaccvPasteboard *general = maccv_pasteboard_general();
    int behavior = (int)maccv_pasteboard_access_behavior(general);
    printf("general behavior   = %d (%s)\n", behavior,
           behavior == 0 ? "default" : behavior == 1 ? "ask" : behavior == 2 ? "always allow"
                                                           : "always deny");
    maccv_pasteboard_free(general);

    /* --- accessibility permission ------------------------------------- */
    enum MaccvAccessibilityPermission perm = maccv_secure_input_permission();
    printf("\nsecure input perm  = %s\n",
           perm == MACCV_ACCESSIBILITY_PERMISSION_GRANTED ? "granted" : "not granted");
    printf("recommended limit  = %zu chars\n", maccv_secure_input_recommended_limit());

    /* Typing must never silently succeed without the permission. */
    const char *typed = "hello from C";
    enum MaccvError type_result =
        maccv_secure_input_type((const uint8_t *)typed, strlen(typed), 2000);
    if (type_result == MACCV_ERROR_OK) {
        if (perm != MACCV_ACCESSIBILITY_PERMISSION_GRANTED) {
            fail("typing claimed success without the Accessibility permission");
        }
        printf("typing             = delivered\n");
    } else if (type_result == MACCV_ERROR_ACCESS_DENIED) {
        if (perm == MACCV_ACCESSIBILITY_PERMISSION_GRANTED) {
            fail("typing refused despite a granted permission");
        }
        printf("typing             = refused (permission absent, as expected)\n");
    } else {
        fail("typing returned an unexpected status");
    }

    /* --- redaction ----------------------------------------------------- */
    printf("\nredaction:\n");
    const char *secret = "card 4111 1111 1111 1111 and mail a@b.com";
    struct MaccvRedacted *redacted = NULL;
    expect_ok(maccv_redact((const uint8_t *)secret, strlen(secret), MACCV_REDACTION_MODE_FULL,
                           &redacted),
              "redact");
    if (!redacted) fail("redact returned a null result");
    if (!maccv_redacted_was_redacted(redacted)) fail("a card number was not detected");
    if (maccv_redacted_match_count(redacted) < 2) fail("expected two sensitive spans");

    struct MaccvString masked = maccv_redacted_text(redacted);
    copy_out(masked, buf, sizeof buf);
    printf("  masked           = %s\n", buf);
    if (strstr(buf, "4111") != NULL) fail("the card number leaked through redaction");
    if (strstr(buf, "a@b.com") != NULL) fail("the email leaked through redaction");
    if (strstr(buf, "card") == NULL) fail("ordinary text was lost");

    struct MaccvString kinds = maccv_redacted_kinds(redacted);
    copy_out(kinds, buf, sizeof buf);
    printf("  kinds            = %s\n", buf);
    if (strstr(buf, "card-number") == NULL) fail("card-number not reported");
    if (strstr(buf, "email") == NULL) fail("email not reported");

    /* A heuristic guess must never cause content to be dropped. */
    if (maccv_redacted_should_skip_recording(redacted)) {
        fail("a heuristic match must not skip recording");
    }
    maccv_redacted_free(redacted);

    /* Clean text round-trips unchanged. */
    const char *clean = "just an ordinary sentence";
    struct MaccvRedacted *untouched = NULL;
    expect_ok(maccv_redact((const uint8_t *)clean, strlen(clean), MACCV_REDACTION_MODE_FULL,
                           &untouched),
              "redact clean");
    if (maccv_redacted_was_redacted(untouched)) fail("clean text was reported as sensitive");
    struct MaccvString same = maccv_redacted_text(untouched);
    copy_out(same, buf, sizeof buf);
    if (strcmp(buf, clean) != 0) fail("clean text was modified");
    /* Freeing a result with an empty kind list must not crash. */
    maccv_redacted_free(untouched);

    if (maccv_redacted_text(NULL).len != 0) fail("null redacted accessor is not empty");
    if (maccv_redacted_match_count(NULL) != 0) fail("null redacted count is not zero");
    maccv_redacted_free(NULL);

    /* --- version comparison ------------------------------------------- */
    printf("\nversion compare:\n");
    printf("  0.10.0 vs 0.9.0  = %d (expect 1)\n",
           maccv_compare_versions("0.10.0", "0.9.0"));
    if (maccv_compare_versions("0.10.0", "0.9.0") != 1) fail("numeric ordering is wrong");
    if (maccv_compare_versions("v1.2.3", "1.2.3") != 0) fail("tag prefix not ignored");
    if (maccv_compare_versions("1.0.0-beta", "1.0.0") != -1) fail("prerelease ordering is wrong");

    /* --- quarantine ---------------------------------------------------- */
    printf("\nquarantine:\n");
    char staging_template[] = "/tmp/maccv-smoke-XXXXXX";
    char *staging_dir = mkdtemp(staging_template);
    if (!staging_dir) fail("could not create a staging directory");
    printf("  clean path       = %s\n",
           maccv_path_is_quarantined(staging_dir) ? "quarantined" : "clean");
    if (maccv_path_is_quarantined(staging_dir)) fail("a fresh temp dir is not quarantined");
    if (maccv_path_is_quarantined(NULL)) fail("null path reported as quarantined");

    /* --- update check (offline is a status, not an error) -------------- */
    printf("\nupdate check (deliberately unreachable feed):\n");
    struct MaccvUpdateResult *update = NULL;
    expect_ok(maccv_update_check("0.1.0", "https://127.0.0.1:9/nope.json",
                                 MACCV_UPDATE_CHANNEL_STABLE, &update),
              "update_check");
    if (!update) fail("update_check returned a null result");
    int update_status = (int)maccv_update_result_status(update);
    printf("  status           = %d (%s)\n", update_status,
           update_status == 0 ? "up to date" : update_status == 1 ? "available" : "check failed");
    if (update_status != 2) fail("an unreachable feed should report a failed check");
    struct MaccvString why = maccv_update_result_reason(update);
    copy_out(why, buf, sizeof buf);
    printf("  reason           = %s\n", buf);
    if (why.len == 0) fail("a failed check must explain itself");
    /* Every accessor must be safe on unset fields. */
    if (maccv_update_result_version(update).len != 0) fail("version should be unset");
    copy_out(maccv_update_result_notes(update), buf, sizeof buf);
    printf("  notes            = \"%s\"\n", buf);
    /* This free is the one that used to crash. */
    maccv_update_result_free(update);
    maccv_update_result_free(NULL);
    printf("  freed cleanly    = yes\n");

    /* --- updater download path ---------------------------------------- */
    struct MaccvStagedUpdate *staged = NULL;
    expect_ok(maccv_update_download("https://127.0.0.1:9/nope.zip",
                                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                                    "9.9.9", staging_dir, &staged),
              "update_download");
    if (!staged) fail("update_download returned a null result");
    printf("\ndownload (unreachable):\n");
    printf("  signature valid  = %s\n",
           maccv_staged_update_signature_valid(staged) ? "yes" : "no");
    printf("  dequarantined    = %s\n",
           maccv_staged_update_dequarantined(staged) ? "yes" : "no");
    if (maccv_staged_update_signature_valid(staged)) {
        fail("an unreachable download cannot have a valid signature");
    }
    if (maccv_staged_update_dequarantined(staged)) {
        fail("an unverified download must never be de-quarantined");
    }
    copy_out(maccv_staged_update_reason(staged), buf, sizeof buf);
    printf("  reason           = %s\n", buf);
    /* Unverified means nothing is claimed. */
    if (maccv_staged_update_app_path(staged).len != 0) fail("no bundle should be reported");
    maccv_staged_update_free(staged);
    maccv_staged_update_free(NULL);

    rmdir(staging_dir);
    maccv_owned_string_free((struct MaccvString){NULL, 0});

    printf("\nALL C CHECKS PASSED\n");
    return 0;
}
