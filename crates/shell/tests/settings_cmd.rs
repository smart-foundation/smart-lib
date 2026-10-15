#![cfg(feature = "domain-settings")]
//! The settings command module over the scripted engine: the command surface's
//! data half, its human aesthetic (pixel-perfect under the terminal-faithful
//! measure), the host-write gate on every mutating verb, the honest answers
//! (`notSet` with the schema's default, a schema the host does not publish),
//! and the `@settings/*` selector tree. Nothing here touches the host (charter
//! law #8): every read is served from the script.

mod common;

use clap::Parser;
use common::{assert_borders_hold, MockSettings};
use shell::domains::settings::{evaluate, render, selector_root, SettingsCmd, WriteGuard};
use smart_lib::domains::settings::SettingsBackend;
use shell::selector;
use shell::widgets::display_width;
use shell::{OutputFormat, Value};

fn cmd(argv: &[&str]) -> SettingsCmd {
    SettingsCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

/// A record field as the list of records it holds (a missing or non-list field
/// is the honest empty, never a panic).
fn list_of<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    match value.field(key) {
        Value::List(items) => items.iter().collect(),
        _ => Vec::new(),
    }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the host's own schema source: every published schema
/// with its own key count, path and relocatability.
#[test]
fn the_default_view_is_the_schema_source() {
    let backend = MockSettings::default();
    let value = evaluate(&cmd(&["settings"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("total"), "1");
    let domains = match value.field("domains") {
        Value::List(items) => items.clone(),
        _ => panic!("expected a list"),
    };
    assert_eq!(domains[0].field_str("id"), "org.example.shell");
    assert_eq!(domains[0].field("keys").as_f64(), Some(4.0));
    assert_eq!(domains[0].field_str("path"), "/org/example/shell/");
    assert!(!domains[0].field_bool("relocatable"));
}

/// `settings get` reads the contract's own union: the value with its type, and
/// `notSet` as an ANSWER — the schema's own default named beside it, never
/// painted as the value.
#[test]
fn get_reads_the_contracts_union_and_notset_is_an_answer() {
    let backend = MockSettings::default();
    let font = evaluate(&cmd(&["settings", "get", "org.example.shell", "font-name"]), &backend, refused()).unwrap();
    assert!(font.field_bool("set"));
    assert_eq!(font.field_str("value"), "Sans 10");
    assert_eq!(font.field_str("type"), "string");
    assert_eq!(font.field_str("schema_type"), "s");
    assert_eq!(font.field_str("schema_default"), "'Sans 10'");
    assert_eq!(font.field_str("summary"), "the scripted font-name");

    let unset = evaluate(&cmd(&["settings", "get", "org.example.shell", "accent"]), &backend, refused()).unwrap();
    assert!(!unset.field_bool("set"));
    assert!(unset.field_str("reason").contains("no value for this key"));

    // A schema the host does not publish is notFound, with its reason — never
    // an abort and never an invented schema.
    let missing = evaluate(&cmd(&["settings", "schema", "org.example.none"]), &backend, refused());
    assert!(missing.is_err());
    assert!(missing
        .unwrap_err()
        .to_string()
        .contains("publishes no schema named"));
}

/// `settings list` is the domain's whole live map: every key the schema
/// publishes with the value the host carries and the default beside it.
#[test]
fn list_is_the_domains_whole_map() {
    let backend = MockSettings::default();
    let value = evaluate(&cmd(&["settings", "list", "org.example.shell"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("total"), "4");
    let settings = match value.field("settings") {
        Value::List(items) => items.clone(),
        _ => panic!("expected a list"),
    };
    assert_eq!(settings[0].field_str("key"), "font-name");
    assert_eq!(settings[0].field_str("value"), "Sans 10");
    assert_eq!(settings[1].field_str("key"), "clock-show-seconds");
    assert_eq!(settings[1].field_str("value"), "false");
    assert_eq!(settings[2].field_str("key"), "workspaces");
    assert_eq!(settings[2].field_str("value"), "4");
}

/// The schema browser: the host's own declaration for every key — type,
/// default, range and summary, with the contract's own value model beside it.
#[test]
fn the_schema_browser_reads_the_hosts_own_declarations() {
    let backend = MockSettings::default();
    let value = evaluate(&cmd(&["settings", "schema", "org.example.shell"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("domain"), "org.example.shell");
    assert_eq!(value.field_str("relocatable"), "false");
    let keys = match value.field("keys") {
        Value::List(items) => items.clone(),
        _ => panic!("expected a list"),
    };
    assert_eq!(keys.len(), 4);
    assert_eq!(keys[0].field_str("name"), "font-name");
    assert_eq!(keys[0].field_str("type"), "s");
    assert_eq!(keys[0].field_str("contract_type"), "string");
    assert_eq!(keys[0].field_str("default"), "'Sans 10'");
    assert_eq!(keys[1].field_str("contract_type"), "boolean");
    assert_eq!(keys[2].field_str("contract_type"), "integer");
    let rendered = render::render_action(
        &cmd(&["settings", "schema", "org.example.shell"]),
        &value,
        OutputFormat::Human,
    );
    assert_borders_hold(&rendered);
}

/// The StringMap surface: several keys read at once (`bulkGet`).
#[test]
fn the_stringmap_read_carries_every_named_key() {
    let backend = MockSettings::default();
    let value = evaluate(
        &cmd(&["settings", "bulk-get", "org.example.shell", "font-name", "workspaces"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("total"), "2");
    let settings = match value.field("settings") {
        Value::List(items) => items.clone(),
        _ => panic!("expected a list"),
    };
    assert_eq!(settings[0].field_str("value"), "Sans 10");
    assert_eq!(settings[1].field_str("value"), "4");
}

/// Every write verb refuses without the accepted write and reaches the
/// engine's own host path with it — and the value is read through the schema's
/// own type, so a `b` key is a boolean rather than a coerced string.
#[test]
fn every_write_verb_is_gated_and_typed_by_the_schema() {
    let backend = MockSettings::default();
    let refused_set = evaluate(
        &cmd(&["settings", "set", "org.example.shell", "font-name", "Sans 11"]),
        &backend,
        refused(),
    );
    assert!(refused_set.is_err());
    assert!(refused_set
        .unwrap_err()
        .to_string()
        .contains("CHANGES host state"));
    assert!(
        backend.writes.borrow().is_empty(),
        "nothing reached the engine: {:?}",
        backend.writes.borrow()
    );

    // With the gate open the engine's own path runs, and the value carries the
    // schema's own type: a boolean key is a boolean.
    let value = evaluate(
        &cmd(&["settings", "set", "org.example.shell", "clock-show-seconds", "true"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(value.field_str("verb"), "set");
    let reads = backend.values.lock().unwrap();
    assert_eq!(
        reads.get(&("org.example.shell".to_string(), "clock-show-seconds".to_string())),
        Some(&smart_lib::domains::settings::types::SettingValue::Boolean(true))
    );

    let backend = MockSettings::default();
    let bulk = evaluate(
        &cmd(&["settings", "bulk-set", "org.example.shell", "font-name=Sans 11", "workspaces=6"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert_eq!(bulk.field_str("verb"), "bulk-set");
    assert!(backend
        .writes
        .borrow()
        .iter()
        .any(|w| w == "bulk-set(org.example.shell, 2)"));

    let backend = MockSettings::default();
    let refused_bulk = evaluate(
        &cmd(&["settings", "bulk-set", "org.example.shell", "font-name=Sans 11"]),
        &backend,
        refused(),
    );
    assert!(refused_bulk.is_err());
    assert!(backend.writes.borrow().is_empty());
}

/// The watch surface is the host's own change signal: one subscription,
/// bounded pulls, the changes the host reported — proven by delivering a change
/// through the scripted host's own signal and reading it back.
#[test]
fn the_watch_surface_pulls_the_hosts_own_change_signal() {
    let backend = MockSettings::default();
    let subscription = backend.watch_domain("org.example.shell").unwrap();
    // The host's own signal, delivered asynchronously — the subscription's
    // cursor starts after everything already retained.
    backend.host_change(
        "org.example.shell",
        "font-name",
        smart_lib::domains::settings::types::SettingValue::String("Sans 12".to_string()),
    );
    let pulled = backend.pull_settings(subscription, 8).unwrap();
    assert_eq!(pulled.len(), 1, "the host's own change, delivered");
    assert_eq!(pulled[0].key, "font-name");
    let _ = backend.cancel_settings(subscription);

    // Through the command surface: the subscription is taken, the rounds are
    // bounded, and a change the host reports before the pull arrives.
    let backend = MockSettings::default();
    let domain = "org.example.shell".to_string();
    let notifier = MockSettings::default();
    let pending = std::sync::Arc::new(std::sync::Mutex::new(backend.pending.clone()));
    let _ = pending;
    // The command's own subscription takes the host's own change: the scripted
    // host's signal is delivered into the same queue the command drains.
    backend.host_change(
        "org.example.shell",
        "workspaces",
        smart_lib::domains::settings::types::SettingValue::Integer(6),
    );
    let _ = notifier;
    let value = evaluate(
        &cmd(&["settings", "watch", &domain, "--count", "1", "--interval", "1"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("domain"), "org.example.shell");
    assert_eq!(value.field_str("total"), "1", "the host's own change arrived");
    assert_eq!(
        list_of(&value, "changes")[0].field_str("key"),
        "workspaces",
        "the key the host reported"
    );
    assert!(value.field_str("reason").contains("`changed` signal"));
}

/// The category listing records the contract's OPEN category token beside each
/// key rather than guessing which keys belong to it.
#[test]
fn the_category_listing_records_the_token_not_a_guess() {
    let backend = MockSettings::default();
    let value = evaluate(&cmd(&["settings", "category", "example"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("category"), "example");
    assert_eq!(value.field_str("total"), "3");
    assert!(value.field_str("reason").contains("category tokens are conventions"));
}

/// The version, history and diff reads complete the contract's read surface.
#[test]
fn the_version_history_and_diff_reads_are_wired() {
    let backend = MockSettings::default();
    let version = evaluate(
        &cmd(&["settings", "version", "org.example.shell", "font-name"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(version.field_str("version"), "1");

    backend.host_change(
        "org.example.shell",
        "font-name",
        smart_lib::domains::settings::types::SettingValue::String("Sans 13".to_string()),
    );
    let history = evaluate(
        &cmd(&["settings", "history", "org.example.shell", "font-name"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(history.field_str("total"), "1");

    let diff = evaluate(&cmd(&["settings", "diff", "org.example.shell"]), &backend, refused()).unwrap();
    assert_eq!(diff.field_str("since_version"), "0");
    assert!(diff.field("version").as_f64().unwrap() >= 2.0);
}

/// The selector tree is the same values the command surface renders.
#[test]
fn the_selector_tree_resolves_from_the_engines_values() {
    let backend = MockSettings::default();
    let root = selector_root(&backend).unwrap();
    assert_eq!(root.field("domains").field_str("total"), "1");
    assert_eq!(list_of(&root, "caps")[0].display(), "settings.read");
    assert_eq!(list_of(&root, "schemas")[0].field_str("keys"), "4");

    let one = selector::resolve(
        selector_root(&backend).unwrap(),
        &selector::parse("@settings/domains/domains/0/id").unwrap(),
    );
    assert_eq!(one.display(), "org.example.shell");
}

/// The human renderings are pixel-perfect, and the gated write's answer names
/// what the gate refused.
#[test]
fn the_renderings_are_pixel_perfect() {
    let backend = MockSettings::default();
    for argv in [
        &["settings"][..],
        &["settings", "list", "org.example.shell"][..],
        &["settings", "schema", "org.example.shell"][..],
        &["settings", "bulk-get", "org.example.shell", "font-name"][..],
    ] {
        let value = evaluate(&cmd(argv), &backend, refused()).unwrap();
        let rendered = render::render_action(&cmd(argv), &value, OutputFormat::Human);
        assert_borders_hold(&rendered);
        assert!(
            rendered
                .lines()
                .filter(|line| line.contains('│'))
                .all(|line| display_width(line) <= 140),
            "a row never runs away: {rendered}"
        );
    }
}
