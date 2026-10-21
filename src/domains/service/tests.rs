//! Inline behaviour tests for the service domain's *public contract*: the
//! owned types' fidelity to the wire, the availability matrix's internal
//! consistency, the trait's honest-Unavailable totality (every method has a
//! `reasonClass`-prefixed reason when a concrete host cannot serve it), the
//! presented-capability scope rules, and the runtime gate. The native-path
//! behaviour over scripted supervision trees is proven in
//! [`crate::platforms::linux::service`]. All hermetic — no host I/O.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::{discovery_capnp, service_capnp};

use super::availability::{
    claimed_l3_tests, runtime_gate, status_counts, AvailStatus, ReasonClass, SERVICE_AVAILABILITY,
    SERVICE_SCHEMA_ID,
};
use super::backend::{PresentedCapability, ServiceBackend, ServiceControl};
use super::types::{
    DependencyType, HealthState, LogLevel, Metadata, Page, Pagination, Quota, ServiceDependency,
    ServiceEvent, ServiceEventPayload, ServiceHealth, ServiceHealthCheck, ServiceInstanceInfo,
    ServiceInstanceState, ServiceMetrics, ServiceResourceAllocation, ServiceStatus, SubscriptionId,
    SupervisionCapabilities,
};

/// Every method name the frozen contract declares on the two interfaces this
/// domain implements, in contract order. The matrix must carry exactly these
/// names (the subscription helpers and the lifecycle bridge are additions the
/// matrix documents separately).
const SERVICE_METHODS: [&str; 14] = [
    "listServices",
    "getService",
    "getState",
    "subscribeState",
    "getHealth",
    "listHealth",
    "listQuarantined",
    "resolve",
    "getDependencies",
    "getResourceAllocation",
    "getHealthCheck",
    "getMetrics",
    "subscribeServiceEvents",
    "control",
];

/// The seven `ServiceControl` contract methods, in contract order.
const CONTROL_METHODS: [&str; 7] = [
    "setDependencies",
    "setResourceAllocation",
    "setHealthCheck",
    "updateConfig",
    "setQuarantine",
    "setLogLevel",
    "clearMetrics",
];

/// A backend that overrides nothing: it is the *honest default* every platform
/// the trait does not directly target becomes. Its whole job is to prove that
/// every method degrades with a reason, never a panic and never a fabricated
/// answer.
struct HonestBackend;

impl ServiceBackend for HonestBackend {}

impl super::backend::ServiceLifecycle for HonestBackend {}

/// A test control reference: every verb is the contract's own honest
/// `notSupported` (a host supervisor owns no registry surface).
struct HonestControl;

impl ServiceControl for HonestControl {}

/// A representative state, round-tripped through the wire by several tests.
fn sample_state() -> ServiceInstanceState {
    ServiceInstanceState {
        status: ServiceStatus::Active,
        main_pid: 7_341,
        since: 1_769_000_000_000_000_000,
        since_us: 4_200_000_000,
        since_valid: true,
        quarantined: false,
        reason: String::new(),
        metadata: vec![
            ("supervisor".to_string(), "runit".to_string()),
            ("stateFile".to_string(), "/run/runit/supervise.sshd/stat".to_string()),
        ],
    }
}

/// The instance state round-trips through a real serialized message: the
/// status enum, the pid, `since`/`sinceUs`/`sinceValid`, and the ordered
/// metadata pairs — a client reading the wire sees exactly what the backend
/// measured.
#[test]
fn the_instance_state_round_trips_through_the_wire() {
    let state = sample_state();
    let mut message = capnp::message::Builder::new_default();
    let mut builder = message.init_root::<service_capnp::service_instance_state::Builder<'_>>();
        state.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<service_capnp::service_instance_state::Reader<'_>>()
        .expect("root");
    let back = ServiceInstanceState::from_reader(&wire);

    assert_eq!(back, state);
    assert!(back.since_valid);
    assert_eq!(back.metadata.len(), 2, "the metadata order is preserved");
    assert_eq!(back.metadata[0].0, "supervisor");
}

/// `ServiceInstanceInfo` round-trips too — including the state it carries.
#[test]
fn the_instance_info_round_trips_through_the_wire() {
    let info = ServiceInstanceInfo {
        id: crate::core::ids::ServiceId::new(3),
        name: "sshd".to_string(),
        description: "OpenBSD secure shell daemon".to_string(),
        state: sample_state(),
        enabled: true,
        on_demand: false,
        manifest: 0,
    };
    let mut message = capnp::message::Builder::new_default();
    let mut builder = message.init_root::<service_capnp::service_instance_info::Builder<'_>>();
        info.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<service_capnp::service_instance_info::Reader<'_>>()
        .expect("root");
    let back = ServiceInstanceInfo::from_reader(&wire);

    assert_eq!(back, info);
}

/// `D.ServiceHealth`, `D.ServiceDependency`, `ServiceResourceAllocation`,
/// `ServiceHealthCheck`, `ServiceMetrics` and `ServiceEvent` all round-trip —
/// every state struct a consumer reads, faithful to the wire.
#[test]
fn every_state_struct_round_trips_through_the_wire() {
    let health = ServiceHealth {
        service_id: crate::core::ids::ServiceId::new(9),
        state: HealthState::Online,
        reason: String::new(),
        quarantined: false,
        since_us: 12_000,
        since_valid: true,
        last_check_passed: false,
        last_check_at: 0,
        fault_count: 0,
        consecutive_faults: 0,
        restarts: 1,
    };
    let mut message = capnp::message::Builder::new_default();
    let mut builder = message.init_root::<discovery_capnp::service_health::Builder<'_>>();
            health.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<discovery_capnp::service_health::Reader<'_>>()
        .expect("root");
    let back = ServiceHealth::from_reader(&wire);

    assert_eq!(back, health);
    assert_eq!(back.state, HealthState::Online);
    // The probe is honestly declared unrun: a zero lastCheckAt, never a
    // fabricated passing check.
    assert_eq!(back.last_check_at, 0);

    let dep = ServiceDependency {
        service_id: crate::core::ids::ServiceId::new(4),
        dependency_type: DependencyType::Required,
        version_constraint: String::new(),
        satisfied: true,
    };
    let mut message = capnp::message::Builder::new_default();
    let mut builder = message.init_root::<discovery_capnp::service_dependency::Builder<'_>>();
            dep.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<discovery_capnp::service_dependency::Reader<'_>>()
        .expect("root");
    let back = ServiceDependency::from_reader(&wire);

    assert_eq!(back, dep);

    let allocation = ServiceResourceAllocation {
        quota: Quota {
            memory_bytes: 512 * 1024 * 1024,
            cpu_percent: 12.5,
            io_read_bytes_per_sec: 0,
            io_write_bytes_per_sec: 0,
            pids: 16,
            wall_clock_budget_ns: 0,
        },
    };
    let mut message = capnp::message::Builder::new_default();
    let mut builder =
                message.init_root::<service_capnp::service_resource_allocation::Builder<'_>>();
            allocation.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<service_capnp::service_resource_allocation::Reader<'_>>()
        .expect("root");
    let back = ServiceResourceAllocation::from_reader(&wire);

    assert_eq!(back.quota.memory_bytes, 512 * 1024 * 1024);
    assert_eq!(back.quota.pids, 16);

    let check = ServiceHealthCheck {
        endpoint: "/etc/sv/sshd/check".to_string(),
        interval_ns: 5_000_000_000,
        timeout_ns: 1_000_000_000,
        unhealthy_threshold: 3,
        healthy_threshold: 1,
    };
    let mut message = capnp::message::Builder::new_default();
    let mut builder = message.init_root::<service_capnp::service_health_check::Builder<'_>>();
            check.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<service_capnp::service_health_check::Reader<'_>>()
        .expect("root");
    let back = ServiceHealthCheck::from_reader(&wire);

    assert_eq!(back.endpoint, "/etc/sv/sshd/check");
    assert_eq!(back.interval_ns, 5_000_000_000);
    assert_eq!(back.unhealthy_threshold, 3);

    let metrics = ServiceMetrics {
        service_id: crate::core::ids::ServiceId::new(11),
        window_ns: 60_000_000_000,
        request_count: 1_024,
        error_count: 3,
        latency_p50_ns: 2_000_000,
        latency_p95_ns: 40_000_000,
        latency_p99_ns: 90_000_000,
        active_requests: 2,
        partial: false,
    };
    let mut message = capnp::message::Builder::new_default();
    let mut builder = message.init_root::<service_capnp::service_metrics::Builder<'_>>();
            metrics.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<service_capnp::service_metrics::Reader<'_>>()
        .expect("root");
    let back = ServiceMetrics::from_reader(&wire);

    assert_eq!(back.request_count, 1_024);
    assert_eq!(back.latency_p99_ns, 90_000_000);

    let event = ServiceEvent {
        payload: ServiceEventPayload::StateChanged(sample_state()),
        timestamp: 1_769_000_000_000_000_000,
        service_id: crate::core::ids::ServiceId::new(12),
    };
    let mut message = capnp::message::Builder::new_default();
    let mut builder = message.init_root::<service_capnp::service_event::Builder<'_>>();
            event.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<service_capnp::service_event::Reader<'_>>()
        .expect("root");
    let back = ServiceEvent::from_reader(&wire);

    match &back.payload {
        ServiceEventPayload::StateChanged(state) => assert_eq!(state.status, ServiceStatus::Active),
        other => panic!("expected the state arm, got {other:?}"),
    }
    assert_eq!(back.event_type(), super::types::ServiceEventType::Started);

    // A config event carries the ordered map; the unknown arm stays unnameable.
    let config = ServiceEvent {
        payload: ServiceEventPayload::ConfigChanged(vec![("key".to_string(), "value".to_string())]),
        timestamp: 1,
        service_id: crate::core::ids::ServiceId::new(13),
    };
    let mut message = capnp::message::Builder::new_default();
    let mut builder = message.init_root::<service_capnp::service_event::Builder<'_>>();
            config.write_to(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<service_capnp::service_event::Reader<'_>>()
        .expect("root");
    let back = ServiceEvent::from_reader(&wire);

    match &back.payload {
        ServiceEventPayload::ConfigChanged(map) => assert_eq!(map[0].1, "value"),
        other => panic!("expected the config arm, got {other:?}"),
    }
}

/// `T.VoidResult` is the mutating half's wire shape: success or a `T.Error`,
/// and the error's own code and message are preserved verbatim (never a
/// default).
#[test]
fn void_result_carries_a_real_error_when_one_is_handed_over() {
    let error = Error::not_supported("no-backend: a host supervisor owns no registry surface");
    let mut message = capnp::message::Builder::new_default();
    let mut builder =
                    message.init_root::<crate::schema::types_capnp::void_result::Builder<'_>>();
            super::types::write_void_error(&mut builder, &error);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<crate::schema::types_capnp::void_result::Reader<'_>>()
        .expect("root");
    let back = match wire.which() {
        Ok(crate::schema::types_capnp::void_result::WhichReader::Error(Ok(e))) => {
            Error::from_reader(&e)
        }
        _ => panic!("expected the error arm"),
    };

    assert_eq!(back.code, ErrorCode::NotSupported);
    assert!(back.message.contains("no-backend:"), "{back}");

    // And success is a first-class arm, not a defaulted error.
    let mut message = capnp::message::Builder::new_default();
    let mut builder =
                message.init_root::<crate::schema::types_capnp::void_result::Builder<'_>>();
            super::types::write_void_success(&mut builder);
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<crate::schema::types_capnp::void_result::Reader<'_>>()
        .expect("root");
    let ok = matches!(
        wire.which(),
        Ok(crate::schema::types_capnp::void_result::WhichReader::Success(()))
    );

    assert!(ok, "a success arm is a success");
}

/// The availability matrix is internally consistent: one row per contract
/// method on both interfaces, unique ordinals, every REAL/DEGRADED row names
/// its backing `l3_test`, every row resolves by name, and the counts add up. A
/// REAL claim with a vanished row is a FAIL.
#[test]
fn the_availability_matrix_is_internally_consistent() {
    assert_eq!(SERVICE_SCHEMA_ID, 0xa1b2c3d4e5f60021);

    // Every method the frozen contract declares has exactly one row.
    for method in SERVICE_METHODS.iter().chain(CONTROL_METHODS.iter()) {
        let rows: Vec<&str> = SERVICE_AVAILABILITY
            .iter()
            .filter(|r| r.name == *method)
            .map(|r| r.name)
            .collect();
        assert_eq!(rows.len(), 1, "{method} must appear exactly once (got {rows:?})");
    }

    // Ordinals are unique across the matrix (the subscription helpers and the
    // lifecycle bridge carry documented non-contract ordinals).
    let mut ordinals: Vec<u16> = SERVICE_AVAILABILITY.iter().map(|r| r.ordinal).collect();
    let total = ordinals.len();
    ordinals.sort_unstable();
    ordinals.dedup();
    assert_eq!(ordinals.len(), total, "every row carries a unique ordinal");

    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, SERVICE_AVAILABILITY.len());
    // A host supervisor cannot serve the whole contract: the honest shape of
    // this domain has real rows, degraded rows and unavailable rows, and the
    // counts below prove none of the three is empty (an all-unavailable matrix
    // is as dishonest as an all-available one).
    assert!(real > 0, "the native observation surface is real");
    assert!(degraded > 0, "the degraded-with-reason rows are named");
    assert!(unavailable > 0, "the honest absences are named");

    for row in SERVICE_AVAILABILITY {
        let resolved = super::availability::method_availability_for(row.name).map(|r| r.name);
        assert_eq!(resolved, Some(row.name), "{} resolves by name", row.name);
        match row.status {
            AvailStatus::Real => {
                assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
                assert!(row.reason.is_empty(), "{} is REAL", row.name);
            }
            AvailStatus::Degraded => {
                assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
                assert!(!row.reason.is_empty(), "{} names what is degraded", row.name);
                assert!(row.reason_class.is_none());
            }
            AvailStatus::Unavailable => {
                assert!(row.reason_class.is_some(), "{} carries a class", row.name);
                assert!(!row.reason.is_empty(), "{} says why", row.name);
                assert!(row.l3_test.is_none());
            }
        }
    }
    assert!(method_availability_for_check("noSuchMethod").is_none());
    assert!(claimed_l3_tests().all(|(_, t)| !t.is_empty()), "every claim names a test");
}

/// Shorthand so the assertion above reads as the invariant it states.
fn method_availability_for_check(name: &str) -> Option<&'static super::availability::MethodAvailability> {
    super::availability::method_availability_for(name)
}

/// The reason-class grammar is the CLI's own (re-used, not a second vocabulary).
#[test]
fn reason_class_tokens_are_the_cli_grammar() {
    assert_eq!(ReasonClass::NoBackend.as_str(), "no-backend");
    assert_eq!(ReasonClass::NoPrivilege.as_str(), "no-privilege");
    assert_eq!(ReasonClass::NotApplicable.as_str(), "not-applicable");
    assert_eq!(ReasonClass::DisabledByPolicy.as_str(), "disabled-by-policy");
}

/// Error totality (law #6, law #3): every method of a backend that overrides
/// nothing degrades with a non-empty reason and the right code — never a panic,
/// never a fabricated answer. This is the whole "honest Unavailable everywhere
/// else" surface in one test.
#[test]
fn every_method_degrades_with_a_reason_on_a_backend_that_serves_nothing() {
    let b = HonestBackend;

    // The reads that MUST NOT pretend: no supervisor reached is `unavailable`
    // (the condition could change), while a concept that does not exist is
    // `notSupported` (it never will).
    let err = b.list_services(Pagination { offset: 0, limit: u32::MAX }).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(err.message.starts_with("no-backend:"), "{err}");
    assert!(!err.retryable, "Unavailable is not transient");

    let err = b.get_state(crate::core::ids::ServiceId::new(1)).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(err.message.starts_with("no-backend:"), "{err}");

    let err = b
        .get_metrics(crate::core::ids::ServiceId::new(1), 1_000_000_000)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(err.message.starts_with("not-applicable:"), "{err}");

    for (method, result) in [
        ("subscribeState", b.subscribe_state(crate::core::ids::ServiceId::new(1)).map(|_| ())),
        ("pullState", b.pull_state(SubscriptionId(1), 1).map(|_| ())),
        ("cancelState", b.cancel_state(SubscriptionId(1))),
        ("subscribeServiceEvents", b.subscribe_service_events(crate::core::ids::ServiceId::new(1)).map(|_| ())),
        ("pullServiceEvents", b.pull_service_events(SubscriptionId(1), 1).map(|_| ())),
        ("cancelServiceEvents", b.cancel_service_events(SubscriptionId(1))),
        ("getDependencies", b.get_dependencies(crate::core::ids::ServiceId::new(1)).map(|_| ())),
        ("getResourceAllocation", b.get_resource_allocation(crate::core::ids::ServiceId::new(1)).map(|_| ())),
        ("getHealthCheck", b.get_health_check(crate::core::ids::ServiceId::new(1)).map(|_| ())),
    ] {
        let err = result.expect_err(method);
        assert_eq!(err.code, ErrorCode::NotSupported, "{method}: {err}");
        assert!(!err.message.is_empty(), "{method} carries its reason");
    }

    // The control accessor refuses rather than minting an authority the host
    // never granted (law #5).
    let err = match b.control(PresentedCapability {
        target: "*".to_string(),
        ..Default::default()
    }) {
        Ok(_) => panic!("a backend with no control surface must not mint a reference"),
        Err(e) => e,
    };
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.contains("permission-denied:"), "{err}");

    // `resolve`'s absent answer and `listQuarantined`'s honest empty are
    // first-class results, not faults.
    assert_eq!(b.resolve("nope").unwrap(), (crate::core::ids::ServiceId::new(0), false));
    assert_eq!(b.list_quarantined(Pagination::default()).unwrap(), Page::default());

    // Every ServiceControl verb answers with a reason.
    let control = HonestControl;
    for result in [
        control.set_dependencies(crate::core::ids::ServiceId::new(1), &[]),
        control.set_resource_allocation(
            crate::core::ids::ServiceId::new(1),
            super::types::ServiceResourceAllocation::default(),
        ),
        control.set_health_check(
            crate::core::ids::ServiceId::new(1),
            super::types::ServiceHealthCheck::default(),
        ),
        control.update_config(crate::core::ids::ServiceId::new(1), &Metadata::new()),
        control.set_quarantine(crate::core::ids::ServiceId::new(1), true, "test"),
        control.set_log_level(crate::core::ids::ServiceId::new(1), LogLevel::Info),
        control.clear_metrics(crate::core::ids::ServiceId::new(1)),
    ] {
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported, "{err}");
        assert!(!err.message.is_empty(), "a notSupported carries its reason");
    }

    // The lifecycle bridge is honest about having no native control path.
    let err = super::backend::ServiceLifecycle::apply_lifecycle(
        &b,
        crate::core::ids::ServiceId::new(1),
        "sshd",
        super::backend::LifecycleVerb::Start,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(err.message.starts_with("no-backend:"), "{err}");

    // The caps are the honest empty: a supervisor nothing can reach.
    assert_eq!(b.supervision_caps(), SupervisionCapabilities::default());
    assert_eq!(b.supervisor().supervisor, "");
    let _ = b.last_unavailable_reason("getState");
}

/// Every one of the 20 contract error codes has a `From`/name mapping that is
/// total and its retryable flag is the contract default — the model the
/// domain's errors stand on.
#[test]
fn error_codes_carry_honest_defaults() {
    for code in ErrorCode::ALL {
        let err = Error::new(code);
        assert_eq!(err.code, code);
        assert_eq!(err.retryable, code.default_retryable());
        assert!(err.message.is_empty(), "a bare code has no message");
    }
    // The two honest degradation constructors REQUIRE a reason.
    let e = Error::unavailable("reason");
    assert!(!e.message.is_empty());
    let names: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(names.len(), 20);
}

/// The presented capability's scope rules are the contract's own: `"*"` covers
/// every service, a service's name covers that service, and anything else
/// covers nothing (an over-broad read of an authority is the defect the
/// authority design exists to stop).
#[test]
fn a_presented_capability_covers_exactly_its_scope() {
    let all = PresentedCapability {
        target: "*".to_string(),
        operations: vec!["service.control".to_string()],
        ..Default::default()
    };
    assert!(all.covers("sshd"));
    assert!(all.covers("anything-else"));
    assert!(all.grants("service.control"));
    assert!(!all.grants("service.start"), "an ungranted operation is not granted");

    let one = PresentedCapability {
        target: "sshd".to_string(),
        ..Default::default()
    };
    assert!(one.covers("sshd"));
    assert!(!one.covers("dbus"), "a narrow grant is not a broad one");

    let none = PresentedCapability::default();
    assert!(!none.covers("sshd"), "an empty target covers nothing");
}

/// The runtime gate is computed from the measured caps: a backend that can read
/// the supervisor's state pays no gate, and one that cannot reports the exact
/// class the matrix's reason promises — so the runtime answer and the table can
/// never disagree.
#[test]
fn the_runtime_gate_follows_the_measured_caps() {
    let open = SupervisionCapabilities {
        supervisor: "runit".to_string(),
        can_enumerate: true,
        can_read_state: true,
        // The exact half is separately measured: a runit tree whose
        // supervise files are readable has it, a root-only one does not.
        can_read_exact_state: true,
        can_control: true,
    };
    for method in ["listServices", "getService", "getState", "getHealth", "listHealth"] {
        assert_eq!(runtime_gate(&open, method), None, "{method} is ungated when readable");
        let gated = runtime_gate(&SupervisionCapabilities { can_read_state: false, ..open.clone() }, method);
        let (class, reason) = gated.expect("a state read gates when the state is unreadable");
        assert_eq!(class, ReasonClass::NoPrivilege);
        assert!(reason.starts_with("no-privilege:"), "{reason}");
    }
    for method in ["start", "stop", "restart"] {
        let gated = runtime_gate(&SupervisionCapabilities { can_control: false, ..open.clone() }, method);
        let (class, reason) = gated.expect("a lifecycle verb gates when control is unwritable");
        assert_eq!(class, ReasonClass::NoPrivilege);
        assert!(reason.starts_with("no-privilege:"), "{reason}");
    }
    // A method the caps do not gate is never gated by them.
    assert_eq!(runtime_gate(&open, "resolve"), None);
    assert_eq!(runtime_gate(&open, "getMetrics"), None);
}

/// The pagination rule is the contract-wide one: a limit of 0 means "one",
/// never "all".
#[test]
fn pagination_zero_limit_means_one() {
    assert_eq!(Pagination { offset: 0, limit: 0 }.effective_limit(), 1);
    assert_eq!(Pagination { offset: 5, limit: 10 }.effective_limit(), 10);
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

/// The service cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/service/tests.rs");
/// The service cell's own source, read at compile time.
const PLATFORM_SOURCE_1: &str = include_str!("../../platforms/linux/service/openrc.rs");
/// The service cell's own source, read at compile time.
const PLATFORM_SOURCE_2: &str = include_str!("../../platforms/linux/service/systemd.rs");

/// Every source the cell's proofs may live in.
const PLATFORM_SOURCES: &[&str] = &[PLATFORM_SOURCE_0, PLATFORM_SOURCE_1, PLATFORM_SOURCE_2];


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
            "{method} claims {test}, which is not a test function in ../../platforms/linux/service/tests.rs, ../../platforms/linux/service/openrc.rs, ../../platforms/linux/service/systemd.rs"
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
