//! The queue command module over the scripted engine: the command surface's
//! data half, its rendering, the write gate on the mutating verbs, and the
//! `@queue/*` selector tree. Nothing here touches the host (charter law #8).

#![cfg(feature = "domain-queue")]

mod common;

use clap::Parser;
use common::batch6::queue::MockQueue;
use shell::domains::queue::{evaluate, selector_root, QueueCmd, WriteGuard};
use shell::selector::{self, resolve};
use shell::Value;

fn cmd(argv: &[&str]) -> QueueCmd {
    QueueCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the store's queues.
#[test]
fn the_default_view_is_the_stores_queues() {
    let backend = MockQueue::seeded();
    let value = evaluate(&cmd(&["queue"]), &backend, refused()).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("name"), "jobs");
        }
        other => panic!("expected a queue list, got {other:?}"),
    }
}

/// `peek` reads the head of a queue without removing it, and never calls an
/// absent queue "empty": the existence is read first.
#[test]
fn peek_reads_the_head_without_removing_it() {
    let backend = MockQueue::seeded();
    let value = evaluate(&cmd(&["queue", "peek", "jobs"]), &backend, refused()).unwrap();
    assert!(value.field_bool("exists"));
    assert_eq!(value.field_str("payload"), "first");
    assert_eq!(value.field_str("encoding"), "utf-8");
    // The peek did not consume it.
    let again = evaluate(&cmd(&["queue", "peek", "jobs"]), &backend, refused()).unwrap();
    assert_eq!(again.field_str("payload"), "first");

    // A queue that is not there is not an empty queue.
    let missing = evaluate(&cmd(&["queue", "peek", "nope"]), &backend, refused()).unwrap();
    assert!(!missing.field_bool("exists"));
}

/// `get` answers whether the store holds a queue under that name.
#[test]
fn get_answers_whether_the_queue_exists() {
    let backend = MockQueue::seeded();
    let value = evaluate(&cmd(&["queue", "get", "jobs"]), &backend, refused()).unwrap();
    assert!(value.field_bool("exists"));
    assert_eq!(value.field_str("name"), "jobs");
    assert!(evaluate(&cmd(&["queue", "get", "nope"]), &backend, refused()).is_err());
}

/// Every mutating verb is gate-routed: refused without an acceptance, and the
/// refused write reaches NO engine.
#[test]
fn the_mutating_verbs_are_gate_routed() {
    let backend = MockQueue::seeded();
    for argv in [
        vec!["queue", "enqueue", "jobs", "payload"],
        vec!["queue", "dequeue", "jobs"],
        vec!["queue", "clear", "jobs"],
        vec!["queue", "delete", "jobs"],
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

    // Accepted: the engine call is recorded, and the store really changed.
    evaluate(
        &cmd(&["queue", "enqueue", "jobs", "third"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(backend.writes.borrow().len(), 1);
    let value = evaluate(&cmd(&["queue", "dequeue", "jobs"]), &backend, allowed()).unwrap();
    assert_eq!(value.field_str("payload"), "first");
    assert!(value.field_bool("removed"));
}

/// The rendering says "the queue is empty" honestly, and a queue that is not
/// there is never called empty. It never claims a depth the engine never
/// measured.
#[test]
fn the_rendering_is_honest_about_an_empty_queue() {
    let backend = MockQueue::empty();
    // A queue that exists and holds nothing.
    evaluate(
        &cmd(&["queue", "enqueue", "jobs", "x"]),
        &backend,
        allowed(),
    )
    .unwrap();
    evaluate(&cmd(&["queue", "dequeue", "jobs"]), &backend, allowed()).unwrap();
    let value = evaluate(&cmd(&["queue", "peek", "jobs"]), &backend, refused()).unwrap();
    assert!(value.field_bool("exists"));
    let rendered = shell::domains::queue::render::render_action(
        &cmd(&["queue", "peek", "jobs"]),
        &value,
        shell::OutputFormat::Human,
    );
    assert!(rendered.contains("empty"), "{rendered}");
    assert!(!rendered.contains("depth"), "{rendered}");

    // And a queue that is not there is named as such.
    let value = evaluate(&cmd(&["queue", "peek", "nope"]), &backend, refused()).unwrap();
    let rendered = shell::domains::queue::render::render_action(
        &cmd(&["queue", "peek", "nope"]),
        &value,
        shell::OutputFormat::Human,
    );
    assert!(rendered.contains("no queue by that name"), "{rendered}");
    assert!(!rendered.contains("empty"), "{rendered}");
}

/// The `@queue/*` selector tree is the same data the command renders.
#[test]
fn the_queue_selector_tree_matches_the_command() {
    let backend = MockQueue::seeded();
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@queue/queues/0/name").unwrap();
    assert_eq!(resolve(root, &sel), Value::String("jobs".to_string()));
}

/// The availability matrix is the ENGINE's own published claim.
#[test]
fn the_matrix_is_the_engines_own_claim() {
    let backend = MockQueue::empty();
    let value = evaluate(&cmd(&["queue", "matrix"]), &backend, refused()).unwrap();
    match value.field("rows") {
        Value::List(rows) => {
            assert_eq!(rows[0].field_str("method"), "enqueue");
            assert_eq!(rows[0].field_str("status"), "real");
        }
        other => panic!("expected the matrix rows, got {other:?}"),
    }
}
