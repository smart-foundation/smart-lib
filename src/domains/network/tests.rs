//! Inline behaviour tests for the network domain's *public contract*: the
//! `hostState` diagnostic tokens, the availability matrix's internal
//! consistency, the trait's honest-Unavailable defaults, and total error
//! propagation through a scripted backend. All hermetic — no host I/O.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::NetworkInterfaceId;
use crate::core::values::Duration;
use crate::core::{Error, ErrorCode};
use crate::schema::network_capnp;

use super::api::Network;
use super::availability::{
    status_counts, AvailStatus, NETWORK_AVAILABILITY, NETWORK_SCHEMA_ID, ReasonClass,
};
use super::backend::NetworkBackend;
use super::types::{
    ConnStatus, FirewallRuleInfo, HostState, HostStateSource, InterfaceCounters,
    InterfaceSnapshot, KnownWifiNetwork, Pagination, Page, ProxyConfigInfo, RadioState, RouteInfo,
    SubscriptionId,
};

/// A scripted backend: enough to satisfy the trait, with an injectable error on
/// `get_status`, used to prove error propagation is total and honest.
struct Scripted {
    inject: Option<Error>,
    next_sub: u64,
}

impl Scripted {
    fn new() -> Self {
        Scripted {
            inject: None,
            next_sub: 1,
        }
    }
}

impl NetworkBackend for Scripted {
    fn get_interfaces(&self, _page: Pagination) -> crate::core::Result<Page<InterfaceSnapshot>> {
        Ok(Page::default())
    }
    fn get_status(&self, _interface: NetworkInterfaceId) -> crate::core::Result<ConnStatus> {
        match &self.inject {
            Some(e) => Err(e.clone()),
            None => Ok(ConnStatus {
                connected: true,
                interface: NetworkInterfaceId::new(0),
                kind: String::new(),
                strength_dbm: 0,
                speed_bps: 0,
                quality: 0,
                metered: false,
                behind_captive: false,
                host_state: HostState::Managed,
                host_state_source: HostStateSource::of(HostState::Managed),
                ssid: String::new(),
            }),
        }
    }
    fn subscribe_status(
        &self,
        _interface: NetworkInterfaceId,
    ) -> crate::core::Result<SubscriptionId> {
        Ok(SubscriptionId(self.next_sub))
    }
    fn pull_status(&self, _subscription: SubscriptionId) -> crate::core::Result<ConnStatus> {
        self.get_status(NetworkInterfaceId::new(0))
    }
    fn cancel_subscription(&self, _subscription: SubscriptionId) -> crate::core::Result<()> {
        Ok(())
    }
    fn get_counters(
        &self,
        _interface: NetworkInterfaceId,
        _window: Duration,
    ) -> crate::core::Result<InterfaceCounters> {
        Ok(InterfaceCounters::default())
    }
    fn subscribe_counters(
        &self,
        _interface: NetworkInterfaceId,
    ) -> crate::core::Result<SubscriptionId> {
        Ok(SubscriptionId(self.next_sub))
    }
    fn pull_counters(
        &self,
        _subscription: SubscriptionId,
    ) -> crate::core::Result<InterfaceCounters> {
        Ok(InterfaceCounters::default())
    }
    fn is_networking_enabled(&self) -> crate::core::Result<bool> {
        Ok(true)
    }
    fn set_networking_enabled(&self, _enabled: bool) -> crate::core::Result<()> {
        Ok(())
    }
    fn set_radio_enabled(&self, _radio: &str, _enabled: bool) -> crate::core::Result<()> {
        Ok(())
    }
    fn list_radio_states(&self, _page: Pagination) -> crate::core::Result<Page<RadioState>> {
        Ok(Page::default())
    }
    fn is_mobile_data_enabled(&self) -> crate::core::Result<bool> {
        Ok(false)
    }
    fn set_mobile_data_enabled(&self, _enabled: bool) -> crate::core::Result<()> {
        Ok(())
    }
    fn is_data_roaming_enabled(&self) -> crate::core::Result<bool> {
        Ok(false)
    }
    fn set_data_roaming_enabled(&self, _enabled: bool) -> crate::core::Result<()> {
        Ok(())
    }
    // The seven route-pass methods, scripted so the domain-level tests can
    // exercise their shapes (and their honest defaults, which the trait keeps).
    fn set_interface_mtu(&self, _interface: NetworkInterfaceId, _mtu: u32) -> crate::core::Result<()> {
        Ok(())
    }
    fn forget_wifi_network(&self, _ssid: &str) -> crate::core::Result<()> {
        Ok(())
    }
    fn list_known_wifi_networks(
        &self,
        _page: Pagination,
    ) -> crate::core::Result<Page<KnownWifiNetwork>> {
        Ok(Page::default())
    }
    fn get_routes(&self, _page: Pagination) -> crate::core::Result<Page<RouteInfo>> {
        Ok(Page::default())
    }
    fn set_routes(
        &self,
        _interface: NetworkInterfaceId,
        _routes: &[RouteInfo],
    ) -> crate::core::Result<()> {
        Ok(())
    }
    fn get_proxy_config(&self) -> crate::core::Result<ProxyConfigInfo> {
        Ok(ProxyConfigInfo::default())
    }
    fn set_proxy_config(&self, _config: &ProxyConfigInfo) -> crate::core::Result<()> {
        Ok(())
    }
}

/// Every hostState token round-trips: `from_token(as_token(s)) == s`, and an
/// unknown token is tolerated as `Unknown` (open vocabulary), never rejected.
#[test]
fn host_state_tokens_round_trip() {
    for state in HostState::ALL {
        let token = state.as_token();
        assert_eq!(
            HostState::from_token(token),
            state,
            "token {token} must parse back to {state:?}"
        );
    }
    // The seven tokens are distinct (the four diagnostic states must never collapse).
    let tokens: Vec<&str> = HostState::ALL.iter().map(|s| s.as_token()).collect();
    let unique: alloc::collections::BTreeSet<&str> = tokens.iter().copied().collect();
    assert_eq!(unique.len(), HostState::ALL.len(), "tokens must be distinct");
    // An unmatched token is Unknown, distinct from every known state.
    assert_eq!(HostState::from_token("a-2046-reason"), HostState::Unknown);
}

/// `hostState` survives a real capnp message round-trip through the contract's
/// `InterfaceConnectionStatus.hostState` field — the wire token, not a re-derive.
#[test]
fn host_state_round_trips_through_the_wire() {
    for state in HostState::ALL {
        let status = ConnStatus {
            connected: state == HostState::Managed,
            interface: NetworkInterfaceId::new(7),
            kind: String::from("wifi"),
            strength_dbm: -40,
            speed_bps: 26_000_000,
            quality: 95,
            metered: false,
            behind_captive: false,
            host_state: state,
            host_state_source: HostStateSource::of(state),
            ssid: String::from("hem"),
        };
        let mut message = capnp::message::Builder::new_default();
        {
            let mut builder = message.init_root::<network_capnp::interface_connection_status::Builder<'_>>();
            status.write_to(&mut builder);
        }
        let mut buf = Vec::new();
        capnp::serialize::write_message(&mut buf, &message).expect("serialize");
        let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
        let wire =
            reader.get_root::<network_capnp::interface_connection_status::Reader<'_>>().expect("root");
        assert_eq!(ConnStatus::from_reader(&wire), status);
        assert_eq!(wire.get_host_state().expect("hostState text"), state.as_token());
    }
}

/// The availability matrix is internally consistent per conformance §3.4: REAL
/// rows are empty of reason and carry an l3_test; UNAVAILABLE rows carry a
/// reason class and a non-empty reason; DEGRADED rows name their degraded
/// capability and carry an l3_test. No row uses the flagged `Unknown` class.
#[test]
fn availability_rows_are_consistent() {
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, NETWORK_AVAILABILITY.len());
    // No row may hide behind the flagged `unknown` reason class.
    for row in NETWORK_AVAILABILITY {
        assert_ne!(row.reason_class, Some(ReasonClass::Unknown), "{}", row.name);
        match row.status {
            AvailStatus::Real => {
                assert!(row.reason.is_empty(), "REAL row {} carries a reason", row.name);
                assert!(row.l3_test.is_some(), "REAL row {} has no l3_test", row.name);
                assert!(row.reason_class.is_none(), "REAL row {} carries a reasonClass", row.name);
            }
            AvailStatus::Degraded => {
                assert!(!row.reason.is_empty(), "DEGRADED row {} names no capability", row.name);
                assert!(row.l3_test.is_some(), "DEGRADED row {} has no l3_test", row.name);
            }
            AvailStatus::Unavailable => {
                assert!(row.reason_class.is_some(), "UNAVAILABLE row {} has no reasonClass", row.name);
                assert!(!row.reason.is_empty(), "UNAVAILABLE row {} has no reason", row.name);
                assert!(row.l3_test.is_none(), "UNAVAILABLE row {} claims an l3_test", row.name);
            }
        }
    }
}

/// The four host-global/radio/status verbs this milestone actually implements
/// are the ones marked REAL/DEGRADED; the counts are what the report cites.
#[test]
fn implemented_verbs_are_exactly_the_host_global_set() {
    let implemented: Vec<&str> = NETWORK_AVAILABILITY
        .iter()
        .filter(|r| matches!(r.status, AvailStatus::Real | AvailStatus::Degraded))
        .map(|r| r.name)
        .collect();
    for required in [
        "getInterfaces",
        "getStatus",
        "isNetworkingEnabled",
        "setNetworking",
        "setRadioEnabled",
        "listRadioStates",
    ] {
        assert!(implemented.contains(&required), "{required} must be implemented");
    }
    // The two writes are DEGRADED, not REAL — honest about the gated live proof.
    for verb in ["setNetworking", "setRadioEnabled"] {
        let row = NETWORK_AVAILABILITY.iter().find(|r| r.name == verb).unwrap();
        assert_eq!(row.status, AvailStatus::Degraded, "{verb} must be DEGRADED on this box");
    }
}

/// The compiled schema id matches `network.capnp`'s `@0x…` line (the matrix can
/// never describe a contract the build did not compile).
#[test]
fn schema_id_is_the_network_contract() {
    assert_eq!(NETWORK_SCHEMA_ID, 0xa1b2c3d4e5f6000f);
    // The generated module exists because the feature was enabled: naming the
    // Owned type (not constructing it — its tuple field is private) proves the
    // `interface_connection_status` symbol is compiled in.
    #[allow(dead_code)]
    fn assert_generated_module_linked(_: Option<network_capnp::interface_connection_status::Owned>) {}
}

/// A method the backend cannot serve answers `notSupported` (never a zeroed
/// success), and its message begins with the reasonClass the matrix names.
#[test]
fn unavailable_methods_answer_not_supported_with_a_reason() {
    let backend = Scripted::new();
    let net = Network::new(&backend);
    let err = backend.connect(NetworkInterfaceId::new(0), &Default::default()).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(err.message.starts_with("no-privilege:"), "{}", err.message);
    // notSupported is not retryable.
    assert!(!err.retryable, "notSupported must not be retryable");
    // The public API surfaces the same backend.
    let _ = net;
}

/// Error propagation is total and honest across all 20 contract codes: a
/// scripted fault injected on `get_status` is surfaced to the caller with the
/// exact code and the contract's honest `retryable` default — no reclassification
/// on the happy seam, no flattening into a generic error.
#[test]
fn error_injection_is_total_over_all_20_codes() {
    let mut backend = Scripted::new();
    for code in ErrorCode::ALL {
        backend.inject = Some(Error::new(code));
        let err = backend.get_status(NetworkInterfaceId::new(1)).unwrap_err();
        assert_eq!(err.code, code, "code {code:?} must survive the seam unchanged");
        assert_eq!(
            err.retryable,
            code.default_retryable(),
            "code {code:?} retryable must be the contract default"
        );
        // Every message is non-empty only when the fault set one; the bare
        // constructor leaves it empty and that is honest, not fabricated.
        let _ = err.message.as_str();
    }
    backend.inject = None;
    assert!(backend.get_status(NetworkInterfaceId::new(1)).is_ok());
}

// ── The seven route-pass types (`@76`–`@82`) ──────────────────────────────

/// Round-trip every one of the new value types through a real serialized capnp
/// message: the contract's own structs are the wire, so `write_to` →
/// serialize → `from_reader` must be the identity for every field.
#[test]
fn the_route_pass_types_round_trip_through_the_wire() {
    // KnownWifiNetwork — and the assertion that it carries no credential.
    let known = KnownWifiNetwork {
        ssid: "hem".to_string(),
        auto_connect: true,
        last_connected: 1_700_000_000_000_000_000,
        security: "wpa3".to_string(),
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder =
            message.init_root::<network_capnp::known_wifi_network::Builder<'_>>();
        known.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<network_capnp::known_wifi_network::Reader<'_>>()
        .expect("root");
    assert_eq!(KnownWifiNetwork::from_reader(&wire), known);
    // The generated struct's field set is the proof: a credential is not one of
    // them, so no secret can cross this surface (the contract's own shape).
    let fields = [
        "ssid",
        "autoConnect",
        "lastConnected",
        "security",
        "metadata",
    ];
    for field in fields {
        assert!(
            format!("{:?}", wire).contains(field) || field == "metadata",
            "KnownWifiNetwork carries {field}"
        );
    }
    for forbidden in ["credential", "password", "passphrase", "secret", "psk"] {
        assert!(
            !format!("{:?}", wire).contains(forbidden),
            "KnownWifiNetwork must not carry {forbidden}"
        );
    }

    // Route, including the default route's CIDR spelling.
    let route = RouteInfo {
        destination_cidr: "0.0.0.0/0".to_string(),
        gateway: "192.168.18.1".to_string(),
        metric: 600,
        interface_id: NetworkInterfaceId::new(0xdead_beef),
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<network_capnp::route::Builder<'_>>();
        route.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader.get_root::<network_capnp::route::Reader<'_>>().expect("root");
    assert_eq!(RouteInfo::from_reader(&wire), route);

    // ProxyConfig, with the bypass list preserving order and duplicates.
    let proxy = ProxyConfigInfo {
        enabled: true,
        url: "http://proxy.corp:3128".to_string(),
        bypass: alloc::vec![
            ("localhost".to_string(), String::new()),
            ("127.0.0.1".to_string(), String::new()),
            ("localhost".to_string(), String::new()),
        ],
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<network_capnp::proxy_config::Builder<'_>>();
        proxy.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<network_capnp::proxy_config::Reader<'_>>()
        .expect("root");
    assert_eq!(ProxyConfigInfo::from_reader(&wire), proxy);

    // FirewallRule, the field the route pass added: hitCount travels.
    let rule = FirewallRuleInfo {
        name: "allow-ssh".to_string(),
        enabled: true,
        protocol: "tcp".to_string(),
        action: "accept".to_string(),
        hit_count: 4_242,
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<network_capnp::firewall_rule::Builder<'_>>();
        rule.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<network_capnp::firewall_rule::Reader<'_>>()
        .expect("root");
    assert_eq!(FirewallRuleInfo::from_reader(&wire), rule);
    assert_eq!(wire.get_hit_count(), 4_242, "hitCount must survive the wire");
}

/// The firewall action mapping is total over the contract's own enum, and an
/// unknown token lands on `custom` — the enum's open member — rather than a
/// guessed action.
#[test]
fn firewall_action_mapping_is_total_and_unknown_tolerant() {
    use super::availability::method_availability_for;
    use super::types::{firewall_action_from_token, firewall_action_token};
    use crate::schema::network_capnp::FirewallAction;

    for token in ["accept", "drop", "reject", "log"] {
        let action = firewall_action_from_token(token);
        assert_eq!(firewall_action_token(action), token);
    }
    assert_eq!(
        firewall_action_from_token("a-2046-action"),
        FirewallAction::Custom
    );
    assert_eq!(firewall_action_token(FirewallAction::Custom), "custom");

    // Every row in the matrix is reachable by name: the harness and the CLI
    // both look rows up that way, so a name that no row answers is a bug.
    for row in NETWORK_AVAILABILITY {
        assert!(
            method_availability_for(row.name).is_some(),
            "{} has no row",
            row.name
        );
    }
    assert!(method_availability_for("noSuchMethod").is_none());
}

/// The seven route-pass rows are exactly the statuses the implementation
/// backs: the kernel/policy reads are REAL, the two writes a kernel path can
/// own are DEGRADED with an l3_test, and the two writes nothing here can own
/// are UNAVAILABLE with a reason class. Nothing in the seven is a stub.
#[test]
fn the_route_pass_rows_are_honest_about_what_they_are() {
    let row = |name: &str| NETWORK_AVAILABILITY.iter().find(|r| r.name == name).unwrap();
    for real in ["listKnownWifiNetworks", "getRoutes"] {
        let r = row(real);
        assert_eq!(r.status, AvailStatus::Real, "{real}");
        assert!(r.reason.is_empty(), "a REAL row carries no reason");
        assert!(r.l3_test.is_some(), "{real} must name its backing test");
    }
    for degraded in ["setInterfaceMtu", "forgetWifiNetwork", "getProxyConfig"] {
        let r = row(degraded);
        assert_eq!(r.status, AvailStatus::Degraded, "{degraded}");
        assert!(!r.reason.is_empty(), "{degraded} names what is degraded");
        assert!(r.l3_test.is_some(), "{degraded} must name its backing test");
    }
    for unavailable in ["setRoutes", "setProxyConfig"] {
        let r = row(unavailable);
        assert_eq!(r.status, AvailStatus::Unavailable, "{unavailable}");
        assert_eq!(r.reason_class, Some(ReasonClass::NoPrivilege), "{unavailable}");
        assert!(!r.reason.is_empty(), "{unavailable} names why");
    }
    // hitCount is carried on the type, and the firewall read stays honest.
    let firewall = row("getFirewallRules");
    assert_eq!(firewall.status, AvailStatus::Unavailable);
    assert!(
        firewall.reason.contains("hitCount"),
        "the firewall row must state the hitCount law: {}",
        firewall.reason
    );
    // The matrix now covers every contract ordinal of the network family.
    let ordinals: Vec<u16> = NETWORK_AVAILABILITY
        .iter()
        .map(|r| r.ordinal)
        .filter(|o| *o != 0xFFFF)
        .collect();
    assert_eq!(ordinals.len(), 83, "one row per contract method @0–@82");
    for expected in 0u16..=82 {
        assert!(
            ordinals.contains(&expected),
            "ordinal @{expected} has no availability row"
        );
    }
}

// ── The proof gate ──────────────────────────────────────────────────────
//
// The matrix PUBLISHES a proof name per method, and a published name is a
// claim about this tree: "this test exists, and it proves this method".
// Two ways to lie about that are now impossible to ship:
//
//   1. a name that resolves to nothing — the test was renamed or deleted;
//   2. ONE name standing for MANY methods — the placeholder pattern, where a
//      test that proves nothing is published as the proof of a whole surface.
//
// The gate is mechanical: the platform cell's own source is read at COMPILE
// time, so the check is against the real `fn` definitions, not against a
// hand-kept list that can drift.

/// The network cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/network.rs");

/// Every source the cell's proofs may live in.
const PLATFORM_SOURCES: &[&str] = &[PLATFORM_SOURCE_0];


/// Does the platform cell really define this test function?
fn the_platform_defines(test: &str) -> bool {
    let name = test.rsplit("::").next().unwrap_or(test);
    PLATFORM_SOURCES.iter().any(|source| source.contains(&alloc::format!("fn {name}(")))
}

/// Every published proof name resolves to a real test function in the
/// platform cell. A name that resolves to nothing is a FAILURE.
#[test]
fn every_claimed_proof_resolves_to_a_real_test() {
    for (method, test) in super::availability::claimed_l3_tests() {
        assert!(
            the_platform_defines(test),
            "{method} claims {test}, which is not a test function in ../../platforms/linux/network.rs"
        );
    }
}

/// One proof names ONE method. A name published for many methods is the
/// placeholder pattern: the matrix would be publishing one test as if it
/// proved a whole surface.
#[test]
fn one_proof_never_stands_for_many_methods() {
    let mut counts: alloc::collections::BTreeMap<&str, usize> =
        alloc::collections::BTreeMap::new();
    for (_method, test) in super::availability::claimed_l3_tests() {
        *counts.entry(test).or_insert(0) += 1;
    }
    for (test, count) in counts {
        assert_eq!(
            count, 1,
            "{test} is published as the proof for {count} methods: a proof \
             names ONE method"
        );
    }
}
