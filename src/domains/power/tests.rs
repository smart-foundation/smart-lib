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
