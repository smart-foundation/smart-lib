//! The clipboard command module over the scripted engine: the command surface's
//! data half, its rendering, the host-write gate on the mutating verbs, and the
//! `@clipboard/*` selector tree. Nothing here touches the host (charter law #8).

mod common;

use clap::Parser;
use common::MockClipboard;
use shell::domains::clipboard::{evaluate, selector_root, ClipboardCmd, WriteGuard};
use shell::selector::{self, resolve};
use shell::Value;
use smart_lib::domains::clipboard::ClipboardBackend;

fn cmd(argv: &[&str]) -> ClipboardCmd {
    ClipboardCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the clipboard's state: transport, content, and (empty)
/// history.
#[test]
fn the_default_view_is_the_clipboard_state() {
    let backend = MockClipboard::seat();
    let value = evaluate(&cmd(&["clipboard"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("transport"), "scripted");
    assert!(value.field_bool("available"));
    assert_eq!(value.field("content"), &Value::Null);
}

/// `clipboard get` reads the native selection: content when the seat holds it,
/// and the contract's first-class empty when it does not.
#[test]
fn get_reads_the_selection_and_distinguishes_empty() {
    let backend = MockClipboard::seat();
    // Empty clipboard.
    let empty = evaluate(&cmd(&["clipboard", "get"]), &backend, refused()).unwrap();
    assert_eq!(empty, Value::Null, "an empty clipboard is Null, not an error");
    // After a (gated) set, the copy is served back.
    backend
        .set_content(smart_lib::domains::clipboard::types::ClipboardContent {
            mime_type: "text/plain".into(),
            data: b"via the mock".to_vec(),
        })
        .unwrap();
    let value = evaluate(&cmd(&["clipboard", "get"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("mime_type"), "text/plain");
    assert_eq!(value.field_str("text"), "via the mock");
}

/// The mutating verb is gate-routed: refused without `--force`, recorded once
/// accepted — exactly the network domain's gate, re-used (charter law #8).
#[test]
fn set_is_gated_and_records_the_engine_call() {
    let backend = MockClipboard::seat();
    let err = evaluate(&cmd(&["clipboard", "set", "hello"]), &backend, refused()).unwrap_err();
    assert!(err.to_string().contains("CHANGES host state"), "{err}");
    assert!(backend.writes.borrow().is_empty(), "a refused write reaches no engine");

    evaluate(&cmd(&["clipboard", "set", "hello"]), &backend, allowed()).unwrap();
    assert_eq!(backend.writes.borrow().len(), 1, "an accepted write reaches the engine");
    assert_eq!(evaluate(&cmd(&["clipboard", "get"]), &backend, refused()).unwrap().field_str("text"), "hello");
}

/// `clipboard clear` is gate-routed too.
#[test]
fn clear_is_gated() {
    let backend = MockClipboard::seat();
    assert!(evaluate(&cmd(&["clipboard", "clear"]), &backend, refused()).is_err());
    assert!(evaluate(&cmd(&["clipboard", "clear"]), &backend, allowed()).is_ok());
    assert_eq!(backend.writes.borrow().last().map(String::as_str), Some("clear_content"));
}

/// The copy-history ledger renders as records the shell language can pipe.
#[test]
fn history_lists_the_ledger() {
    let backend = MockClipboard::seat();
    backend
        .set_content(smart_lib::domains::clipboard::types::ClipboardContent {
            mime_type: "text/plain".into(),
            data: b"one".to_vec(),
        })
        .unwrap();
    let value = evaluate(&cmd(&["clipboard", "history"]), &backend, refused()).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("text"), "one");
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// The `@clipboard/*` selector tree is the same data the command renders, so a
/// selector and a command can never disagree.
#[test]
fn the_clipboard_selector_tree_matches_the_command() {
    let backend = MockClipboard::seat();
    backend
        .set_content(smart_lib::domains::clipboard::types::ClipboardContent {
            mime_type: "text/plain".into(),
            data: b"selector@1".to_vec(),
        })
        .unwrap();
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@clipboard/content/text").unwrap();
    assert_eq!(resolve(root, &sel), Value::String("selector@1".to_string()));

    // And `@clipboard/history` is pipable through the language.
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@clipboard/history").unwrap();
    match resolve(root, &sel) {
        Value::List(items) => assert_eq!(items.len(), 1),
        other => panic!("expected the history list, got {other:?}"),
    }
}

/// The JSON rendering is contract-shaped over the owned value tree.
#[test]
fn json_output_is_contract_shaped() {
    let backend = MockClipboard::seat();
    let value = evaluate(&cmd(&["clipboard", "get"]), &backend, refused()).unwrap();
    assert_eq!(shell::value::to_json(&value), "null");
}
