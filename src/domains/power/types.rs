use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::values::Duration;
use crate::schema::{power_capnp, types_capnp};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pub offset: u32,
    pub limit: u32,
}

impl Pagination {
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 { 1 } else { self.limit }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatteryState {
    pub present: bool,
    pub percentage: u32,
    pub charging: bool,
    pub plugged: bool,
    pub time_to_empty: Duration,
    pub time_to_full: Duration,
}

impl BatteryState {
    pub fn write_to(&self, b: &mut power_capnp::battery_state::Builder<'_>) {
        b.set_present(self.present);
        b.set_percentage(self.percentage);
        b.set_charging(self.charging);
        b.set_plugged(self.plugged);
        b.reborrow().init_time_to_empty().set_nanoseconds(self.time_to_empty.as_nanos());
        b.reborrow().init_time_to_full().set_nanoseconds(self.time_to_full.as_nanos());
    }
    pub fn from_reader(r: &power_capnp::battery_state::Reader<'_>) -> Self {
        Self {
            present: r.get_present(),
            percentage: r.get_percentage(),
            charging: r.get_charging(),
            plugged: r.get_plugged(),
            time_to_empty: Duration::from_nanos(r.get_time_to_empty().map(|d| d.get_nanoseconds()).unwrap_or(0)),
            time_to_full: Duration::from_nanos(r.get_time_to_full().map(|d| d.get_nanoseconds()).unwrap_or(0)),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerProfile {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub active: bool,
}

impl PowerProfile {
    pub fn write_to(&self, b: &mut power_capnp::power_profile::Builder<'_>) {
        b.reborrow().init_id().set_id(self.id);
        b.set_name(&self.name);
        b.set_description(&self.description);
        b.set_active(self.active);
    }
    pub fn from_reader(r: &power_capnp::power_profile::Reader<'_>) -> Self {
        Self {
            id: r.get_id().map(|x| x.get_id()).unwrap_or(0),
            name: r.get_name().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            description: r.get_description().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            active: r.get_active(),
        }
    }
}
