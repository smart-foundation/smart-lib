//! The migration queue as data: the invariants that make it trustworthy —
//! every domain the original shell hosted is present, the order is the
//! recon's, and the honest pending reason is generated from the same table.

use shell::migrate::{self, Domain, Status, QUEUE};

#[test]
fn every_domain_the_original_shell_hosted_is_in_the_queue() {
    // The original shell's 46 command modules (45 domains + network), each
    // present under its full grammar name.
    let original = [
        "accessibility", "app", "appindex", "audio", "auth", "backup", "buffer", "calendar",
        "camera", "capture", "clipboard", "compositor", "contacts", "database", "display",
        "extension", "files", "filesystem", "font", "input", "logging", "location", "media",
        "network", "notification", "package", "power", "print", "process", "search", "secret",
        "service", "session", "settings", "speech", "surface", "system", "textinput", "theme",
        "time", "translation", "update", "usb", "web", "window",
    ];
    for domain in original {
        assert!(
            QUEUE.iter().any(|d| d.name == domain),
            "{domain} is missing from the queue"
        );
    }
    // Plus the one domain the queue adds: the engine has a real body for it
    // and the original shell never hosted a command for it.
    assert!(QUEUE.iter().any(|d| d.name == "queue"));
}

#[test]
fn eighteen_domains_are_real_today() {
    let real: Vec<&Domain> = QUEUE.iter().filter(|d| d.status == Status::RealToday).collect();
    assert_eq!(
        real.len(),
        18,
        "network, clipboard, service, the batch-4 six (files, process, settings, logging, \
         clock, time), the batch-5 three (audio, media, power) and the batch-6 six \
         (database, queue, appindex, manifest, vcard, content)"
    );
    let names: Vec<&str> = real.iter().map(|d| d.name).collect();
    for domain in [
        "network", "clipboard", "service", "files", "process", "settings", "logging", "clock",
        "time", "audio", "media", "power", "database", "queue", "appindex", "manifest", "vcard",
        "content",
    ] {
        assert!(names.contains(&domain), "{domain} is real today on the engine");
    }
    // A landed domain names its engine half, not a salvage archive path, and
    // its count names the engine's own availability rows.
    for domain in &real {
        assert!(
            domain.archive.starts_with("engine:"),
            "{}: {}",
            domain.name,
            domain.archive
        );
        assert!(
            domain.count.to_lowercase().contains("real"),
            "{}: {}",
            domain.name,
            domain.count
        );
    }
}

#[test]
fn the_batch6_domains_are_on_the_board_with_their_own_counts() {
    for (domain, count) in [
        ("database", "27 REAL"),
        ("queue", "7 REAL"),
        ("appindex", "5 REAL"),
        ("manifest", "5 REAL"),
        ("vcard", "7 REAL"),
        ("content", "10 REAL"),
    ] {
        let row = migrate::find(domain).expect("the domain is on the board");
        assert_eq!(row.status, Status::RealToday, "{domain} is real today");
        assert!(
            row.count.contains(count),
            "{domain} names its engine's own counts: {}",
            row.count
        );
        assert!(row.order.is_none(), "{domain} left the queue");
    }
}

#[test]
fn the_queue_order_is_the_recons_value_order_past_the_landed_domains() {
    let ordered: Vec<u16> = QUEUE.iter().filter_map(|d| d.order).collect();
    let mut sorted = ordered.clone();
    sorted.sort_unstable();
    assert_eq!(ordered, sorted, "the positions are the order");
    assert_eq!(ordered, (1..=3).collect::<Vec<u16>>(), "three queued domains, contiguous");
}

#[test]
fn every_entry_names_its_archive_body() {
    for domain in QUEUE {
        assert!(
            !domain.archive.is_empty(),
            "{} has no archive note",
            domain.name
        );
        if domain.status != Status::RealToday {
            assert!(
                domain.archive.starts_with("platforms/linux/"),
                "{} points at the salvage archive: {}",
                domain.name,
                domain.archive
            );
        }
    }
}

#[test]
fn the_pending_reason_is_generated_from_the_table() {
    // A still-queued domain reads its pending reason from the archive note.
    let reason = migrate::pending_reason("window");
    assert!(reason.contains("migration pending"), "{reason}");
    assert!(reason.contains("platforms/linux/window.rs"), "{reason}");
    assert!(reason.contains("smart migrate"), "{reason}");
    // A domain that is real today is never told it is pending — honesty both ways.
    let clipboard = migrate::pending_reason("clipboard");
    assert!(clipboard.contains("real today"), "{clipboard}");
    let service = migrate::pending_reason("service");
    assert!(service.contains("real today"), "{service}");
    // The batch-6 six are real today too, and say so.
    for domain in ["database", "queue", "appindex", "manifest", "vcard", "content"] {
        let reason = migrate::pending_reason(domain);
        assert!(reason.contains("real today"), "{domain}: {reason}");
    }
    // A domain the queue never knew answers as unknown, never as a lie.
    let reason = migrate::pending_reason("telepathy");
    assert!(reason.contains("unknown domain"), "{reason}");
}

#[test]
fn the_queue_renders_its_order_and_counts() {
    let human = migrate::render_queue(&shell::OutputFormat::Human);
    assert!(human.contains("Migration Queue"));
    assert!(human.contains("clipboard"));
    assert!(human.contains("migrated-pending"));
    assert!(human.contains("archive-body-exists"));
    assert!(human.contains("real-today"));
    let json = migrate::render_queue(&shell::OutputFormat::Json);
    assert!(json.starts_with('[') && json.contains("\"name\":\"network\""));
}
