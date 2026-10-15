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
    assert!(
        matches!(err, shell::SmartError::NotFound(ref reason) if reason.contains("wwan9")),
        "the notFound answer carries its reason: {err:?}"
    );
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

/// An active scan routes to the engine and carries the real visible networks:
/// SSID / signal / security — the three the founder asked to finally see.
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

    // A real scan returns the visible networks with signal and security.
    let mut backend = MockBackend::host();
    backend.scan = vec![common_wifi("Big Boy Soweto", -40, "open")];
    let value = evaluate(&cmd(&["network", "wifi", "scan"]), &backend, refused()).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].field_str("ssid"), "Big Boy Soweto");
            assert_eq!(items[0].field_str("security"), "open");
            assert_eq!(items[0].field_str("strength_dbm"), "-40");
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

fn common_wifi(ssid: &str, dbm: i32, security: &str) -> smart_lib::domains::network::types::WifiNetwork {
    use smart_lib::domains::network::types::{MacAddress, WifiNetwork};
    WifiNetwork {
        ssid: ssid.to_string(),
        bssid: MacAddress("AA:BB:CC:DD:EE:FF".to_string()),
        strength_dbm: dbm,
        frequency_hz: 2_437_000,
        security: security.to_string(),
        hidden: false,
    }
}

/// The network NAME reaches the interface detail — the founder's exact
/// complaint ("there is no network name anywhere").
#[test]
fn the_interface_detail_carries_the_connected_ssid() {
    let backend = MockBackend::host();
    let value = evaluate(&cmd(&["network", "interface", "wlp0s20f3"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("ssid"), "VR-3245-0884");
    // A wired interface carries no name.
    let wired = evaluate(&cmd(&["network", "interface", "enp0s31f6"]), &backend, refused()).unwrap();
    assert!(wired.field_str("ssid").is_empty());
    // The human render prints it (the screen a user reads).
    let rendered = shell::domains::network::render::render_action(
        &cmd(&["network", "interface", "wlp0s20f3"]),
        &value,
        OutputFormat::Human,
    );
    let plain = strip_ansi(&rendered);
    assert!(plain.contains("Network") && plain.contains("VR-3245-0884"), "{plain}");
}

/// `network wifi privilege` returns the MEASURED verdict — the truth that
/// replaced the assumed "polkit-gated" label.
#[test]
fn wifi_privilege_reports_the_measured_verdict() {
    let mut backend = MockBackend::host();
    backend.wifi_privilege = Some(smart_lib::domains::network::types::WifiWritePrivilege {
        network_control: "yes".to_string(),
        modify_connection: "no".to_string(),
        measured: true,
        uid: 1000,
        detail: "uid 1000 → network-control=yes, settings.modify.system=no".to_string(),
    });
    let value = evaluate(&cmd(&["network", "wifi", "privilege"]), &backend, refused()).unwrap();
    assert!(value.field_bool("measured"));
    assert_eq!(value.field_str("network_control"), "yes");
    assert_eq!(value.field_str("modify_connection"), "no");
    // An engine that cannot reach the authority is honestly unmeasured.
    let cold = MockBackend::host();
    assert!(evaluate(&cmd(&["network", "wifi", "privilege"]), &cold, refused()).is_err());
}

/// `network wifi join` decodes a `WIFI:` payload and shows the resolved engine
/// call WITHOUT performing it (the dry-run); with --force it performs. The
/// password never appears in the resolved call — only a secret reference.
#[test]
fn wifi_join_decodes_and_shows_the_resolved_call() {
    let backend = MockBackend::host();
    // Dry-run: parse + show, perform nothing.
    let value = evaluate(
        &cmd(&["network", "wifi", "join", "WIFI:T:WPA;S:Big Boy Soweto;P:hunter2;;"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(value.field_str("ssid"), "Big Boy Soweto");
    assert_eq!(value.field_str("security"), "WPA");
    assert!(!value.field_bool("performed"), "a dry-run performs nothing");
    let resolved = value.field_str("resolved_call");
    assert!(resolved.contains("wifiConnect(ssid=\"Big Boy Soweto\""), "{resolved}");
    assert!(resolved.contains("SecretKeyId("), "{resolved}");
    assert!(!resolved.contains("hunter2"), "the password must never appear: {resolved}");
    assert!(backend.writes().is_empty(), "no host write performed");

    // With --force the real engine call is issued (mock-verified).
    let value = evaluate(
        &cmd(&["network", "wifi", "join", "WIFI:T:WPA;S:Matrix;P:secret;;"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert!(value.field_bool("performed"));
    assert!(backend.seams().contains(&"wifi_connect(Matrix)".to_string()));
}

/// `network wifi share` builds a scannable payload whose round-trip reads back
/// to the exact payload — the proof a phone will join the right network.
#[test]
fn wifi_share_renders_a_qr_that_reads_back() {
    let backend = MockBackend::host();
    let value = evaluate(&cmd(&["network", "wifi", "share", "VR-3245-0884"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("ssid"), "VR-3245-0884");
    assert!(value.field_bool("round_trip_ok"), "the rendered QR decodes to the payload");
    let payload = value.field_str("payload");
    assert_eq!(value.field_str("round_trip"), payload);
    assert!(payload.starts_with("WIFI:T:nopass;S:VR-3245-0884;"), "{payload}");
}

/// A correct CSI skipper strips ANSI escapes but keeps the text between them —
/// the width measure the report runs against, so a false drift cannot be
/// manufactured by a hand-rolled stripper.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ , intermediates, a final byte in @..~
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
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
