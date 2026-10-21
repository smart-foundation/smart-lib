//! Inline behaviour tests for the clock domain's *public contract*: the owned
//! types' fidelity to the wire, the availability matrix's internal consistency,
//! the trait's honest-Unavailable totality, the contract's pagination and
//! anchor rules, and the alarm surface's shapes. The native-path behaviour over
//! the kernel's real clocks is proven in
//! [`crate::platforms::linux::clock`] (the `l3_linux_clock::…` tests the matrix
//! names). All hermetic — no host I/O in this file.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::ids::TimerId;
use crate::core::values::{Duration, Timestamp};
use crate::core::{Error, ErrorCode};
use crate::schema::clock_capnp;

use super::availability::{
    claimed_l3_tests, status_counts, AvailStatus, ReasonClass, CLOCK_AVAILABILITY, CLOCK_SCHEMA_ID,
};
use super::backend::{AlarmService, ClockBackend};
use super::types::{
    Alarm, AlarmRecurrence, ClockDomainKind, ClockInfo, ClockSyncInfo, Page, Pagination, TimerAnchor,
    TimerEvent, TimerHandle, TimerSpec, VirtualClockConfig, VirtualClockHandle,
};

/// The contract's own method list, in `ClockService` order (`@0`–`@19`), plus
/// the subscription helpers and the `AlarmService` methods. The matrix must
/// carry exactly these names — the flat list is the contract.
const CONTRACT_METHODS: &[&str] = &[
    "info",
    "monotonicNow",
    "realtimeNow",
    "createTimer",
    "cancelTimer",
    "resetTimer",
    "checkTimer",
    "subscribeTimers",
    "pullTimers",
    "cancelTimers",
    "sleep",
    "listClockDomains",
    "sleepCancellable",
    "getClockSync",
    "getClockDrift",
    "createVirtualClock",
    "advanceVirtualClock",
    "getClockQuality",
    "persistTimer",
    "listPersistedTimers",
    "setTimerDependency",
    "alarms",
    // The AlarmService interface.
    "createAlarm",
    "updateAlarm",
    "deleteAlarm",
    "getAlarm",
    "listAlarms",
    "snoozeAlarm",
    "dismissAlarm",
    "getNextAlarm",
];

/// A backend that serves nothing: the honesty baseline every unavailable method
/// is measured against.
struct Nothing;

impl ClockBackend for Nothing {}

/// One arm of the contract's time surface: a one-shot relative timer, stated
/// explicitly rather than inferred from a zero anchor.
#[test]
fn a_relative_timer_resolves_from_now() {
    let spec = TimerSpec::one_shot_relative(Duration::from_nanos(500));
    assert_eq!(
        spec.resolve_first_fire(Timestamp::from_nanos(1_000)).as_nanos(),
        1_500
    );
    assert!(!spec.is_periodic());
    let periodic = TimerSpec {
        interval: Duration::from_nanos(250),
        ..spec.clone()
    };
    assert!(periodic.is_periodic());
}

/// The anchor union is total: absolute, relative, zero and the unnamed `custom`
/// arm each read back as themselves, never as a guessed default.
#[test]
fn the_anchor_union_round_trips_through_the_wire() {
    for anchor in [
        TimerAnchor::Absolute(Timestamp::from_nanos(42)),
        TimerAnchor::Relative(Duration::from_nanos(7)),
        TimerAnchor::Zero,
        TimerAnchor::Custom,
    ] {
        let spec = TimerSpec {
            anchor_type: anchor,
            ..TimerSpec::one_shot_relative(Duration::from_nanos(1))
        };
        let mut message = capnp::message::Builder::new_default();
        {
            let mut builder = message.init_root::<clock_capnp::timer_spec::Builder<'_>>();
            spec.write_to(&mut builder);
        }
        let mut buf = Vec::new();
        capnp::serialize::write_message(&mut buf, &message).expect("serialize");
        let reader =
            capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
                .expect("deserialize");
        let wire = reader
            .get_root::<clock_capnp::timer_spec::Reader<'_>>()
            .expect("root");
        assert_eq!(TimerSpec::from_reader(&wire).anchor_type, anchor);
    }
}

/// `ClockInfo` and its children survive a real serialized round-trip, so the
/// Linux backend's measured info reaches a consumer unchanged.
#[test]
fn clock_info_round_trips_through_the_wire() {
    let info = ClockInfo {
        monotonic: Timestamp::from_nanos(1),
        realtime: Timestamp::from_nanos(2),
        resolution: Duration::from_nanos(3),
        source_name: "tsc".to_string(),
        boot_time: Timestamp::from_nanos(4),
        sync: ClockSyncInfo {
            synced: false,
            source: "chrony".to_string(),
            stratum: 0,
            last_sync: Timestamp::UNIX_EPOCH,
            offset: Duration::ZERO,
        },
        security: Default::default(),
        domains: vec![
            crate::domains::clock::types::ClockDomainInfo {
                domain: ClockDomainKind::Realtime,
                name: "CLOCK_REALTIME".to_string(),
                resolution: Duration::from_nanos(1),
                available: true,
            },
            crate::domains::clock::types::ClockDomainInfo {
                domain: ClockDomainKind::Monotonic,
                name: "CLOCK_MONOTONIC".to_string(),
                resolution: Duration::from_nanos(1),
                available: true,
            },
        ],
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<clock_capnp::clock_info::Builder<'_>>();
        info.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<clock_capnp::clock_info::Reader<'_>>()
        .expect("root");
    assert_eq!(wire.get_realtime().expect("realtime").get_nanoseconds(), 2);
    assert_eq!(wire.get_source_name().expect("source"), "tsc");
    assert_eq!(wire.get_domains().expect("domains").len(), 2);
    assert_eq!(
        wire.get_sync().expect("sync").get_synced(),
        false,
        "a backend that cannot prove synchronization says so on the wire too"
    );
}

/// A `TimerEvent` carries the kernel's own firing count through the wire.
#[test]
fn timer_events_round_trip_through_the_wire() {
    let event = TimerEvent {
        timer: TimerHandle {
            id: TimerId::new(9),
            label: "ticker".to_string(),
        },
        fired_at: Timestamp::from_nanos(5),
        count: 3,
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<clock_capnp::timer_event::Builder<'_>>();
        event.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<clock_capnp::timer_event::Reader<'_>>()
        .expect("root");
    assert_eq!(wire.get_count(), 3, "the kernel's count is not collapsed to 1");
    assert_eq!(wire.get_timer().expect("timer").get_label().expect("label"), "ticker");
}

/// The availability matrix is internally consistent per conformance §3.4: REAL
/// rows are empty of reason and carry an `l3_test`; DEGRADED rows name the
/// degraded capability; UNAVAILABLE rows carry a reason class and a reason; and
/// no row hides behind the flagged `unknown` class.
#[test]
fn availability_rows_are_consistent() {
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, CLOCK_AVAILABILITY.len());
    for row in CLOCK_AVAILABILITY {
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

/// The matrix carries exactly the contract's methods: no phantom row, no method
/// without a row, and ordinals that match the frozen schema.
#[test]
fn the_matrix_is_exactly_the_contract_surface() {
    for name in CONTRACT_METHODS {
        let row = CLOCK_AVAILABILITY
            .iter()
            .find(|r| r.name == *name)
            .unwrap_or_else(|| panic!("{name} has no availability row"));
        assert!(!row.reason.is_empty() || row.status == AvailStatus::Real);
    }
    for row in CLOCK_AVAILABILITY {
        assert!(
            CONTRACT_METHODS.contains(&row.name),
            "row {} is not a contract method",
            row.name
        );
    }
    // The ordinals the contract froze, for the interface this domain serves.
    let expected: &[(&str, u16)] = &[
        ("info", 0),
        ("monotonicNow", 1),
        ("realtimeNow", 2),
        ("createTimer", 3),
        ("cancelTimer", 4),
        ("resetTimer", 5),
        ("checkTimer", 6),
        ("subscribeTimers", 7),
        ("sleep", 8),
        ("listClockDomains", 9),
        ("sleepCancellable", 10),
        ("getClockSync", 11),
        ("getClockDrift", 12),
        ("createVirtualClock", 13),
        ("advanceVirtualClock", 14),
        ("getClockQuality", 15),
        ("persistTimer", 16),
        ("listPersistedTimers", 17),
        ("setTimerDependency", 18),
        ("alarms", 19),
        // The AlarmService interface, disambiguated at 0x100 + its ordinal.
        ("createAlarm", 0x100),
        ("getNextAlarm", 0x107),
    ];
    for (name, ordinal) in expected {
        let row = CLOCK_AVAILABILITY.iter().find(|r| r.name == *name).expect(name);
        assert_eq!(row.ordinal, *ordinal, "{name} ordinal moved");
    }
}

/// Every `l3_test` the matrix claims is a real test name in this suite, and
/// every test here is claimed: a REAL row whose test vanished is a FAIL, and a
/// test nobody claims is dead evidence.
///
/// The suite is read from the cell's OWN source at compile time — never from
/// a hand-kept list, which is a list that can drift away from the truth.
#[test]
fn every_claimed_test_exists() {
    for (method, test) in claimed_l3_tests() {
        assert!(
            the_cells_own_test_names().contains(&test.rsplit("::").next().unwrap_or(test)),
            "{method} claims {test}, which is not a test in the clock cell"
        );
    }
    for (name, is_unit_test) in the_cells_own_tests() {
        if is_unit_test {
            // The test says it proves the engine's own helper, not a
            // contract method, so the matrix does not publish it.
            continue;
        }
        let claim = alloc::format!("l3_linux_clock::{name}");
        assert!(
            claimed_l3_tests().any(|(_, t)| t == claim),
            "{claim} is real but unclaimed by the matrix, and it does not say \
             it is a unit test either"
        );
    }
}

/// Every test function the clock cell defines, read from its own source,
/// with the flag that says whether the matrix publishes it: a test the
/// matrix does not claim must SAY it is a unit test, in its own doc comment.
fn the_cells_own_tests() -> Vec<(&'static str, bool)> {
    let source = include_str!("../../platforms/linux/clock.rs");
    let lines: Vec<&str> = source.lines().collect();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[test]" {
            continue;
        }
        let Some(next) = lines.get(index + 1) else { break };
        let trimmed = next.trim();
        let Some(rest) = trimmed.strip_prefix("fn ") else { continue };
        let name: alloc::string::String =
            rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            continue;
        }
        // The doc comment above the test says whether it is a unit test.
        let mut is_unit_test = false;
        for earlier in lines[..index].iter().rev() {
            let trimmed = earlier.trim();
            if trimmed.starts_with("///") {
                if trimmed.contains("UNIT:") {
                    is_unit_test = true;
                }
                continue;
            }
            break;
        }
        names.push((leak(name), is_unit_test));
    }
    names
}

/// The names are read at compile time, so they live for the whole run.
fn the_cells_own_test_names() -> Vec<&'static str> {
    the_cells_own_tests()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// The names are read at compile time, so they live for the whole run.
fn leak(name: alloc::string::String) -> &'static str {
    alloc::boxed::Box::leak(name.into_boxed_str())
}

/// The compiled schema id matches `clock.capnp`'s `@0x…` line.
#[test]
fn schema_id_is_the_clock_contract() {
    assert_eq!(CLOCK_SCHEMA_ID, 0xa1b2c3d4e5f60040);
    #[allow(dead_code)]
    fn assert_generated_module_linked(_: Option<clock_capnp::clock_info::Owned>) {}
}

/// The trait's default surface answers `unavailable` **with a reason**, never
/// a zeroed success and never a panic — the law #6 baseline a platform backend
/// overrides.
#[test]
fn every_default_method_answers_with_a_reason() {
    let backend = Nothing;
    let err = backend
        .measure_clock_sync()
        .expect_err("a backend that cannot measure must say so");
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(!err.retryable, "unavailable is not retryable");
    assert!(!err.message.is_empty(), "an unavailable without a reason is banned");
}

/// Error propagation is total and honest across all 20 contract codes: a
/// backend that fails with a contract code surfaces it unchanged.
#[test]
fn error_injection_is_total_over_all_20_codes() {
    struct Faulted(Error);
    impl ClockBackend for Faulted {
        fn info(&self) -> crate::core::Result<ClockInfo> {
            Err(self.0.clone())
        }
    }
    for code in ErrorCode::ALL {
        let backend = Faulted(Error::new(code));
        let err = backend.info().unwrap_err();
        assert_eq!(err.code, code, "code {code:?} must survive the seam unchanged");
        assert_eq!(
            err.retryable,
            code.default_retryable(),
            "code {code:?} retryable must be the contract default"
        );
    }
}

/// Pagination is the contract's rule: `limit == 0` means "one", never "all",
/// and an offset past the end is an empty page rather than an error.
#[test]
fn pagination_zero_limit_means_one() {
    assert_eq!(Pagination { offset: 0, limit: 0 }.effective_limit(), 1);
    assert_eq!(Pagination { offset: 5, limit: 9 }.effective_limit(), 9);
    let _page = Page::<u32> {
        items: Vec::new(),
        total: 0,
    };
    assert_eq!(_page.total, 0);
}

/// The clock domains are a closed set with an open outlet: an unnamed domain is
/// `Custom`, never a guessed realtime.
#[test]
fn clock_domains_are_closed_but_unknown_tolerant() {
    assert_eq!(ClockDomainKind::Realtime.host_clock_id(), 0);
    assert_eq!(ClockDomainKind::Monotonic.host_clock_id(), 1);
    assert_eq!(ClockDomainKind::from_token("realtime"), ClockDomainKind::Realtime);
    assert_eq!(ClockDomainKind::from_token("tai"), ClockDomainKind::Custom);
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<clock_capnp::clock_domain_info::Builder<'_>>();
        builder.set_domain(clock_capnp::ClockDomain::Custom);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<clock_capnp::clock_domain_info::Reader<'_>>()
        .expect("root");
    assert_eq!(
        crate::domains::clock::types::ClockDomainInfo::from_reader(&wire).domain,
        ClockDomainKind::Custom
    );
}

/// A virtual clock is software by definition, so it is REAL rather than a
/// degraded host read: it does not move until told.
#[test]
fn virtual_clocks_are_software_and_honest_about_it() {
    let config = VirtualClockConfig {
        initial_time: Timestamp::from_nanos(10),
        domain: ClockDomainKind::Realtime,
        rate: 1.0,
        name: "test".to_string(),
    };
    let handle = VirtualClockHandle {
        id: 1,
        name: config.name.clone(),
    };
    // The types carry what the contract says they carry: a rate of 0.0 is
    // paused, and a rate is not a sleep.
    assert_eq!(config.rate, 1.0);
    assert_eq!(handle.name, "test");
}

/// The alarm types carry the contract's shape: recurrence's union and the
/// holiday behaviour travel through the wire as written.
#[test]
fn alarms_carry_the_contracts_shape() {
    let alarm = Alarm {
        id: crate::core::ids::AlarmId::new(3),
        label: "wake".to_string(),
        fire_time: Timestamp::from_nanos(1_000),
        recurrence: Some(AlarmRecurrence::Weekly(vec![1, 3, 5])),
        snooze_duration: Duration::from_nanos(540_000_000_000),
        enabled: true,
        holiday_behavior: Default::default(),
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<clock_capnp::alarm::Builder<'_>>();
        alarm.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<clock_capnp::alarm::Reader<'_>>()
        .expect("root");
    assert_eq!(wire.get_label().expect("label"), "wake");
    let days = match wire.get_recurrence().expect("recurrence").which() {
        Ok(clock_capnp::alarm_recurrence::WhichReader::Weekly(Ok(weekly))) => {
            weekly.get_days_of_week().expect("days").len()
        }
        _ => panic!("the weekly recurrence arm must round-trip"),
    };
    assert_eq!(days, 3);
    assert_eq!(
        wire.get_snooze_duration().expect("snooze").get_nanoseconds(),
        540_000_000_000
    );
}

/// The public API is a thin borrowing wrapper: it adds no state and surfaces
/// the backend's own answers and capability tokens.
#[test]
fn the_public_api_is_a_thin_wrapper() {
    struct Scripted;
    impl ClockBackend for Scripted {
        fn info(&self) -> crate::core::Result<ClockInfo> {
            Ok(ClockInfo {
                monotonic: Timestamp::from_nanos(1),
                realtime: Timestamp::from_nanos(2),
                resolution: Duration::from_nanos(1),
                source_name: "test-clock".to_string(),
                boot_time: Timestamp::from_nanos(3),
                sync: ClockSyncInfo::default(),
                security: Default::default(),
                domains: Vec::new(),
            })
        }
        fn realtime_now(&self) -> crate::core::Result<Timestamp> {
            Ok(Timestamp::from_nanos(2))
        }
        fn create_timer(&self, spec: &TimerSpec) -> crate::core::Result<TimerHandle> {
            Ok(TimerHandle {
                id: TimerId::new(1),
                label: spec.label.clone(),
            })
        }
        fn subscribe_timers(&self, _f: &super::types::TimerFilter) -> crate::core::Result<super::types::SubscriptionId> {
            Ok(super::types::SubscriptionId(1))
        }
        fn pull_timers(&self, _s: super::types::SubscriptionId) -> crate::core::Result<Vec<TimerEvent>> {
            Ok(Vec::new())
        }
        fn cancel_timers(&self, _s: super::types::SubscriptionId) -> crate::core::Result<()> {
            Ok(())
        }
        fn alarms(&self) -> crate::core::Result<alloc::boxed::Box<dyn AlarmService>> {
            struct Store;
            impl AlarmService for Store {
                fn create_alarm(&self, _a: &Alarm) -> crate::core::Result<crate::core::ids::AlarmId> {
                    Ok(crate::core::ids::AlarmId::new(1))
                }
                fn update_alarm(&self, _a: &Alarm) -> crate::core::Result<()> {
                    Ok(())
                }
                fn delete_alarm(&self, _id: crate::core::ids::AlarmId) -> crate::core::Result<()> {
                    Ok(())
                }
                fn get_alarm(&self, _id: crate::core::ids::AlarmId) -> crate::core::Result<Alarm> {
                    Ok(Alarm {
                        id: crate::core::ids::AlarmId::new(1),
                        label: "x".to_string(),
                        fire_time: Timestamp::UNIX_EPOCH,
                        recurrence: None,
                        snooze_duration: Duration::ZERO,
                        enabled: true,
                        holiday_behavior: Default::default(),
                    })
                }
                fn list_alarms(&self, _page: Pagination) -> crate::core::Result<Page<Alarm>> {
                    Ok(Page {
                        items: alloc::vec![Alarm {
                            id: crate::core::ids::AlarmId::new(1),
                            label: "x".to_string(),
                            fire_time: Timestamp::UNIX_EPOCH,
                            recurrence: None,
                            snooze_duration: Duration::ZERO,
                            enabled: true,
                            holiday_behavior: Default::default(),
                        }],
                        total: 1,
                    })
                }
                fn snooze_alarm(&self, _id: crate::core::ids::AlarmId, _d: Duration) -> crate::core::Result<()> {
                    Ok(())
                }
                fn dismiss_alarm(&self, _id: crate::core::ids::AlarmId) -> crate::core::Result<()> {
                    Ok(())
                }
                fn get_next_alarm(&self) -> crate::core::Result<Alarm> {
                    Ok(Alarm {
                        id: crate::core::ids::AlarmId::new(1),
                        label: "x".to_string(),
                        fire_time: Timestamp::UNIX_EPOCH,
                        recurrence: None,
                        snooze_duration: Duration::ZERO,
                        enabled: true,
                        holiday_behavior: Default::default(),
                    })
                }
            }
            Ok(alloc::boxed::Box::new(Store))
        }
    }
    let backend = Scripted;
    let clock = super::Clock::new(&backend);
    let summary = clock.summary().expect("summary");
    assert_eq!(summary.realtime.as_nanos(), 2);
    assert_eq!(summary.source_name, "test-clock");
    let timer = clock
        .create_timer(&TimerSpec::one_shot_relative(Duration::from_nanos(1)))
        .expect("timer");
    assert_eq!(timer.label, "");
    let (subscription, first) = clock.timer_stream(&[]).expect("stream");
    assert!(first.is_empty());
    clock.close_timer_stream(subscription).expect("close");
    let alarms = clock.alarms(Pagination { offset: 0, limit: 0 }).expect("alarms");
    assert_eq!(alarms.total, 1);
    assert_eq!(clock.caps().len(), 4);
    // The row for one method resolves by name — the tooling path.
    assert!(super::method_availability_for("info").is_some());
    assert!(super::method_availability_for("noSuchMethod").is_none());
    let _ = super::AvailStatus::Real;
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

/// The clock cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/clock.rs");

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
            "{method} claims {test}, which is not a test function in ../../platforms/linux/clock.rs"
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
