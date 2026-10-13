//! The network command module over the scripted engine: the command
//! surface's data half, its rendering, and the control verbs — the old
//! shell's fake arms (`wifi on/off` printing without calling) are real
//! engine calls now, verified here against the mock.

mod common;

use clap::Parser;
use common::MockBackend;
use shell::domains::network::{evaluate, run, NetworkCmd, WriteGuard};
use shell::{OutputFormat, Value};

fn cmd(argv: &[&str]) -> NetworkCmd {
    NetworkCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

// ── the read surface ───────────────────────────────────────────────────────

#[test]
fn the_default_view_is_the_interface_table() {
    let backend = MockBackend::host();
    let value = evaluate(&cmd(&["network"]), &backend, refused()).unwrap();
    match &value {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            // Bug 1's lock on the command surface too: the wifi interface is
            // typed wifi, never a defaulted ethernet.
            let wifi = items
                .iter()
                .find(|i| i.field_str("name") == "wlp0s20f3")
                .expect("the wifi interface");
            assert_eq!(wifi.field_str("type"), "wifi");
            assert!(wifi.field_bool("connected"));
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

#[test]
fn status_carries_networking_radios_and_hoststates() {
    let backend = MockBackend::host();
    let value = evaluate(&cmd(&["network", "status"]), &backend, refused()).unwrap();
    assert!(value.field_bool("networking"));
    match value.field("radios") {
        Value::List(radios) => {
            assert_eq!(radios.len(), 2);
            let wifi = radios.iter().find(|r| r.field_str("name") == "wifi").unwrap();
            assert!(wifi.field_bool("enabled"));
            assert!(!wifi.field_bool("hard_blocked"), "hard and soft are separate");
            assert!(!wifi.field_bool("soft_blocked"));
        }
        other => panic!("expected radios, got {other:?}"),
    }
    match value.field("interfaces") {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            let wifi = items
                .iter()
                .find(|i| i.field_str("name") == "wlp0s20f3")
                .unwrap();
            assert_eq!(wifi.field_str("host_state"), "managed");
        }
        other => panic!("expected interfaces, got {other:?}"),
    }
}

#[test]
fn one_interface_resolves_by_name_and_answers_its_type_and_hoststate() {
    let backend = MockBackend::host();
    let value = evaluate(&cmd(&["network", "interface", "wlp0s20f3"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("name"), "wlp0s20f3");
    assert_eq!(value.field_str("type"), "wifi");
    assert_eq!(value.field_str("host_state"), "managed");
    // An interface the host does not have is notFound — never a fabricated
    // record.
    let err = evaluate(&cmd(&["network", "interface", "wwan9"]), &backend, refused()).unwrap_err();
    assert!(matches!(err, shell::SmartError::NotFound));
}

/// The interface up/down arm the original faked: the backend exposes no such
/// verb, and the shell says so naming the switches that do exist.
#[test]
fn interface_up_down_is_honestly_not_wired() {
    let backend = MockBackend::host();
    let err = evaluate(&cmd(&["network", "interface", "wlp0s20f3", "up"]), &backend, refused())
        .unwrap_err();
    assert!(
        err.to_string().contains("not wired") && err.to_string().contains("setNetworking"),
        "the honest answer names what exists: {err}"
    );
}

#[test]
fn the_known_networks_read_is_real_and_an_empty_set_is_honest() {
    let mut backend = MockBackend::host();
    let value = evaluate(&cmd(&["network", "wifi", "list"]), &backend, refused()).unwrap();
    assert_eq!(value, Value::List(vec![]), "an honest empty");
    assert!(backend.seams().contains(&"list_known_wifi_networks".to_string()));

    backend.known = vec![common::KnownWifiNetwork {
        ssid: "matrix".to_string(),
        auto_connect: true,
        last_connected: 0,
        security: "wpa2".to_string(),
    }];
    let value = evaluate(&cmd(&["network", "wifi", "list"]), &backend, refused()).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("ssid"), "matrix");
            assert_eq!(items[0].field_str("security"), "wpa2");
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// An active scan routes to the engine, and the engine's refusal is the
/// answer — rendered, never papered over.
#[test]
fn wifi_scan_routes_to_the_engine_and_carries_its_answer() {
    let mut backend = MockBackend::host();
    backend.scan_error = true;
    let err = evaluate(&cmd(&["network", "wifi", "scan"]), &backend, refused()).unwrap_err();
    assert!(
        err.to_string().contains("notSupported") || err.to_string().contains("no-backend"),
        "the engine's own reason survives: {err}"
    );
    assert!(backend.seams().contains(&"wifi_scan".to_string()));
}

// ── the control verbs (the old fake arms, now real calls) ──────────────────

/// `wifi on` when the radio is already enabled is a READ: the shell says so
/// and writes nothing.
#[test]
fn wifi_on_when_already_on_writes_nothing() {
    let backend = MockBackend::host();
    let value = evaluate(&cmd(&["network", "wifi", "on"]), &backend, refused()).unwrap();
    assert!(
        value.field_str("result").contains("already"),
        "the read is the whole answer: {}",
        value.field_str("result")
    );
    assert!(backend.writes().is_empty(), "no host write");
}

/// `wifi off` when the radio is on is a real engine call — and it is gated:
/// without acceptance the engine is never called.
#[test]
fn wifi_off_is_a_real_gated_engine_call() {
    let backend = MockBackend::host();
    // Refused: the reason is the answer, and nothing is written.
    let err = evaluate(&cmd(&["network", "wifi", "off"]), &backend, refused()).unwrap_err();
    assert!(err.to_string().contains("CHANGES host state"), "{err}");
    assert!(backend.writes().is_empty());
    // Accepted: the real engine call is issued (mock-verified; the live
    // bring-back proof is the founder's — charter law #8).
    let value = evaluate(&cmd(&["network", "wifi", "off"]), &backend, allowed()).unwrap();
    assert!(value.field_str("result").contains("acknowledged"));
    assert_eq!(
        backend.writes(),
        vec!["set_radio_enabled(wifi, false)".to_string()],
        "the wire call is the engine's own verb"
    );
}

#[test]
fn wifi_connect_and_disconnect_route_to_the_engine() {
    let backend = MockBackend::host();
    let value = evaluate(
        &cmd(&["network", "wifi", "connect", "matrix", "hunter2"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert!(value.field_str("result").contains("matrix"));
    assert!(backend.seams().contains(&"wifi_connect(matrix)".to_string()));

    let value = evaluate(&cmd(&["network", "wifi", "disconnect"]), &backend, allowed()).unwrap();
    assert!(value.field_str("result").contains("acknowledged"));
    assert!(backend.seams().contains(&"wifi_disconnect".to_string()));
}

#[test]
fn wifi_forget_is_gated_and_deletes_the_profile() {
    let backend = MockBackend::host();
    let err = evaluate(&cmd(&["network", "wifi", "forget", "matrix"]), &backend, refused()).unwrap_err();
    assert!(err.to_string().contains("CHANGES host state"));
    assert!(backend.writes().is_empty());

    evaluate(&cmd(&["network", "wifi", "forget", "matrix"]), &backend, allowed()).unwrap();
    assert_eq!(
        backend.writes(),
        vec!["forget_wifi_network(matrix)".to_string()]
    );
}

// ── the machine forms ──────────────────────────────────────────────────────

#[test]
fn json_output_is_contract_shaped_and_escaped() {
    let backend = MockBackend::host();
    // `run` prints; capture the shape by evaluating + rendering directly.
    let value = evaluate(&cmd(&["network"]), &backend, refused()).unwrap();
    let json = shell::value::to_json(&value);
    assert!(json.starts_with('[') && json.ends_with(']'));
    assert!(json.contains("\"type\":\"wifi\""));
    // A quote in the data escapes rather than breaking the document.
    let tricky = Value::Object(vec![
        ("ssid".into(), Value::String("a\"b\\c\nd".to_string())),
    ]);
    let json = shell::value::to_json(&tricky);
    assert_eq!(json, "{\"ssid\":\"a\\\"b\\\\c\\nd\"}");
}

#[test]
fn raw_output_is_the_values_display_form() {
    let backend = MockBackend::host();
    let value = evaluate(&cmd(&["network"]), &backend, refused()).unwrap();
    let rendered = shell::domains::network::render::render_action(&cmd(&["network"]), &value, OutputFormat::Raw);
    assert!(rendered.contains("wlp0s20f3") && rendered.contains("wifi"));
}

#[test]
fn run_renders_the_human_form_and_returns_the_value() {
    let backend = MockBackend::host();
    let value = run(&cmd(&["network"]), &OutputFormat::Human, &backend, refused()).unwrap();
    assert!(matches!(value, Value::List(_)));
}
