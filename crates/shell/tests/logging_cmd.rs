#![cfg(feature = "domain-logging")]
//! The logging command module over the scripted engine: the command surface's
//! data half, its human aesthetic (pixel-perfect under the terminal-faithful
//! measure), the host-write gate on the emit/level/sampling/clear verbs, the
//! per-sink honesty rendered as measured reasons, and the `@logging/*`
//! selector tree. Nothing here touches the host (charter law #8): every read is
//! served from the script.

mod common;

use clap::Parser;
use common::{assert_borders_hold, MockLogging};
use shell::domains::logging::{evaluate, render, selector_root, LoggingCmd, WriteGuard};
use shell::selector;
use smart_lib::domains::logging::LoggingBackend;
use shell::widgets::display_width;
use shell::{OutputFormat, Value};

fn cmd(argv: &[&str]) -> LoggingCmd {
    LoggingCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the sink table: every sink the host has, with the
/// measured verdict for each — the readable ones named and the root-only ones
/// named with the host's own error words.
#[test]
fn the_default_view_is_the_sink_table() {
    let backend = MockLogging::default();
    let value = evaluate(&cmd(&["logging"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("total"), "2");
    assert_eq!(value.field_str("readable"), "1");
    let sinks = match value.field("sinks") {
        Value::List(items) => items.clone(),
        _ => panic!("expected a list"),
    };
    assert_eq!(sinks[0].field_str("domain"), "Xorg.0.log");
    assert!(sinks[0].field_bool("readable"));
    assert!(sinks[0].field_str("reason").is_empty());
    assert!(!sinks[1].field_bool("readable"));
    assert_eq!(
        sinks[1].field_str("reason"),
        "/run/runit/supervise.sshd-log: Permission denied (os error 13)"
    );
    assert!(value.field_str("reason").contains("no journald"));

    let rendered = render::render_action(&cmd(&["logging"]), &value, OutputFormat::Human);
    assert_borders_hold(&rendered);
    // The reason column is bounded like every other cell, so the host's own
    // words are drawn up to the cut and the cut is marked.
    assert!(rendered.contains("Permissi…"), "{rendered}");
    assert!(rendered.contains("✗ no"), "the unreadable sink is named, not skipped: {rendered}");
}

/// `logging tail` reads one sink's own lines; a root-only sink is the host's
/// own refusal, named — never a silent skip.
#[test]
fn tail_reads_the_sinks_own_lines_and_names_a_refusal() {
    let backend = MockLogging::default();
    let value = evaluate(&cmd(&["logging", "tail", "Xorg.0.log", "--lines", "2"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("total"), "2");
    let entries = match value.field("entries") {
        Value::List(items) => items.clone(),
        _ => panic!("expected a list"),
    };
    assert_eq!(entries[0].field_str("message"), "the scripted host booted");

    let refused = evaluate(&cmd(&["logging", "tail", "service:sshd"]), &backend, refused());
    assert!(refused.is_err());
    let reason = refused.unwrap_err().to_string();
    assert!(reason.contains("not readable by this caller"), "{reason}");
    assert!(reason.contains("Permission denied"), "{reason}");
}

/// The filter surface speaks the contract's own severity vocabulary, and the
/// default minimum is the wire reading of an absent `minLevel` — trace, never
/// info (which would silently drop the debug entry nobody asked to drop).
#[test]
fn the_filter_surface_speaks_the_contracts_vocabulary() {
    let backend = MockLogging::default();
    let all = evaluate(&cmd(&["logging", "entries"]), &backend, refused()).unwrap();
    assert_eq!(all.field_str("total"), "4", "trace is the default bound");
    assert_eq!(all.field_str("min_level"), "trace");

    let warned = evaluate(&cmd(&["logging", "entries", "--level", "warn"]), &backend, refused()).unwrap();
    assert_eq!(warned.field_str("total"), "2", "warn and error");
    assert_eq!(warned.field_str("min_level"), "warn");

    let filtered = evaluate(
        &cmd(&["logging", "entries", "--level", "error", "--domain", "app", "--pattern", "service"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(filtered.field_str("total"), "1");

    // The vocabulary is the contract's own eleven levels; an unknown token is
    // refused rather than guessed at a severity.
    let bad = evaluate(&cmd(&["logging", "entries", "--level", "verbose"]), &backend, refused());
    assert!(bad.is_err());
    assert!(bad.unwrap_err().to_string().contains("a severity is"));
}

/// `logging follow` is the contract's own subscription: one token, bounded
/// pulls, and the entries the backend reported.
#[test]
fn follow_is_the_contracts_own_subscription() {
    let backend = MockLogging::default();
    let value = evaluate(
        &cmd(&["logging", "follow", "--count", "2", "--interval", "1"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("total"), "0", "nothing was logged in the rounds");
    assert!(value.field_str("reason").contains("LogSubscription"));

    // After the host's own write, the subscription delivers it.
    let backend = MockLogging::default();
    backend
        .log(common::log_types_of::info(), "app", "the scripted host started")
        .unwrap();
    let value = evaluate(
        &cmd(&["logging", "follow", "--count", "1", "--interval", "1"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("total"), "1");
    assert_eq!(
        list_of(&value, "entries")[0].field_str("message"),
        "the scripted host started"
    );
    assert!(backend
        .writes
        .borrow()
        .iter()
        .any(|w| w.starts_with("cancel(")));
}

/// Every host-touching verb refuses without the accepted write and reaches the
/// engine's own path with it — and the refusal names what it would touch.
#[test]
fn the_host_touching_verbs_are_gated() {
    let backend = MockLogging::default();
    for argv in [
        &["logging", "emit", "info", "a message"][..],
        &["logging", "set-level", "app", "warn"][..],
        &["logging", "clear", "app"][..],
    ] {
        let refused = evaluate(&cmd(argv), &backend, refused());
        assert!(refused.is_err(), "{} ran without the accepted write", argv.join(" "));
        assert!(refused
            .unwrap_err()
            .to_string()
            .contains("CHANGES host state"));
    }
    // A sampling rate is gated too, and its refusal names what it changes.
    let sampling = evaluate(&cmd(&["logging", "sampling", "app", "--set", "0.5"]), &backend, refused());
    assert!(sampling.is_err());
    assert!(sampling
        .unwrap_err()
        .to_string()
        .contains("what this backend retains"));
    assert!(
        backend.writes.borrow().is_empty(),
        "nothing reached the engine: {:?}",
        backend.writes.borrow()
    );

    // With the gate open the engine's own path runs.
    let emitted = evaluate(
        &cmd(&["logging", "emit", "warn", "--domain", "app", "a", "message"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(emitted.field_str("outcome"), "accepted");
    assert_eq!(emitted.field_str("message"), "a message");
    assert!(backend
        .writes
        .borrow()
        .iter()
        .any(|w| w == "log(warn, app)"));

    let level = evaluate(&cmd(&["logging", "set-level", "app", "warn"]), &backend, allowed()).unwrap();
    assert_eq!(level.field_str("verb"), "set-level");
    let read_back = evaluate(&cmd(&["logging", "level", "app"]), &backend, refused()).unwrap();
    assert_eq!(read_back.field_str("level"), "warn");
}

/// The sampling read is the contract's own declared rate: a producer that will
/// drop half its debug entries says so instead of a consumer discovering it.
#[test]
fn the_sampling_read_is_the_declared_rate() {
    let backend = MockLogging::default();
    backend.set_sampling_rate("app", 0.5).unwrap();
    let value = evaluate(&cmd(&["logging", "sampling", "app"]), &backend, refused()).unwrap();
    assert_eq!(value.field("sampling_rate").as_f64(), Some(0.5));
}

/// The export surface carries the retained entries in the caller's OPEN content
/// type, rendered by the backend.
#[test]
fn the_export_surface_carries_the_entries() {
    let backend = MockLogging::default();
    let value = evaluate(&cmd(&["logging", "export", "--mime", "text/plain"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("mime_type"), "text/plain");
    assert!(value.field("bytes").as_f64().unwrap() > 0.0);
    assert!(value.field_str("text").contains("a service refused a start"));
    assert!(backend.writes.borrow().iter().any(|w| w == "export(text/plain)"));
}

/// The selector tree is the same values the command surface renders.
#[test]
fn the_selector_tree_resolves_from_the_engines_values() {
    let backend = MockLogging::default();
    let root = selector_root(&backend).unwrap();
    assert_eq!(root.field("sinks").field_str("total"), "2");
    assert_eq!(root.field("domains").field_str("total"), "3");
    assert_eq!(list_of(&root, "caps")[0].display(), "logging.write");

    let sinks = selector::resolve(
        selector_root(&backend).unwrap(),
        &selector::parse("@logging/sinks/sinks").unwrap(),
    );
    match sinks {
        Value::List(items) => assert_eq!(items.len(), 2),
        other => panic!("expected a list, got {other:?}"),
    }
}

/// A record field as the list of records it holds (a missing or non-list field
/// is the honest empty, never a panic).
fn list_of<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    match value.field(key) {
        Value::List(items) => items.iter().collect(),
        _ => Vec::new(),
    }
}

/// The human renderings are pixel-perfect.
#[test]
fn the_renderings_are_pixel_perfect() {
    let backend = MockLogging::default();
    for argv in [
        &["logging"][..],
        &["logging", "domains"][..],
        &["logging", "entries"][..],
        &["logging", "tail", "Xorg.0.log"][..],
    ] {
        let value = evaluate(&cmd(argv), &backend, refused()).unwrap();
        let rendered = render::render_action(&cmd(argv), &value, OutputFormat::Human);
        assert!(
            rendered
                .lines()
                .filter(|line| line.contains('│'))
                .all(|line| display_width(line) <= 140),
            "a row never runs away: {rendered}"
        );
    }
    let value = evaluate(&cmd(&["logging"]), &backend, refused()).unwrap();
    assert_borders_hold(&render::render_action(&cmd(&["logging"]), &value, OutputFormat::Human));
}
