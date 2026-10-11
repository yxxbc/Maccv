//! Live tests against a real `NSPasteboard`.
//!
//! These use a **process-private** pasteboard (`NSPasteboard.pasteboardWithUniqueName`)
//! rather than the general one, so running the suite never clobbers the
//! developer's own clipboard.

use std::time::{Duration, Instant};

use maccv_core::{Content, ContentKind, FileList, ImageData, Pasteboard, RichText, WellKnownType};

/// A 1×1 red PNG. Hand-rolled so the test suite has no binary fixtures.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn scratch() -> Pasteboard {
    Pasteboard::unique()
}

#[test]
fn writes_and_reads_plain_text() {
    let pb = scratch();
    pb.set_string("hello 世界 🚀").expect("write");

    let snapshot = pb.snapshot().expect("read");
    assert_eq!(snapshot.len(), 1);
    assert!(!snapshot.is_empty());
    assert_eq!(snapshot.kind(), Some(ContentKind::Text));

    match &snapshot.items[0].content {
        Content::Text(text) => assert_eq!(text.text, "hello 世界 🚀"),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(pb.get_string().as_deref(), Some("hello 世界 🚀"));
    assert_eq!(snapshot.preview(5), "hello…");
}

#[test]
fn write_increments_change_count() {
    let pb = scratch();
    let before = pb.change_count();
    let after = pb.set_string("x").expect("write");
    assert!(after > before, "{after:?} should exceed {before:?}");
    assert_eq!(pb.change_count(), after);

    let delta = after.delta_since(before);
    assert!(delta >= 1, "expected at least one increment, got {delta}");
}

#[test]
fn records_self_writes_so_watchers_can_skip_them() {
    let pb = scratch();
    assert_eq!(pb.last_self_write(), None);
    let written = pb.set_string("mine").expect("write");
    assert_eq!(pb.last_self_write(), Some(written));
    assert!(pb.is_self_write(written));
    // A change count from a *different* write is not ours.
    pb.set_string("theirs").expect("write");
    assert!(!pb.is_self_write(written));
}

#[test]
fn round_trips_a_rich_item_byte_for_byte() {
    let pb = scratch();
    let rich = RichText {
        plain: Some("bold 粗体".into()),
        rtf: Some(b"{\\rtf1\\ansi bold}".to_vec()),
        html: Some("<b>bold</b>".into()),
        rtfd: None,
    };
    pb.set_rich_text(&rich).expect("write");

    let snapshot = pb.snapshot().expect("read");
    let item = &snapshot.items[0];

    // Every representation we wrote must come back with the same bytes.
    let rtf = item
        .get(WellKnownType::Rtf)
        .expect("rtf representation present");
    assert_eq!(rtf.data, b"{\\rtf1\\ansi bold}");
    assert_eq!(
        std::str::from_utf8(&item.get(WellKnownType::Html).unwrap().data).unwrap(),
        "<b>bold</b>"
    );

    match &item.content {
        Content::RichText(back) => {
            assert_eq!(back.plain.as_deref(), Some("bold 粗体"));
            assert_eq!(back.rtf.as_deref(), Some(&b"{\\rtf1\\ansi bold}"[..]));
            assert_eq!(back.html.as_deref(), Some("<b>bold</b>"));
        }
        other => panic!("expected rich text, got {other:?}"),
    }

    // Replaying the snapshot reproduces the same payloads.
    pb.write_snapshot(&snapshot).expect("replay");
    let again = pb.snapshot().expect("read again");
    assert_eq!(
        again.items[0].get(WellKnownType::Rtf).unwrap().data,
        b"{\\rtf1\\ansi bold}"
    );
}

#[test]
fn round_trips_an_image_and_resolves_dimensions_from_the_png_header() {
    let pb = scratch();
    pb.set_image(&ImageData {
        png: Some(PNG_1X1.to_vec()),
        ..Default::default()
    })
    .expect("write");

    let snapshot = pb.snapshot().expect("read");
    match &snapshot.items[0].content {
        Content::Image(image) => {
            assert_eq!(image.png.as_deref(), Some(PNG_1X1));
            assert_eq!(image.pixel_width, Some(1));
            assert_eq!(image.pixel_height, Some(1));
            assert_eq!(image.preferred().map(|(uti, _)| uti), Some("public.png"));
        }
        other => panic!("expected image, got {other:?}"),
    }
    assert_eq!(snapshot.kind(), Some(ContentKind::Image));
}

#[test]
fn writes_files_as_file_urls() {
    let pb = scratch();
    let files = FileList {
        paths: vec!["/tmp".into(), "/usr".into()],
    };
    pb.set_files(&files).expect("write");

    let snapshot = pb.snapshot().expect("read");

    // A pasteboard item can hold at most one entry per type, so a multi-file
    // copy is necessarily one item per file — exactly how Finder does it.
    assert_eq!(snapshot.len(), 2);
    for item in &snapshot.items {
        assert_eq!(item.content.kind(), ContentKind::Files);
    }

    // The flat view pulls every file out of the item list.
    assert_eq!(
        snapshot.files(),
        vec![
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/usr")
        ]
    );
}

#[test]
fn writes_multiple_items_and_keeps_them_separate() {
    let pb = scratch();
    let items: Vec<Vec<(maccv_core::PasteboardType, Vec<u8>)>> = (1..=3)
        .map(|n| {
            vec![(
                WellKnownType::String.into(),
                format!("item-{n}").into_bytes(),
            )]
        })
        .collect();
    pb.write_items(items).expect("write");

    let snapshot = pb.snapshot().expect("read");
    assert_eq!(snapshot.len(), 3);
    let texts: Vec<_> = snapshot
        .items
        .iter()
        .map(|i| i.plain_text().unwrap().to_owned())
        .collect();
    assert_eq!(texts, ["item-1", "item-2", "item-3"]);
}

#[test]
fn rejects_empty_writes_instead_of_corrupting_the_pasteboard() {
    let pb = scratch();
    assert!(pb.set_files(&FileList::default()).is_err());
    assert!(pb.write_raw(&[]).is_err());
    assert!(
        pb.write_snapshot(&maccv_core::Snapshot {
            change_count: pb.change_count(),
            items: Vec::new(),
        })
        .is_err()
    );
}

#[test]
fn clear_empties_the_pasteboard() {
    let pb = scratch();
    pb.set_string("something").expect("write");
    assert!(!pb.is_empty());
    pb.clear();
    assert!(pb.is_empty());
    assert_eq!(pb.snapshot().expect("read").len(), 0);
}

#[test]
fn reports_supported_types_without_materialising_payloads() {
    let pb = scratch();
    pb.set_rich_text(&RichText {
        plain: Some("x".into()),
        html: Some("<i>x</i>".into()),
        ..Default::default()
    })
    .expect("write");

    assert!(pb.can_read(WellKnownType::String));
    assert!(pb.can_read(WellKnownType::Html));
    assert!(!pb.can_read(WellKnownType::Png));
    assert!(pb.types().iter().any(|t| t.as_str() == "public.html"));
}

#[test]
fn snapshot_survives_a_reopen_of_the_same_pasteboard() {
    let pb = scratch();
    pb.set_string("persisted").expect("write");

    let reopened = Pasteboard::by_name(&pb.name());
    assert_eq!(reopened.get_string().as_deref(), Some("persisted"));
}

#[test]
fn change_count_is_cheap_enough_to_poll() {
    let pb = scratch();
    // 10k reads; even a pessimistic interpretation must stay well under a
    // millisecond each, or polling would be the wrong strategy.
    let elapsed = pb.benchmark_change_count(10_000);
    let per_read = elapsed / 10_000;
    assert!(
        per_read < Duration::from_micros(50),
        "changeCount averaged {per_read:?}, too slow to poll"
    );
}

#[test]
fn watcher_notices_its_own_writes_only_when_asked() {
    let pb = scratch();
    let min = Duration::from_micros(500);
    let max = Duration::from_millis(2);

    // A raw watcher reports everything, including our own write.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&seen);
    let handle = pb.raw_watch(min, move |count| {
        sink.lock().unwrap().push(count);
    });
    std::thread::sleep(Duration::from_millis(20));
    let written = pb.set_string("watched").expect("write");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && !seen.lock().unwrap().contains(&written) {
        std::thread::sleep(Duration::from_millis(5));
    }
    handle.join();
    assert!(
        seen.lock().unwrap().contains(&written),
        "raw watcher missed its own write {written:?}"
    );

    // A filtered watcher must stay silent about it (max interval keeps it from
    // spinning, so this is a short bounded wait).
    let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&fired);
    let handle = pb.watch_adaptive(min, max, move |_| {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    pb.set_string("also mine").expect("write");
    std::thread::sleep(Duration::from_millis(50));
    handle.join();
    assert!(
        !fired.load(std::sync::atomic::Ordering::SeqCst),
        "filtered watcher reported this handle's own write"
    );
}
