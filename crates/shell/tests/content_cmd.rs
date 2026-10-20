//! The content command module over the scripted engine: the command surface's
//! data half, its rendering, the write gate on the mutating verbs, the
//! optimistic-concurrency check, and the `@content/*` selector tree. Nothing
//! here touches the host (charter law #8).

#![cfg(feature = "domain-content")]

mod common;

use clap::Parser;
use common::batch6::content::MockContent;
use shell::domains::content::{evaluate, selector_root, ContentCmd, WriteGuard};
use shell::selector::{self, resolve};
use shell::Value;

fn cmd(argv: &[&str]) -> ContentCmd {
    ContentCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the store's items, with the store's own total.
#[test]
fn the_default_view_is_the_stores_items() {
    let backend = MockContent::seeded();
    let value = evaluate(&cmd(&["content"]), &backend, refused()).unwrap();
    assert_eq!(value.field("total").display(), "1");
    match value.field("items") {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("title"), "First Note");
            assert_eq!(items[0].field("version").display(), "1");
        }
        other => panic!("expected an item list, got {other:?}"),
    }
}

/// `get` reads one item, with the engine's own body and version.
#[test]
fn get_reads_one_item() {
    let backend = MockContent::seeded();
    let value = evaluate(&cmd(&["content", "get", "1"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("title"), "First Note");
    assert!(value.field_str("body").contains("First Note"));
    assert!(evaluate(&cmd(&["content", "get", "99"]), &backend, refused()).is_err());
}

/// The query filters on the text, the MIME type and the tags — the engine's
/// own filter, not the shell's guess.
#[test]
fn the_query_filters_on_the_text_the_mime_and_the_tags() {
    let backend = MockContent::seeded();
    let value = evaluate(
        &cmd(&["content", "list", "--query", "first"]),
        &backend,
        refused(),
    )
    .unwrap();
    match value.field("items") {
        Value::List(items) => assert_eq!(items.len(), 1),
        other => panic!("expected an item list, got {other:?}"),
    }
    let value = evaluate(
        &cmd(&["content", "list", "--mime", "application/"]),
        &backend,
        refused(),
    )
    .unwrap();
    match value.field("items") {
        Value::List(items) => assert!(items.is_empty(), "no item carries that MIME type"),
        other => panic!("expected an item list, got {other:?}"),
    }
    let value = evaluate(
        &cmd(&["content", "list", "--tag", "demo"]),
        &backend,
        refused(),
    )
    .unwrap();
    match value.field("items") {
        Value::List(items) => assert_eq!(items.len(), 1),
        other => panic!("expected an item list, got {other:?}"),
    }
}

/// Every mutating verb is gate-routed: refused without an acceptance, and the
/// refused write reaches NO engine.
#[test]
fn the_mutating_verbs_are_gate_routed() {
    let backend = MockContent::seeded();
    for argv in [
        vec!["content", "publish", "A Note"],
        vec!["content", "publish-many", "A", "B"],
        vec!["content", "update", "1", "--title", "New"],
        vec!["content", "unpublish", "1"],
        vec!["content", "delete", "1"],
        vec!["content", "trash", "1"],
        vec!["content", "restore", "1"],
        vec!["content", "empty-trash"],
    ] {
        let err = evaluate(&cmd(&argv), &backend, refused()).unwrap_err();
        assert!(
            err.to_string().contains("CHANGES the engine's store"),
            "{argv:?}: {err}"
        );
    }
    assert!(
        backend.writes.borrow().is_empty(),
        "a refused write reaches no engine"
    );
}

/// An accepted publish is visible to the next read — the store is the engine's
/// own, and the shell speaks it, not a copy.
#[test]
fn an_accepted_publish_is_visible_to_the_next_read() {
    let backend = MockContent::seeded();
    let value = evaluate(
        &cmd(&["content", "publish", "Second Note", "--body", "the body"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(value.field("id").display(), "2");
    let value = evaluate(&cmd(&["content", "get", "2"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("title"), "Second Note");
    assert_eq!(value.field_str("body"), "the body");
}

/// The update carries the contract's own optimistic concurrency: a write that
/// presents a stale version is refused with a conflict, never a silent
/// overwrite.
#[test]
fn a_stale_update_is_refused_with_a_conflict() {
    let backend = MockContent::seeded();
    let err = evaluate(
        &cmd(&["content", "update", "1", "--expected-version", "9", "--title", "New"]),
        &backend,
        allowed(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
    // And the item kept its own version.
    let value = evaluate(&cmd(&["content", "get", "1"]), &backend, refused()).unwrap();
    assert_eq!(value.field("version").display(), "1");
}

/// The update replaces only the named fields: an update that names no field is
/// a re-write of the same item, never a blank one.
#[test]
fn an_update_keeps_the_fields_it_does_not_name() {
    let backend = MockContent::seeded();
    evaluate(
        &cmd(&["content", "update", "1", "--expected-version", "1", "--title", "Renamed"]),
        &backend,
        allowed(),
    )
    .unwrap();
    let value = evaluate(&cmd(&["content", "get", "1"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("title"), "Renamed");
    assert_eq!(value.field_str("summary"), "the First Note summary");
    assert_eq!(value.field("version").display(), "2");
}

/// The trash and the restore move an item between the two states, and
/// emptying the trash removes only the trashed ones.
#[test]
fn the_trash_the_restore_and_the_empty_trash() {
    let backend = MockContent::seeded();
    evaluate(&cmd(&["content", "trash", "1"]), &backend, allowed()).unwrap();
    let value = evaluate(&cmd(&["content", "get", "1"]), &backend, refused()).unwrap();
    assert!(value.field_bool("trashed"));
    // A trashed item is out of the query's way.
    let value = evaluate(&cmd(&["content", "list"]), &backend, refused()).unwrap();
    match value.field("items") {
        Value::List(items) => assert!(items.is_empty()),
        other => panic!("expected an item list, got {other:?}"),
    }
    // Unless the caller asks for the trashed ones.
    let value = evaluate(
        &cmd(&["content", "list", "--include-trashed"]),
        &backend,
        refused(),
    )
    .unwrap();
    match value.field("items") {
        Value::List(items) => assert_eq!(items.len(), 1),
        other => panic!("expected an item list, got {other:?}"),
    }
    let value = evaluate(
        &cmd(&["content", "empty-trash"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(value.field("emptied").display(), "1");
    assert!(evaluate(&cmd(&["content", "get", "1"]), &backend, refused()).is_err());
}

/// The rendering is a table of the store's own rows.
#[test]
fn the_rendering_is_a_table_of_the_stores_rows() {
    let backend = MockContent::seeded();
    let value = evaluate(&cmd(&["content"]), &backend, refused()).unwrap();
    let rendered = shell::domains::content::render::render_action(
        &cmd(&["content"]),
        &value,
        shell::OutputFormat::Human,
    );
    assert!(rendered.contains("First Note"), "{rendered}");
    assert!(rendered.contains("TITLE"), "{rendered}");
    common::assert_borders_hold(&rendered);
}

/// The `@content/*` selector tree is the same data the command renders.
#[test]
fn the_content_selector_tree_matches_the_command() {
    let backend = MockContent::seeded();
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@content/items/0/title").unwrap();
    assert_eq!(resolve(root, &sel), Value::String("First Note".to_string()));
}

/// The availability matrix is the ENGINE's own published claim.
#[test]
fn the_matrix_is_the_engines_own_claim() {
    let backend = MockContent::empty();
    let value = evaluate(&cmd(&["content", "matrix"]), &backend, refused()).unwrap();
    match value.field("rows") {
        Value::List(rows) => {
            assert_eq!(rows[0].field_str("method"), "publish");
            assert_eq!(rows[0].field_str("status"), "real");
        }
        other => panic!("expected the matrix rows, got {other:?}"),
    }
}
