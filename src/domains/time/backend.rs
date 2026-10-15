//! The `TimeBackend` trait: one object-safe method per `Time` contract method
//! (`getDateTime @0` through `diffCalendarDates @5`), each returning
//! [`crate::core::Result`].
//!
//! ## Capability story (law #5)
//!
//! There is **no permission string on any method**. Authority is the *held
//! service reference*. Reading the wall clock and converting a calendar are
//! reads of pure arithmetic or of the kernel's own clock; the only
//! write-adjacent verb in the wider time surface, `setDefaultTimezone`, belongs
//! to `timezone.capnp` and not here.
//!
//! ## Object safety and the honest default surface
//!
//! Every method borrows `&self` and returns owned values. Each has a default
//! body that answers `notSupported` **with a reason** — the law #6 baseline a
//! platform backend overrides.

use alloc::vec::Vec;

use crate::core::values::Timestamp;
use crate::core::{Error, Result};

use super::types::{
    CalendarDiff, CalendarSystem, CalendarUnit, DateTime, HsmTimestamp, HistoricalDate,
    TaiTimestamp,
};

/// A protocol-compliant time backend: one method per contract method, with an
/// honest `notSupported`-with-reason default for everything a host cannot
/// serve.
pub trait TimeBackend {
    /// The current wall-clock reading in the host's zone (`getDateTime @0`).
    ///
    /// `timezone` and `utcOffsetSeconds` are the backend's report of what it
    /// applied: the zone the host publishes and the offset the kernel reports,
    /// never an inferred "local".
    fn get_date_time(&self) -> Result<DateTime> {
        Err(Error::not_supported(
            "no-backend: the wall-clock reading is not served on this backend",
        ))
    }

    /// Verify an HSM-attested timestamp (`verifyHsmTimestamp @1`), answering in
    /// the contract's `T.BoolResult` shape so the outcome is machine-routable.
    ///
    /// `None` means "this host cannot verify" and the reason travels in the
    /// error; `Some(true)`/`Some(false)` is a real verification verdict.
    #[allow(clippy::result_unit_err)]
    fn verify_hsm_timestamp(&self, _hsm: &HsmTimestamp) -> Result<Option<bool>> {
        Err(Error::not_supported(
            "no-backend: HSM timestamp verification is not served on this backend",
        ))
    }

    /// Convert a date between two calendars (`convertCalendar @2`). A pair this
    /// build does not carry answers honestly rather than echoing the input.
    fn convert_calendar(
        &self,
        _date: &HistoricalDate,
        _from: CalendarSystem,
        _to: CalendarSystem,
    ) -> Result<HistoricalDate> {
        Err(Error::not_supported(
            "no-backend: calendar conversion is not served on this backend",
        ))
    }

    /// Add calendar units to a date (`addCalendarUnits @3`).
    fn add_calendar_units(&self, _date: &HistoricalDate, _units: &[CalendarUnit]) -> Result<HistoricalDate> {
        Err(Error::not_supported(
            "no-backend: calendar arithmetic is not served on this backend",
        ))
    }

    /// Subtract calendar units from a date (`subtractCalendarUnits @4`).
    fn subtract_calendar_units(
        &self,
        _date: &HistoricalDate,
        _units: &[CalendarUnit],
    ) -> Result<HistoricalDate> {
        Err(Error::not_supported(
            "no-backend: calendar arithmetic is not served on this backend",
        ))
    }

    /// The difference between two dates (`diffCalendarDates @5`).
    fn diff_calendar_dates(
        &self,
        _date1: &HistoricalDate,
        _date2: &HistoricalDate,
    ) -> Result<CalendarDiff> {
        Err(Error::not_supported(
            "no-backend: calendar arithmetic is not served on this backend",
        ))
    }

    // ── The TAI surface, in the contract's own epoch ────────────────────

    /// Convert a UTC timestamp to the contract's `TaiTimestamp`
    /// (`timezone.capnp`'s `convertToTai`, whose epoch this domain mirrors so a
    /// caller holding only a `Time` reference never reaches for a second,
    /// 1958-epoched spelling of "TAI").
    fn convert_to_tai(&self, _utc: Timestamp) -> Result<TaiTimestamp> {
        Err(Error::not_supported(
            "no-backend: the TAI converter is not served on this backend",
        ))
    }

    /// Convert the contract's `TaiTimestamp` back to UTC (the contract's
    /// `convertFromTai`).
    fn convert_from_tai(&self, _tai: TaiTimestamp) -> Result<Timestamp> {
        Err(Error::not_supported(
            "no-backend: the TAI converter is not served on this backend",
        ))
    }

    /// Publish this backend's honest time capability tokens (law #6).
    fn time_caps(&self) -> Vec<&'static str> {
        alloc::vec!["time.datetime", "time.calendar", "time.tai"]
    }
}
