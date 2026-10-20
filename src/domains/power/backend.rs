use alloc::string::String;
use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{BatteryState, PowerProfile, SubscriptionId};

pub trait PowerBackend {
    fn get_battery(&self) -> Result<BatteryState> {
        Err(Error::not_supported("no-backend: battery is not served on this backend"))
    }
    fn get_profiles(&self) -> Result<Vec<PowerProfile>> {
        Err(Error::not_supported("no-backend: power profiles are not served on this backend"))
    }
    fn set_profile(&self, _profile: u64) -> Result<()> {
        Err(Error::not_supported("no-backend: set power profile is not served on this backend"))
    }
    fn subscribe_battery(&self) -> Result<SubscriptionId> {
        Err(Error::not_supported("no-backend: battery subscription is not served on this backend"))
    }
    fn pull_battery(&self, _subscription: SubscriptionId) -> Result<BatteryState> {
        Err(Error::not_supported("no-backend: battery pull is not served on this backend"))
    }
    fn cancel_battery(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(Error::not_supported("no-backend: battery cancel is not served on this backend"))
    }
    fn suspend(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: suspend is not served on this backend"))
    }
    fn hibernate(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: hibernate is not served on this backend"))
    }
    fn reboot(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: reboot is not served on this backend"))
    }
    fn shutdown(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: shutdown is not served on this backend"))
    }
    fn get_charging_info(&self) -> Result<String> {
        Err(Error::not_supported("no-backend: charging info is not served on this backend"))
    }
    fn get_fuel_gauge(&self) -> Result<String> {
        Err(Error::not_supported("no-backend: fuel gauge is not served on this backend"))
    }
}
