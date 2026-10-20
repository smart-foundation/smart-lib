//! The selector grammar and its evaluation over the engine: paths, indexes,
//! wildcards, `where` filters — and the honest answer for a domain that is
//! not migrated yet.

mod common;

use common::MockBackend;
use shell::selector::{evaluate, parse, Value};

fn host() -> MockBackend {
    MockBackend::host()
}

#[test]
fn parses_the_selector_grammar() {
    let sel = parse("@network/interfaces").unwrap();
    assert_eq!(sel.domain, "network");
    assert_eq!(sel.path.len(), 1);

    let sel = parse("@network/interfaces where type == wifi").unwrap();
    assert!(sel.filter.is_some(), "the filter parsed");
    assert_eq!(sel.path.len(), 1);

    let sel = parse("@network/interfaces/0/name").unwrap();
    assert_eq!(sel.path.len(), 3, "index and field segments");

    let sel = parse("@network/radios/*").unwrap();
    assert_eq!(sel.path.len(), 2);

    assert!(parse("network/interfaces").is_err(), "a selector must start with @");
}

#[test]
fn evaluates_the_network_tree_from_the_engines_owned_types() {
    let backend = host();
    let root = evaluate(&parse("@network").unwrap(), &backend).unwrap();
    // The root carries the whole domain, so any path resolves off it.
    for section in ["networking", "interfaces", "radios", "known", "status"] {
        assert!(
            matches!(root.field(section), v if !matches!(v, Value::Null)),
            "the network tree carries {section}"
        );
    }
    assert!(root.field_bool("networking"));
}

#[test]
fn a_selector_filter_agrees_with_the_command_surface() {
    let backend = host();
    let value = evaluate(
        &parse("@network/interfaces where type == wifi").unwrap(),
        &backend,
    )
    .unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("name"), "wlp0s20f3");
            // The record the selector serves is the engine's own snapshot:
            // the same type `getInterfaces` enumerates (bug 1's lock).
            assert_eq!(items[0].field_str("type"), "wifi");
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

#[test]
fn paths_index_and_project() {
    let backend = host();
    let first = evaluate(&parse("@network/interfaces/0/name").unwrap(), &backend).unwrap();
    assert_eq!(first, Value::String("enp0s31f6".to_string()));
    let names = evaluate(&parse("@network/interfaces/*/name").unwrap(), &backend).unwrap();
    assert!(matches!(names, Value::List(_)));
    let missing = evaluate(&parse("@network/interfaces/99/name").unwrap(), &backend).unwrap();
    assert_eq!(missing, Value::Null, "an out-of-range index is Null, never a panic");
}

#[test]
fn the_status_section_carries_the_four_state_hoststate() {
    let backend = host();
    let value = evaluate(&parse("@network/status").unwrap(), &backend).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            let wifi = items
                .iter()
                .find(|i| i.field_str("name") == "wlp0s20f3")
                .expect("the wifi interface's status");
            assert_eq!(wifi.field_str("type"), "wifi");
            assert_eq!(wifi.field_str("host_state"), "managed");
            assert!(wifi.field_str("host_state_source") == "kernel" || wifi.field_str("host_state_source") == "nm-policy");
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// The honesty rule on the selector surface: an unmigrated domain's
/// selector answers from the queue, never a fabricated value.
#[test]
fn a_still_queued_selector_answers_from_the_queue() {
    let backend = host();
    // A domain that is still queued (window) is answered from the migration
    // queue's archive note — the batch-6 six (database, queue, appindex,
    // manifest, vcard, content) are real now, so they are no longer the
    // example of an honest pending answer.
    let err = evaluate(&parse("@window/tree").unwrap(), &backend).unwrap_err();
    assert!(
        err.to_string().contains("migration pending")
            && err.to_string().contains("platforms/linux/window.rs"),
        "the selector's honest answer names the archive body: {err}"
    );
    // And the engine was never asked for it.
    assert!(backend.seams().is_empty());
}
