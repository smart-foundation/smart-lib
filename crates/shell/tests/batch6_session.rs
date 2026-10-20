//! The Batch 6 six at the SHELL level: the session keeps ONE backend per
//! domain, so a write one line makes is a read the next line sees — and the
//! write gate holds when the operator has not accepted it. The host is never
//! touched (charter law #8): every backend here is a scripted one.

#![cfg(all(
    feature = "domain-database",
    feature = "domain-queue",
    feature = "domain-appindex",
    feature = "domain-manifest",
    feature = "domain-vcard",
    feature = "domain-content"
))]

mod common;

use common::batch6::appindex::MockAppindex;
use common::batch6::content::MockContent;
use common::batch6::database::MockDatabase;
use common::batch6::manifest::MockManifest;
use common::batch6::queue::MockQueue;
use common::batch6::vcard::MockVcard;
use common::MockBackend;
use shell::selector::Value;
use shell::shell::{Shell, WriteGate};

fn shell() -> Shell {
    Shell::new(Box::new(MockBackend::host()))
}

/// The gate with the write accepted — the documented `--force`.
fn forced() -> WriteGate {
    WriteGate {
        force: true,
        ..WriteGate::default()
    }
}

/// The session keeps ONE database backend, so a write one line makes is a
/// read the next line sees.
#[test]
fn the_session_keeps_one_database_backend() {
    let mut sh = shell();
    sh.set_database(Box::new(MockDatabase::seeded()));
    sh.set_gate(forced());

    sh.exec_cmd(&Shell::parse("database set demo greeting hello-there"))
        .unwrap();
    // The NEXT line reads the engine's own store — the same backend, the
    // same memory: the value the write made is the value the read sees.
    let value = sh.database().get("demo", "greeting").unwrap();
    assert_eq!(value, b"hello-there".to_vec());
}

/// The same for the content store: publish, then get, in one session.
#[test]
fn the_session_keeps_one_content_backend() {
    let mut sh = shell();
    sh.set_content(Box::new(MockContent::empty()));
    sh.set_gate(forced());
    sh.exec_cmd(&Shell::parse("content publish Note")).unwrap();
    let backend = sh.content();
    // The store really holds the item the line published.
    let items = shell::domains::content::render::list_value(
        backend,
        &smart_lib::domains::content::types::ContentFilter::default(),
        smart_lib::domains::content::types::Pagination {
            offset: 0,
            limit: 10,
        },
    )
    .unwrap();
    match items.field("items") {
        Value::List(rows) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].field_str("title"), "Note");
        }
        other => panic!("expected an item list, got {other:?}"),
    }
}

/// The queue, the manifest store, the vCard store and the pinned set are the
/// same story: one backend each, for the life of the session.
#[test]
fn the_session_keeps_one_backend_per_batch6_domain() {
    let mut sh = shell();
    sh.set_queue(Box::new(MockQueue::seeded()));
    sh.set_manifest(Box::new(MockManifest::seeded()));
    sh.set_vcard(Box::new(MockVcard::seeded()));
    sh.set_appindex(Box::new(MockAppindex::seeded()));

    // The same backend answers every read — the one the session built.
    let first = sh.queue().list_queues().unwrap();
    let second = sh.queue().list_queues().unwrap();
    assert_eq!(first, second);
    assert_eq!(sh.manifest().list(Default::default()).unwrap().total, 1);
    assert_eq!(sh.vcard().contacts().unwrap().len(), 1);
    assert_eq!(sh.appindex().get_stats().unwrap().total_apps, 3);
}

/// The write gate holds at the shell level too: without an acceptance the
/// verb refuses and the engine is never asked.
#[test]
fn the_write_gate_holds_at_the_shell_level() {
    let mut sh = shell();
    sh.set_database(Box::new(MockDatabase::seeded()));
    // The default gate: nobody accepted the write.
    let err = sh
        .exec_cmd(&Shell::parse("database set demo greeting hello"))
        .unwrap_err();
    assert!(err.to_string().contains("CHANGES state"), "{err}");
    // And the engine's store is untouched.
    let value = shell::domains::database::render::value_record(
        "demo",
        "greeting",
        &sh.database().get("demo", "greeting").unwrap(),
    );
    assert_eq!(value.field_str("value"), "hello");
}
