//! The database command module over the scripted engine: the command surface's
//! data half, its rendering, the write gate on the mutating verbs, and the
//! `@database/*` selector tree. Nothing here touches the host (charter law #8).

#![cfg(feature = "domain-database")]

mod common;

use clap::Parser;
use common::batch6::database::MockDatabase;
use shell::domains::database::{evaluate, selector_root, DatabaseCmd, WriteGuard};
use shell::selector::{self, resolve};
use shell::Value;
use smart_lib::domains::database::DatabaseBackend;

fn cmd(argv: &[&str]) -> DatabaseCmd {
    DatabaseCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the store's namespaces, each with the number of keys it
/// holds — the engine's own `listNamespaces` and `count` reads.
#[test]
fn the_default_view_is_the_stores_namespaces() {
    let backend = MockDatabase::seeded();
    let value = evaluate(&cmd(&["database"]), &backend, refused()).unwrap();
    assert_eq!(value.field("total").display(), "1");
    match value.field("namespaces") {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("namespace"), "demo");
            assert_eq!(items[0].field("keys").display(), "2");
        }
        other => panic!("expected a namespace list, got {other:?}"),
    }
}

/// `get` reads the engine's own bytes: the text when the payload is UTF-8,
/// and the hex when it is not (a lossy replacement would be a lie about the
/// bytes).
#[test]
fn get_reads_the_stores_bytes() {
    let backend = MockDatabase::seeded();
    let value = evaluate(&cmd(&["database", "get", "demo", "greeting"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("value"), "hello");
    assert_eq!(value.field_str("encoding"), "utf-8");
    assert_eq!(value.field("size").display(), "5");

    // A payload that is not UTF-8 is reported as hex, never mangled.
    evaluate(
        &cmd(&["database", "set", "demo", "blob", "x"]),
        &backend,
        allowed(),
    )
    .unwrap();
    backend
        .set("demo", "blob", &[0xff, 0x00, 0x7f])
        .unwrap();
    let value = evaluate(&cmd(&["database", "get", "demo", "blob"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("encoding"), "hex");
    assert_eq!(value.field_str("value"), "ff007f");
}

/// `has` answers the store's own yes or no; a missing key is not an error.
#[test]
fn has_answers_without_an_error() {
    let backend = MockDatabase::seeded();
    let value = evaluate(&cmd(&["database", "has", "demo", "greeting"]), &backend, refused()).unwrap();
    assert!(value.field_bool("present"));
    let value = evaluate(&cmd(&["database", "has", "demo", "nope"]), &backend, refused()).unwrap();
    assert!(!value.field_bool("present"));
}

/// `keys` lists the namespace's keys and honours the prefix and the paging.
#[test]
fn keys_lists_the_namespace_and_honours_the_prefix() {
    let backend = MockDatabase::seeded();
    let value = evaluate(&cmd(&["database", "keys", "demo"]), &backend, refused()).unwrap();
    match value.field("keys") {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].field_str("key"), "colour");
        }
        other => panic!("expected a key list, got {other:?}"),
    }
    let value = evaluate(
        &cmd(&["database", "keys", "demo", "--prefix", "gr"]),
        &backend,
        refused(),
    )
    .unwrap();
    match value.field("keys") {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("key"), "greeting");
        }
        other => panic!("expected a key list, got {other:?}"),
    }
    // And the count is the store's own.
    let value = evaluate(&cmd(&["database", "count", "demo"]), &backend, refused()).unwrap();
    assert_eq!(value.field("count").display(), "2");
}

/// Every mutating verb is gate-routed: refused without an acceptance, and the
/// refused write reaches NO engine (charter law #8's shape).
#[test]
fn the_mutating_verbs_are_gate_routed() {
    let backend = MockDatabase::seeded();
    for argv in [
        vec!["database", "set", "demo", "greeting", "hi"],
        vec!["database", "delete", "demo", "greeting"],
        vec!["database", "clear", "demo"],
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

    evaluate(
        &cmd(&["database", "set", "demo", "greeting", "hi"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(backend.writes.borrow().len(), 1, "an accepted write reaches the engine");
    // And the accepted write is visible to the next read — the store is the
    // engine's own, and the shell speaks it, not a copy.
    let value = evaluate(&cmd(&["database", "get", "demo", "greeting"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("value"), "hi");
}

/// The write intent names the verb and the target, never the value: a stored
/// value is not echoed back by a confirmation prompt.
#[test]
fn the_write_intent_names_the_verb_and_the_target() {
    let intent = shell::domains::database::write_intent(&cmd(&[
        "database", "set", "demo", "greeting", "secret",
    ]))
    .expect("set is a write");
    assert_eq!(intent.0, "set database value");
    assert_eq!(intent.1, "demo/greeting");
    assert!(
        !intent.1.contains("secret"),
        "the value is never echoed: {intent:?}"
    );
    assert!(shell::domains::database::write_intent(&cmd(&["database", "get", "demo", "greeting"])).is_none());
}

/// The rendering is a table the engine's own rows fill: the namespace, the
/// key and the byte count.
#[test]
fn the_rendering_is_a_table_of_the_stores_rows() {
    let backend = MockDatabase::seeded();
    let value = evaluate(&cmd(&["database"]), &backend, refused()).unwrap();
    let rendered = shell::domains::database::render::render_action(
        &cmd(&["database"]),
        &value,
        shell::OutputFormat::Human,
    );
    assert!(rendered.contains("demo"), "{rendered}");
    assert!(rendered.contains("NAMESPACE"), "{rendered}");
    common::assert_borders_hold(&rendered);
}

/// The `@database/*` selector tree is the same data the command renders, so a
/// selector and a command can never disagree.
#[test]
fn the_database_selector_tree_matches_the_command() {
    let backend = MockDatabase::seeded();
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@database/namespaces/0/namespace").unwrap();
    assert_eq!(resolve(root, &sel), Value::String("demo".to_string()));
}

/// The availability matrix is the ENGINE's own published claim, rendered as
/// it stands — never a row the shell invented.
#[test]
fn the_matrix_is_the_engines_own_claim() {
    let backend = MockDatabase::empty();
    let value = evaluate(&cmd(&["database", "matrix"]), &backend, refused()).unwrap();
    match value.field("rows") {
        Value::List(rows) => {
            assert!(rows.len() > 1, "the matrix has its rows");
            let first = &rows[0];
            assert_eq!(first.field_str("method"), "get");
            assert_eq!(first.field_str("status"), "real");
        }
        other => panic!("expected the matrix rows, got {other:?}"),
    }
}
