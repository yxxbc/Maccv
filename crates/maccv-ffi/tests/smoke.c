/*
 * C smoke test for the MacCV C ABI.
 *
 * Links against the release static library and exercises the full
 * read -> inspect -> write -> replay -> watch cycle on a process-private
 * pasteboard, so running it never disturbs a real clipboard.
 */

#include <assert.h>
#include <stdbool.h>
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

    printf("\nALL C CHECKS PASSED\n");
    return 0;
}
