//! The appindex command module over the scripted engine: the command surface's
//! data half, its rendering, the write gate on the mutating verbs, and the
//! `@appindex/*` selector tree. Nothing here touches the host (charter law #8).

#![cfg(feature = "domain-appindex")]

mod common;

use clap::Parser;
use common::batch6::appindex::MockAppindex;
use shell::domains::appindex::{evaluate, selector_root, AppindexCmd, WriteGuard};
use shell::selector::{self, resolve};
use shell::Value;

fn cmd(argv: &[&str]) -> AppindexCmd {
    AppindexCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the corpus the host publishes, with the corpus's own
/// total beside it.
#[test]
fn the_default_view_is_the_hosts_corpus() {
    let backend = MockAppindex::seeded();
    let value = evaluate(&cmd(&["appindex"]), &backend, refused()).unwrap();
    assert_eq!(value.field("total").display(), "3");
    match value.field("apps") {
        Value::List(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0].field_str("id"), "alpha.desktop");
            assert_eq!(items[0].field_str("name"), "Alpha");
            assert_eq!(items[0].field_str("source"), "/usr/share/applications");
        }
        other => panic!("expected an app list, got {other:?}"),
    }
}

/// `get` reads one entry by the id the host keys it under.
#[test]
fn get_reads_one_entry_by_its_id() {
    let backend = MockAppindex::seeded();
    let value = evaluate(
        &cmd(&["appindex", "get", "beta.desktop"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("name"), "Beta");
    assert_eq!(value.field_str("generic_name"), "Browser");
    assert!(evaluate(&cmd(&["appindex", "get", "nope.desktop"]), &backend, refused()).is_err());
}

/// `search` filters on the name, the category and the source, exactly as the
/// engine's own filter does.
#[test]
fn search_filters_on_the_names_categories_and_source() {
    let backend = MockAppindex::seeded();
    let value = evaluate(&cmd(&["appindex", "search", "edit"]), &backend, refused()).unwrap();
    match value.field("apps") {
        Value::List(items) => {
            assert_eq!(items.len(), 2, "Alpha and Gamma are editors");
        }
        other => panic!("expected an app list, got {other:?}"),
    }
    let value = evaluate(
        &cmd(&[
            "appindex",
            "search",
            "e",
            "--source",
            "/usr/local/share/applications",
        ]),
        &backend,
        refused(),
    )
    .unwrap();
    match value.field("apps") {
        Value::List(items) => {
            assert_eq!(items.len(), 1, "only Gamma comes from the local directory");
        }
        other => panic!("expected an app list, got {other:?}"),
    }
}

/// `stats` is the engine's own statistics: the entry count and the per-source
/// and per-category counts.
#[test]
fn stats_are_the_engines_own_counts() {
    let backend = MockAppindex::seeded();
    let value = evaluate(&cmd(&["appindex", "stats"]), &backend, refused()).unwrap();
    assert_eq!(value.field("total_apps").display(), "3");
    match value.field("source_counts") {
        Value::List(items) => {
            assert_eq!(items.len(), 2, "two source directories");
            let counted: f64 = items
                .iter()
                .map(|item| item.field("count").as_f64().unwrap_or(0.0))
                .sum();
            assert_eq!(counted, 3.0, "every entry is counted under its source");
        }
        other => panic!("expected the source counts, got {other:?}"),
    }
}

/// The pinned set is the ENGINE's own: `pin` changes it, and `pinned` reads
/// it back. A refused write reaches no engine.
#[test]
fn the_pinned_set_is_the_engines_own_and_is_gated() {
    let backend = MockAppindex::seeded();
    let err = evaluate(
        &cmd(&["appindex", "pin", "alpha.desktop"]),
        &backend,
        refused(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("pinned set"), "{err}");
    assert!(backend.writes.borrow().is_empty());

    evaluate(
        &cmd(&["appindex", "pin", "alpha.desktop"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(backend.writes.borrow().len(), 1);
    let value = evaluate(&cmd(&["appindex", "pinned"]), &backend, refused()).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("id"), "alpha.desktop");
        }
        other => panic!("expected the pinned list, got {other:?}"),
    }
    // The entry itself now carries the pin.
    let value = evaluate(
        &cmd(&["appindex", "get", "alpha.desktop"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert!(value.field_bool("pinned"));
}

/// `batch` reads several entries in one call, in the order asked.
#[test]
fn batch_reads_several_entries() {
    let backend = MockAppindex::seeded();
    let value = evaluate(
        &cmd(&["appindex", "batch", "gamma.desktop", "alpha.desktop"]),
        &backend,
        refused(),
    )
    .unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].field_str("id"), "gamma.desktop");
        }
        other => panic!("expected an app list, got {other:?}"),
    }
}

/// The rendering is a table of the host's own rows.
#[test]
fn the_rendering_is_a_table_of_the_hosts_rows() {
    let backend = MockAppindex::seeded();
    let value = evaluate(&cmd(&["appindex"]), &backend, refused()).unwrap();
    let rendered = shell::domains::appindex::render::render_action(
        &cmd(&["appindex"]),
        &value,
        shell::OutputFormat::Human,
    );
    assert!(rendered.contains("alpha.desktop"), "{rendered}");
    assert!(rendered.contains("NAME"), "{rendered}");
    common::assert_borders_hold(&rendered);
}

/// The `@appindex/*` selector tree is the same data the command renders.
#[test]
fn the_appindex_selector_tree_matches_the_command() {
    let backend = MockAppindex::seeded();
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@appindex/apps/0/name").unwrap();
    assert_eq!(resolve(root, &sel), Value::String("Alpha".to_string()));
}

/// The availability matrix is the ENGINE's own published claim — and it says
/// why `launch` is not served.
#[test]
fn the_matrix_is_the_engines_own_claim() {
    let backend = MockAppindex::empty();
    let value = evaluate(&cmd(&["appindex", "matrix"]), &backend, refused()).unwrap();
    match value.field("rows") {
        Value::List(rows) => {
            assert_eq!(rows[0].field_str("method"), "listApps");
            let launch = rows
                .iter()
                .find(|row| row.field_str("method") == "launch")
                .expect("the matrix names launch");
            assert_eq!(launch.field_str("status"), "unavailable");
            assert!(!launch.field_str("reason").is_empty(), "the reason is named");
        }
        other => panic!("expected the matrix rows, got {other:?}"),
    }
}
