use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::power_capnp;

use super::availability::{status_counts, AvailStatus, POWER_AVAILABILITY, POWER_SCHEMA_ID};
use super::backend::PowerBackend;
use super::types::{BatteryState, PowerProfile};

const CONTRACT_METHODS: &[&str] = &[
    "getBattery",
    "getProfiles",
    "setProfile",
    "subscribeBattery",
    "suspend",
    "hibernate",
    "reboot",
    "shutdown",
    "subscribePowerTelemetry",
    "getPowerBudget",
    "setPowerBudget",
    "getChargingInfo",
    "negotiateUsbPd",
    "getUsbPdContract",
    "negotiateWirelessCharging",
    "getGridEnergy",
    "getCarbonIntensity",
    "getPoEInfo",
    "configureCellBalancing",
    "getCellBalancingStatus",
    "getFuelGauge",
    "predictThermalRunaway",
];

struct HonestBackend;
impl PowerBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(POWER_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in POWER_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn battery_state_round_trips() {
    let b = BatteryState { present: true, percentage: 62, charging: false, plugged: true, time_to_empty: crate::core::values::Duration::from_nanos(100), time_to_full: crate::core::values::Duration::from_nanos(0) };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<power_capnp::battery_state::Builder<'_>>();
        b.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new()).expect("deserialize");
    let wire = reader.get_root::<power_capnp::battery_state::Reader<'_>>().expect("root");
    assert_eq!(BatteryState::from_reader(&wire), b);
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

/// The power cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/power.rs");

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
            "{method} claims {test}, which is not a test function in ../../platforms/linux/power.rs"
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
