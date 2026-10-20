//! The **power** domain: battery, charging, brightness — over one object-safe
//! [`PowerBackend`] trait. Native mechanism: /sys/class/power_supply and
//! /sys/class/backlight (UPower D-Bus for system integration).

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Power;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, POWER_AVAILABILITY,
    POWER_SCHEMA_ID, MethodAvailability, ReasonClass,
};
pub use backend::PowerBackend;
pub use types::{BatteryState, Page, Pagination, PowerProfile};
