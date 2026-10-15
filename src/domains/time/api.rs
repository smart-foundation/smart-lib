//! The public time API: a thin, borrowing wrapper over [`TimeBackend`] exposing
//! the calls a client actually touches. Borrowed and zero-cost — it adds no
//! state over `&B`.

use crate::core::values::Timestamp;
use crate::core::Result;

use super::backend::TimeBackend;
use super::types::{CalendarSystem, CalendarUnit, DateTime, HistoricalDate};

/// A borrowing handle to any [`TimeBackend`].
pub struct Time<'a, B: TimeBackend + ?Sized>(&'a B);

impl<'a, B: TimeBackend + ?Sized> Time<'a, B> {
    /// Wrap a backend (which the caller has already acquired authority to hold
    /// — law #5: the handle is the authority).
    pub fn new(backend: &'a B) -> Self {
        Time(backend)
    }

    /// The current wall-clock reading, with the zone the backend applied.
    #[must_use = "the reading is the answer; ignoring it is never useful"]
    pub fn now(&self) -> Result<DateTime> {
        self.0.get_date_time()
    }

    /// Convert a date from one calendar to another.
    pub fn convert(
        &self,
        date: &HistoricalDate,
        from: CalendarSystem,
        to: CalendarSystem,
    ) -> Result<HistoricalDate> {
        self.0.convert_calendar(date, from, to)
    }

    /// Add calendar units.
    pub fn add(&self, date: &HistoricalDate, units: &[CalendarUnit]) -> Result<HistoricalDate> {
        self.0.add_calendar_units(date, units)
    }

    /// Subtract calendar units.
    pub fn subtract(&self, date: &HistoricalDate, units: &[CalendarUnit]) -> Result<HistoricalDate> {
        self.0.subtract_calendar_units(date, units)
    }

    /// The contract's TAI converter, in the contract's epoch.
    #[must_use]
    pub fn to_tai(&self, utc: Timestamp) -> Result<crate::domains::time::types::TaiTimestamp> {
        self.0.convert_to_tai(utc)
    }

    /// The backend's published capability tokens (law #6).
    #[must_use]
    pub fn caps(&self) -> alloc::vec::Vec<&'static str> {
        self.0.time_caps()
    }
}
