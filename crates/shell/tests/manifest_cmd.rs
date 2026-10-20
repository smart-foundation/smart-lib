//! The manifest command module over the scripted engine: the command surface's
//! data half, its rendering, the write gate on the mutating verbs, the
//! validation verdict, and the `@manifest/*` selector tree. Nothing here
//! touches the host (charter law #8).

#![cfg(feature = "domain-manifest")]

mod common;

use clap::Parser;
use common::batch6::manifest::MockManifest;
use shell::domains::manifest::{evaluate, selector_root, ManifestCmd, WriteGuard};
use shell::selector::{self, resolve};
use shell::Value;

fn cmd(argv: &[&str]) -> ManifestCmd {
    ManifestCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the store's manifests.
#[test]
fn the_default_view_is_the_stores_manifests() {
    let backend = MockManifest::seeded();
    let value = evaluate(&cmd(&["manifest"]), &backend, refused()).unwrap();
    assert_eq!(value.field("total").display(), "1");
    match value.field("manifests") {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("name"), "com.example.app");
            assert_eq!(items[0].field_str("display_name"), "Example App");
        }
        other => panic!("expected a manifest list, got {other:?}"),
    }
}

/// `resolve` reads the manifest by its name; `by-name` reads it by its human
/// spelling, optionally scoped by its vendor.
#[test]
fn resolve_and_by_name_read_the_manifest() {
    let backend = MockManifest::seeded();
    let value = evaluate(
        &cmd(&["manifest", "resolve", "com.example.app"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("vendor"), "Example Inc");
    assert_eq!(value.field_str("version"), "1.2.3");

    let value = evaluate(
        &cmd(&["manifest", "by-name", "Example Inc", "Example App"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("name"), "com.example.app");

    // A vendor that did not issue it is not the same manifest.
    assert!(evaluate(
        &cmd(&["manifest", "by-name", "Someone Else", "Example App"]),
        &backend,
        refused()
    )
    .is_err());
}

/// The validation is the engine's own verdict: a required dependency no
/// manifest in the store offers is an ERROR, and the shell never turns it
/// green.
#[test]
fn the_validation_reports_the_engines_own_errors() {
    let backend = MockManifest::seeded();
    let value = evaluate(
        &cmd(&["manifest", "validate", "com.example.app"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert!(!value.field_bool("valid"));
    match value.field("errors") {
        Value::List(errors) => {
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0].display().contains("application/x-example"),
                "{errors:?}"
            );
        }
        other => panic!("expected the errors, got {other:?}"),
    }
    // The rendering says so, in the shell's own words.
    let rendered = shell::domains::manifest::render::render_action(
        &cmd(&["manifest", "validate", "com.example.app"]),
        &value,
        shell::OutputFormat::Human,
    );
    assert!(rendered.contains("invalid"), "{rendered}");
}

/// `register` and `unregister` are gate-routed, and an accepted register is
/// visible to the next read.
#[test]
fn register_is_gated_and_reaches_the_store() {
    let backend = MockManifest::seeded();
    let err = evaluate(
        &cmd(&["manifest", "register", "com.example.other", "--display", "Other"]),
        &backend,
        refused(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("CHANGES the engine's store"), "{err}");
    assert!(backend.writes.borrow().is_empty());

    evaluate(
        &cmd(&[
            "manifest",
            "register",
            "com.example.other",
            "--display",
            "Other",
            "--vendor",
            "Example Inc",
            "--version",
            "2.0.0",
            "--depends",
            "text/plain:optional",
        ]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(backend.writes.borrow().len(), 1);
    let value = evaluate(
        &cmd(&["manifest", "resolve", "com.example.other"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("display_name"), "Other");
    assert_eq!(value.field_str("version"), "2.0.0");
}

/// A dependency the caller spells as optional is carried as optional, and the
/// engine's validation does not fail on it.
#[test]
fn an_optional_dependency_is_carried_as_optional() {
    let backend = MockManifest::empty();
    evaluate(
        &cmd(&[
            "manifest",
            "register",
            "com.example.opt",
            "--depends",
            "text/plain:optional",
        ]),
        &backend,
        allowed(),
    )
    .unwrap();
    let value = evaluate(
        &cmd(&["manifest", "validate", "com.example.opt"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert!(value.field_bool("valid"), "an optional dependency is not an error");
}

/// The rendering is a table of the store's own rows.
#[test]
fn the_rendering_is_a_table_of_the_stores_rows() {
    let backend = MockManifest::seeded();
    let value = evaluate(&cmd(&["manifest"]), &backend, refused()).unwrap();
    let rendered = shell::domains::manifest::render::render_action(
        &cmd(&["manifest"]),
        &value,
        shell::OutputFormat::Human,
    );
    assert!(rendered.contains("com.example.app"), "{rendered}");
    assert!(rendered.contains("VENDOR"), "{rendered}");
    common::assert_borders_hold(&rendered);
}

/// The `@manifest/*` selector tree is the same data the command renders.
#[test]
fn the_manifest_selector_tree_matches_the_command() {
    let backend = MockManifest::seeded();
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@manifest/manifests/0/display_name").unwrap();
    assert_eq!(resolve(root, &sel), Value::String("Example App".to_string()));
}

/// The availability matrix is the ENGINE's own published claim.
#[test]
fn the_matrix_is_the_engines_own_claim() {
    let backend = MockManifest::empty();
    let value = evaluate(&cmd(&["manifest", "matrix"]), &backend, refused()).unwrap();
    match value.field("rows") {
        Value::List(rows) => {
            assert_eq!(rows[0].field_str("method"), "resolve");
            assert_eq!(rows[0].field_str("status"), "real");
        }
        other => panic!("expected the matrix rows, got {other:?}"),
    }
}
