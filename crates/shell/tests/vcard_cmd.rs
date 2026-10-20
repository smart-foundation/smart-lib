//! The vcard command module over the scripted engine: the command surface's
//! data half, its rendering, the write gate on the mutating verbs, and the
//! `@vcard/*` selector tree. Nothing here touches the host (charter law #8).

#![cfg(feature = "domain-vcard")]

mod common;

use clap::Parser;
use common::batch6::vcard::MockVcard;
use shell::domains::vcard::{evaluate, selector_root, VcardCmd, WriteGuard};
use shell::selector::{self, resolve};
use shell::Value;

fn cmd(argv: &[&str]) -> VcardCmd {
    VcardCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// A card file the test writes and the command reads: a real RFC 6350 card.
fn card_file(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("smart-vcard-{}-{}", std::process::id(), name));
    std::fs::write(
        &path,
        "BEGIN:VCARD\nVERSION:4.0\nFN:Ada Lovelace\nN:Lovelace;Ada;;;\nEMAIL:ada@example.org\nEND:VCARD\n",
    )
    .expect("the card file");
    path
}

/// The default view is the store's contacts, as the engine's own contact model.
#[test]
fn the_default_view_is_the_stores_contacts() {
    let backend = MockVcard::seeded();
    let value = evaluate(&cmd(&["vcard"]), &backend, refused()).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("name"), "Ada Lovelace");
            assert_eq!(items[0].field_str("emails"), "ada@example.org");
        }
        other => panic!("expected a contact list, got {other:?}"),
    }
}

/// `versions` is the engine's own list of the versions it can read and write.
#[test]
fn versions_are_the_engines_own_list() {
    let backend = MockVcard::seeded();
    let value = evaluate(&cmd(&["vcard", "versions"]), &backend, refused()).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0].field_str("version"), "2.1");
        }
        other => panic!("expected a version list, got {other:?}"),
    }
}

/// `parse` reads a card without importing it: the properties the engine read,
/// as it read them.
#[test]
fn parse_reads_a_card_without_importing_it() {
    let backend = MockVcard::seeded();
    let path = card_file("parse");
    let value = evaluate(
        &cmd(&["vcard", "parse", path.to_str().unwrap()]),
        &backend,
        refused(),
    )
    .unwrap();
    match value {
        Value::List(cards) => {
            assert_eq!(cards.len(), 1);
            match cards[0].field("lines") {
                Value::List(lines) => {
                    assert!(lines
                        .iter()
                        .any(|line| line.field_str("line") == "FN"
                            && line.field_str("value") == "Ada Lovelace"));
                }
                other => panic!("expected the property lines, got {other:?}"),
            }
        }
        other => panic!("expected a card list, got {other:?}"),
    }
    // The parse did not import: the store is untouched.
    assert!(backend.writes.borrow().is_empty());
    let _ = std::fs::remove_file(path);
}

/// `import` is gate-routed: refused without an acceptance, and the refused
/// write reaches no engine.
#[test]
fn import_is_gated_and_reaches_the_store() {
    let backend = MockVcard::seeded();
    let path = card_file("import");
    let err = evaluate(
        &cmd(&["vcard", "import", path.to_str().unwrap()]),
        &backend,
        refused(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("CHANGES the engine's store"), "{err}");
    assert!(backend.writes.borrow().is_empty());

    let value = evaluate(
        &cmd(&["vcard", "import", path.to_str().unwrap()]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(value.field("imported").display(), "1");
    assert_eq!(backend.writes.borrow().len(), 1);
    let _ = std::fs::remove_file(path);
}

/// `export` writes the store's cards as vCard data, and `--out` is a HOST
/// write: it carries the gate like every other mutating verb.
#[test]
fn export_writes_the_stores_cards_and_the_out_file_is_gated() {
    let backend = MockVcard::seeded();
    let value = evaluate(&cmd(&["vcard", "export", "--all"]), &backend, refused()).unwrap();
    assert_eq!(value.field("exported").display(), "1");
    assert!(value.field_str("card").contains("BEGIN:VCARD"));

    let out = std::env::temp_dir().join(format!("smart-vcard-out-{}.vcf", std::process::id()));
    let err = evaluate(
        &cmd(&[
            "vcard",
            "export",
            "--all",
            "--out",
            out.to_str().unwrap(),
        ]),
        &backend,
        refused(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("CHANGES the engine's store"), "{err}");
    assert!(!out.exists(), "a refused export writes no file");

    let value = evaluate(
        &cmd(&[
            "vcard",
            "export",
            "--all",
            "--out",
            out.to_str().unwrap(),
        ]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(value.field_str("written_to"), out.to_str().unwrap());
    assert!(out.exists(), "an accepted export writes the file");
    let _ = std::fs::remove_file(out);
}

/// `new` formats a contact the caller spelled out — through the engine's own
/// formatter, and without storing anything.
#[test]
fn new_formats_a_contact_through_the_engines_formatter() {
    let backend = MockVcard::seeded();
    let value = evaluate(
        &cmd(&[
            "vcard",
            "new",
            "--name",
            "Grace Hopper",
            "--email",
            "grace@example.org",
        ]),
        &backend,
        refused(),
    )
    .unwrap();
    let card = value.field_str("card");
    assert!(card.contains("FN:Grace Hopper"), "{card}");
    assert!(card.contains("EMAIL:grace@example.org"), "{card}");
    // Nothing was stored: the card is formatted, not imported.
    assert!(backend.writes.borrow().is_empty());
}

/// A version this engine does not speak is refused, not guessed.
#[test]
fn an_unknown_version_is_refused() {
    let backend = MockVcard::seeded();
    let err = evaluate(
        &cmd(&["vcard", "export", "--all", "--version", "5.0"]),
        &backend,
        refused(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("5.0"), "{err}");
}

/// The sync token and the sync report are the engine's own.
#[test]
fn the_sync_verbs_are_the_engines_own() {
    let backend = MockVcard::seeded();
    let value = evaluate(
        &cmd(&["vcard", "sync-token", "--id", "1"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("token"), "token:1");
    let value = evaluate(
        &cmd(&["vcard", "sync", "token:1"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("token"), "token:1:next");
}

/// `format` writes one stored contact back out through the engine's own
/// formatter, and an id the store does not hold is named, not guessed.
#[test]
fn format_writes_one_stored_contact() {
    let backend = MockVcard::seeded();
    let value = evaluate(&cmd(&["vcard", "format", "1"]), &backend, refused()).unwrap();
    let card = value.field_str("card");
    assert!(card.contains("FN:Ada Lovelace"), "{card}");
    assert!(card.contains("EMAIL:ada@example.org"), "{card}");

    let err = evaluate(&cmd(&["vcard", "format", "99"]), &backend, refused()).unwrap_err();
    assert!(err.to_string().contains("no contact with id 99"), "{err}");
}

/// The `@vcard/*` selector tree is the same data the command renders.
#[test]
fn the_vcard_selector_tree_matches_the_command() {
    let backend = MockVcard::seeded();
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@vcard/contacts/0/name").unwrap();
    assert_eq!(resolve(root, &sel), Value::String("Ada Lovelace".to_string()));
}

/// The availability matrix is the ENGINE's own published claim — and it names
/// `resolveConflict` as not served.
#[test]
fn the_matrix_is_the_engines_own_claim() {
    let backend = MockVcard::seeded();
    let value = evaluate(&cmd(&["vcard", "matrix"]), &backend, refused()).unwrap();
    match value.field("rows") {
        Value::List(rows) => {
            let conflict = rows
                .iter()
                .find(|row| row.field_str("method") == "resolveConflict")
                .expect("the matrix names resolveConflict");
            assert_eq!(conflict.field_str("status"), "unavailable");
        }
        other => panic!("expected the matrix rows, got {other:?}"),
    }
}
