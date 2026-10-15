//! The **time** domain: what time is it, in which calendar, and is this
//! timestamp attested — over one object-safe [`TimeBackend`] trait with the
//! frozen `time.capnp` surface (`getDateTime @0` through `diffCalendarDates
//! @5`).
//!
//! What this domain is: the contract's canonical time representation and its
//! calendar arithmetic, read natively from the kernel's own clock. The epoch law
//! this file holds: `time.capnp` declares **no** TAI type. `TaiTimestamp` lives
//! in `timezone.capnp`, on the **Unix epoch (1970)**, and its
//! `convertToTai`/`convertFromTai` are the canonical converters. This domain
//! mirrors that converter with the same epoch and the same rule, so a consumer
//! holding only a `Time` reference never reaches for a second, 1958-epoched
//! spelling of "TAI" — the ~120-year silent corruption the revamp deleted.
//!
//! ## The mechanisms on this backend (law #9)
//!
//! | contract surface | native mechanism |
//! |---|---|
//! | `getDateTime` | `CLOCK_REALTIME` plus the host's own zone, read from `/etc/localtime`'s symlink target (the kernel's own mapping, not a `date` shellout) |
//! | `convertCalendar` | exact civil-date arithmetic (`days_from_civil`/`civil_from_days`), the same kernels `timezone.capnp`'s converters sit on |
//! | `addCalendarUnits` / `subtractCalendarUnits` / `diffCalendarDates` | the same kernels, with month-end clamping |
//! | TAI helpers | the contract's rule (TAI = UTC + 37 s) and the contract's epoch |
//!
//! What is honestly refused, and why: an HSM attestation cannot be verified on a
//! host with no HSM (measured: no TPM device, no PKCS#11 module), and two
//! calendars are refused with a reason — the Hebrew calendar's molad and
//! postponement rules, and the Chinese calendar's astronomical new moons — because
//! an approximated conversion between the wrong two calendars is silently wrong.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Time;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    ReasonClass, TIME_AVAILABILITY, TIME_SCHEMA_ID,
};
pub use backend::TimeBackend;
pub use types::{
    civil_from_days, days_from_civil, days_from_ethiopian,
    days_from_islamic, day_of_year, days_in_month, ethiopian_from_days, islamic_from_days,
    islamic_is_leap_year, islamic_year_days, iso_weekday, is_gregorian_leap, is_julian_leap,
    tai_to_utc_nanos, utc_nanos_to_tai, CalendarDiff, CalendarSystem, CalendarUnit,
    CalendarUnitKind, DateTime, HistoricalDate, HsmTimestamp, TaiTimestamp,
    ETHIOPIAN_EPOCH_DAYS, ISLAMIC_EPOCH_DAYS, LEAP_SECONDS_TAI_MINUS_UTC,
};
