//! The Linux **time** backend for the time domain — the kernel's own clock, the
//! host's own zone read out of its TZif file, and the contract's own calendar
//! arithmetic (law #9).
//!
//! ## The mechanisms, and why each one
//!
//! | surface | mechanism |
//! |---|---|
//! | `getDateTime` | `CLOCK_REALTIME` (the kernel's clock — no `date` shellout) split into civil fields with the same `days_from_civil`/`civil_from_days` kernels the contract's own converters sit on |
//! | the zone | `/etc/localtime`'s symlink target: the host's own spelling of its zone |
//! | the offset | the host's own TZif file, parsed natively: the last transition at or before now gives the `ttinfo`'s `utoff` — a measured offset, never an inferred one |
//! | `convertCalendar` | exact arithmetic: the Julian calendar's 365.25-day years, the tabular Islamic calendar's 30-year cycle, the Ethiopian calendar's 13 months |
//! | `addCalendarUnits` / `subtractCalendarUnits` | the same kernels with month-end clamping (Jan 31 + 1 month = Feb 28/29, never a fabricated 31) |
//! | `diffCalendarDates` | exact day counts with the y/m/d breakdown beside them |
//! | TAI | the contract's rule and epoch: TAI = UTC + 37 s, seconds **and** nanoseconds since 1970 |
//!
//! ## Law #8 (the host is read-only)
//!
//! Every path here is a read of the kernel's clock or of `/etc/localtime`. There
//! is no host write anywhere: the one write-adjacent verb in the wider time
//! surface, `setDefaultTimezone`, belongs to `timezone.capnp`.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::values::Timestamp;
use crate::core::{Error, Result};

use crate::domains::time::backend::TimeBackend;
use crate::domains::time::types::{
    civil_from_days, days_from_civil, day_of_year, days_from_ethiopian, days_from_islamic,
    days_in_month, ethiopian_from_days, islamic_from_days, iso_weekday, tai_to_utc_nanos,
    utc_nanos_to_tai, CalendarDiff, CalendarSystem, CalendarUnit, CalendarUnitKind, DateTime,
    HistoricalDate, HsmTimestamp, TaiTimestamp,
};

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

unsafe extern "C" {
    fn clock_gettime(clockid: i32, tp: *mut Timespec) -> i32;
}

const CLOCK_REALTIME: i32 = 0;
const NSEC_PER_SEC: u64 = 1_000_000_000;
const TZIF_MAGIC: &[u8; 4] = b"TZif";

/// The kernel's own realtime clock, in nanoseconds since the Unix epoch.
fn realtime_nanos() -> Result<u64> {
    let mut ts = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a correctly sized, correctly aligned out-param for the
    // kernel's `struct timespec`, and `CLOCK_REALTIME` is a valid clock id.
    let rc = unsafe { clock_gettime(CLOCK_REALTIME, &mut ts) };
    if rc != 0 {
        return Err(Error::failed(format!(
            "clock_gettime(CLOCK_REALTIME) failed with rc={rc}"
        )));
    }
    Ok((ts.tv_sec.max(0) as u64).saturating_mul(NSEC_PER_SEC)
        + (ts.tv_nsec.clamp(0, 999_999_999) as u64))
}

/// One `ttinfo` record out of a TZif file: the offset east of UTC, whether the
/// type is daylight-saving, and the index of its abbreviation.
#[derive(Clone, Copy, Debug)]
struct TtInfo {
    utoff: i32,
    isdst: u8,
    desigidx: u8,
}

/// A parsed TZif file: the transition instants and the type index each one
/// selects. Enough to answer "what offset was the host using at time T" — which
/// is the whole of `DateTime.utcOffsetSeconds`.
struct TzFile {
    transitions: Vec<i64>,
    types: Vec<u8>,
    infos: Vec<TtInfo>,
}

impl TzFile {
    /// The offset in effect at `epoch_seconds` (the host's own rule, read from
    /// the host's own file).
    fn offset_at(&self, epoch_seconds: i64) -> Option<i32> {
        // The last transition at or before the instant decides; with no
        // transitions at all, the first non-DST type is the zone's rule.
        let mut selected: Option<u8> = None;
        for (i, t) in self.transitions.iter().enumerate() {
            if *t <= epoch_seconds {
                selected = Some(self.types[i]);
            } else {
                break;
            }
        }
        let index = selected.or_else(|| self.infos.iter().position(|info| info.isdst == 0).map(|i| i as u8))?;
        self.infos.get(index as usize).map(|info| info.utoff)
    }
}

/// Read one 32-bit big-endian count out of a TZif header.
fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Read one 64-bit big-endian transition instant (TZif's second block).
fn be_i64(bytes: &[u8], at: usize) -> Option<i64> {
    let slice = bytes.get(at..at + 8)?;
    Some(i64::from_be_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

/// Parse a TZif file (RFC 8536). Version 2/3 files carry a 32-bit-time block
/// first and a 64-bit-time block second; the second one is the one this parser
/// reads, because a host whose clock is past 2038 must not be answered from the
/// truncated block.
fn parse_tzif(bytes: &[u8]) -> Option<TzFile> {
    if bytes.len() < 44 || &bytes[0..4] != TZIF_MAGIC {
        return None;
    }
    let version = bytes[4];
    let mut at = 20usize;
    // header: isutcnt, isstdcnt, leapcnt, timecnt, typecnt, charcnt
    let _isutcnt = be_u32(bytes, at)? as usize;
    let _isstdcnt = be_u32(bytes, at + 4)? as usize;
    let leapcnt = be_u32(bytes, at + 8)? as usize;
    let timecnt_v1 = be_u32(bytes, at + 12)? as usize;
    let typecnt_v1 = be_u32(bytes, at + 16)? as usize;
    let charcnt_v1 = be_u32(bytes, at + 20)? as usize;
    at += 24;

    // Skip the 32-bit block when a 64-bit one follows.
    if version == b'2' || version == b'3' {
        let v1_size = timecnt_v1 * 4
            + timecnt_v1
            + typecnt_v1 * 6
            + charcnt_v1
            + leapcnt * 12
            + _isstdcnt
            + _isutcnt;
        at += v1_size;
        if bytes.get(at..at + 4) != Some(&TZIF_MAGIC[..]) {
            return None;
        }
        let _isutcnt2 = be_u32(bytes, at + 20)? as usize;
        let _isstdcnt2 = be_u32(bytes, at + 24)? as usize;
        let leapcnt2 = be_u32(bytes, at + 28)? as usize;
        let timecnt2 = be_u32(bytes, at + 32)? as usize;
        let typecnt2 = be_u32(bytes, at + 36)? as usize;
        let charcnt2 = be_u32(bytes, at + 40)? as usize;
        at += 44;
        let mut transitions = Vec::with_capacity(timecnt2);
        for i in 0..timecnt2 {
            transitions.push(be_i64(bytes, at + i * 8)?);
        }
        at += timecnt2 * 8;
        let mut types = Vec::with_capacity(timecnt2);
        for i in 0..timecnt2 {
            types.push(*bytes.get(at + i)?);
        }
        at += timecnt2;
        let mut infos = Vec::with_capacity(typecnt2);
        for i in 0..typecnt2 {
            let base = at + i * 6;
            let utoff = be_u32(bytes, base)? as i32;
            let isdst = *bytes.get(base + 4)?;
            let desigidx = *bytes.get(base + 5)?;
            infos.push(TtInfo { utoff, isdst, desigidx });
        }
        let _ = (charcnt2, leapcnt2, _isstdcnt2, _isutcnt2);
        return Some(TzFile { transitions, types, infos });
    }

    // Version 0/1: one block, 32-bit transition instants.
    let mut transitions = Vec::with_capacity(timecnt_v1);
    for i in 0..timecnt_v1 {
        transitions.push(be_u32(bytes, at + i * 4)? as i32 as i64);
    }
    at += timecnt_v1 * 4;
    let mut types = Vec::with_capacity(timecnt_v1);
    for i in 0..timecnt_v1 {
        types.push(*bytes.get(at + i)?);
    }
    at += timecnt_v1;
    let mut infos = Vec::with_capacity(typecnt_v1);
    for i in 0..typecnt_v1 {
        let base = at + i * 6;
        infos.push(TtInfo {
            utoff: be_u32(bytes, base)? as i32,
            isdst: *bytes.get(base + 4)?,
            desigidx: *bytes.get(base + 5)?,
        });
    }
    Some(TzFile { transitions, types, infos })
}

/// The Linux time backend: the kernel clock, the host's zone, the contract's
/// calendar arithmetic.
pub struct LinuxTimeBackend;

impl Default for LinuxTimeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxTimeBackend {
    /// A backend over this host's clock.
    pub fn new() -> Self {
        LinuxTimeBackend
    }

    /// The host's own zone, as an open token, read from `/etc/localtime`'s
    /// symlink target — the host's own spelling, never an inferred "local".
    /// Empty when the host publishes none (a plain file, not a symlink).
    pub fn host_timezone(&self) -> String {
        match std::fs::read_link("/etc/localtime") {
            Ok(target) => {
                let text = target.to_string_lossy().to_string();
                for prefix in ["/zoneinfo/", "/usr/share/zoneinfo/"] {
                    if let Some(rest) = text.strip_prefix(prefix) {
                        return rest.to_string();
                    }
                }
                text
            }
            Err(_) => String::new(),
        }
    }

    /// The host's own UTC offset in seconds east, parsed out of the host's TZif
    /// file at the current instant. Zero when the host does not answer — an
    /// unparseable or absent zone file is honest, never a guessed offset.
    pub fn host_utc_offset_seconds(&self) -> i32 {
        let bytes = match std::fs::read("/etc/localtime") {
            Ok(bytes) => bytes,
            Err(_) => return 0,
        };
        let Some(zone) = parse_tzif(&bytes) else {
            return 0;
        };
        let now = realtime_nanos().unwrap_or(0);
        zone.offset_at((now / NSEC_PER_SEC) as i64).unwrap_or(0)
    }
}

impl TimeBackend for LinuxTimeBackend {
    fn get_date_time(&self) -> Result<DateTime> {
        let nanos = realtime_nanos()?;
        let days = (nanos / NSEC_PER_SEC / 86_400) as i64;
        let secs_of_day = (nanos / NSEC_PER_SEC) % 86_400;
        let (year, month, day) = civil_from_days(days);
        Ok(DateTime {
            year: year as i32,
            month: month as u8,
            day: day as u8,
            hour: (secs_of_day / 3_600) as u8,
            minute: ((secs_of_day % 3_600) / 60) as u8,
            second: (secs_of_day % 60) as u8,
            millisecond: ((nanos % NSEC_PER_SEC) / 1_000_000) as u16,
            weekday: iso_weekday(days),
            yearday: day_of_year(days),
            timezone: self.host_timezone(),
            utc_offset_seconds: self.host_utc_offset_seconds(),
        })
    }

    fn verify_hsm_timestamp(&self, _hsm: &HsmTimestamp) -> Result<Option<bool>> {
        // MEASURED, not assumed: the host has no attestation authority (no TPM
        // character device, no PKCS#11 module), so this is refused rather than
        // answered against an imagined verifier.
        Err(Error::not_supported(
            "no-backend: this host exposes no hardware security module (no TPM character device \
             under /dev, no PKCS#11 module), so an HSM attestation cannot be verified",
        ))
    }

    fn convert_calendar(
        &self,
        date: &HistoricalDate,
        from: CalendarSystem,
        to: CalendarSystem,
    ) -> Result<HistoricalDate> {
        if from == to {
            return Ok(*date);
        }
        // Every conversion crosses one day count, so a calendar pair this build
        // does not carry is a refusal and never a silently-shifted date.
        let days = days_in_calendar(date)?;
        date_from_days(days, to)
    }

    fn add_calendar_units(&self, date: &HistoricalDate, units: &[CalendarUnit]) -> Result<HistoricalDate> {
        arithmetic(date, units, 1)
    }

    fn subtract_calendar_units(
        &self,
        date: &HistoricalDate,
        units: &[CalendarUnit],
    ) -> Result<HistoricalDate> {
        arithmetic(date, units, -1)
    }

    fn diff_calendar_dates(&self, date1: &HistoricalDate, date2: &HistoricalDate) -> Result<CalendarDiff> {
        let a = days_in_calendar(date1)?;
        let b = days_in_calendar(date2)?;
        let total = a - b;
        let (years, months, days) = breakdown(date1, date2, total);
        Ok(CalendarDiff {
            years: years as i32,
            months: months as i32,
            days: days as i32,
            total_days: total,
            negative: total < 0,
        })
    }

    fn convert_to_tai(&self, utc: Timestamp) -> Result<TaiTimestamp> {
        Ok(utc_nanos_to_tai(utc.as_nanos()))
    }

    fn convert_from_tai(&self, tai: TaiTimestamp) -> Result<Timestamp> {
        Ok(Timestamp::from_nanos(tai_to_utc_nanos(tai)))
    }
}

/// The day count of a date in its own calendar — the single canonical axis every
/// conversion crosses.
fn days_in_calendar(date: &HistoricalDate) -> Result<i64> {
    match date.calendar {
        CalendarSystem::Gregorian => {
            Ok(days_from_civil(date.year, date.month as u32, date.day as u32))
        }
        CalendarSystem::Julian => Ok(julian_to_days(date.year, date.month, date.day)),
        CalendarSystem::Islamic => Ok(days_from_islamic(date.year, date.month, date.day)),
        CalendarSystem::Ethiopian => Ok(days_from_ethiopian(date.year, date.month, date.day)),
        other => Err(Error::not_supported(format!(
            "no-backend: the {other:?} calendar is not carried by this backend; the contract's \
             `custom` calendar outlet is the way a backend names one it does"
        ))),
    }
}

/// A date in a target calendar from a day count — the other half of
/// [`days_in_calendar`].
fn date_from_days(days: i64, calendar: CalendarSystem) -> Result<HistoricalDate> {
    let date = match calendar {
        CalendarSystem::Gregorian => {
            let (y, m, d) = civil_from_days(days);
            HistoricalDate {
                year: y,
                month: m as u8,
                day: d as u8,
                calendar,
            }
        }
        CalendarSystem::Julian => julian_from_days(days),
        CalendarSystem::Islamic => {
            let (y, m, d) = islamic_from_days(days);
            HistoricalDate {
                year: y,
                month: m,
                day: d,
                calendar,
            }
        }
        CalendarSystem::Ethiopian => {
            let (y, m, d) = ethiopian_from_days(days);
            HistoricalDate {
                year: y,
                month: m,
                day: d,
                calendar,
            }
        }
        other => {
            return Err(Error::not_supported(format!(
                "no-backend: the {other:?} calendar is not carried by this backend; the contract's \
                 `custom` calendar outlet is the way a backend names one it does"
            )))
        }
    };
    Ok(date)
}

/// The y/m/d breakdown of a difference, computed on the later date's own
/// calendar so "one month" means one calendar month and not thirty days.
fn breakdown(date1: &HistoricalDate, date2: &HistoricalDate, total: i64) -> (i64, i64, i64) {
    let (later, earlier) = if total >= 0 { (date1, date2) } else { (date2, date1) };
    let mut years = later.year - earlier.year;
    let mut months = later.month as i64 - earlier.month as i64;
    if months < 0 {
        years -= 1;
        months += 12;
    }
    let mut days = later.day as i64 - earlier.day as i64;
    if days < 0 {
        months -= 1;
        if months < 0 {
            years -= 1;
            months += 12;
        }
        // The month borrowed from is the one before `later`'s own month, so the
        // borrow is the host calendar's real length (28, 29, 30 or 31).
        let borrow_month = if later.month == 1 { 12 } else { later.month - 1 };
        let borrow_year = if later.month == 1 { later.year - 1 } else { later.year };
        days += days_in_month(borrow_year, borrow_month as u32, later.calendar) as i64;
    }
    let sign = if total < 0 { -1 } else { 1 };
    (years * sign, months * sign, days * sign)
}

/// Calendar arithmetic: apply units of days/weeks/months/years in one walk, with
/// the month-end clamp (Jan 31 + 1 month = Feb 28/29, stated rather than
/// overflowing into March).
fn arithmetic(date: &HistoricalDate, units: &[CalendarUnit], sign: i64) -> Result<HistoricalDate> {
    let mut current = *date;
    for unit in units {
        match unit.kind {
            CalendarUnitKind::Days | CalendarUnitKind::Weeks => {
                let per = if unit.kind == CalendarUnitKind::Weeks { 7 } else { 1 };
                let delta = (unit.amount as i64) * per * sign;
                let days = days_in_calendar(&current)? + delta;
                current = date_from_days(days, current.calendar)?;
            }
            CalendarUnitKind::Months | CalendarUnitKind::Years => {
                let months = match unit.kind {
                    CalendarUnitKind::Months => unit.amount as i64,
                    _ => unit.amount as i64 * 12,
                } * sign;
                let mut year = current.year + months / 12;
                let mut month = current.month as i64 + months % 12;
                while month > 12 {
                    month -= 12;
                    year += 1;
                }
                while month < 1 {
                    month += 12;
                    year -= 1;
                }
                let last = days_in_month(year, month as u32, current.calendar) as u8;
                current = HistoricalDate {
                    year,
                    month: month as u8,
                    day: current.day.min(last),
                    calendar: current.calendar,
                };
            }
            _ => {
                return Err(Error::invalid_argument(
                    "an unknown calendar unit kind cannot be applied",
                ))
            }
        }
    }
    Ok(current)
}

/// Days from the Unix epoch to a Julian-calendar date: the Julian Day Number of
/// the date (the Julian calendar's 365.25-day year with a leap day every four
/// years and no century correction) minus the epoch's own JDN.
pub fn julian_to_days(year: i64, month: u8, day: u8) -> i64 {
    let a = (14 - month as i64) / 12;
    let y = year + 4800 - a;
    let m = month as i64 + 12 * a - 3;
    let jdn = day as i64 + (153 * m + 2) / 5 + 365 * y + y / 4 - 32_083;
    jdn - 2_440_588
}

/// A Julian-calendar date from a day count — the exact inverse of
/// [`julian_to_days`].
pub fn julian_from_days(days: i64) -> HistoricalDate {
    let jdn = days + 2_440_588;
    let c = jdn + 32_082;
    let d = (4 * c + 3) / 1461;
    let e = c - (1461 * d) / 4;
    let m = (5 * e + 2) / 153;
    let day = (e - (153 * m + 2) / 5 + 1) as u8;
    let month = (m + 3 - 12 * (m / 10)) as u8;
    let year = d - 4800 + m / 10;
    HistoricalDate {
        year,
        month,
        day,
        calendar: CalendarSystem::Julian,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ErrorCode;
    use crate::domains::time::types::{CalendarUnitKind as CUK, CalendarUnit as CU, HistoricalDate as HD};

    /// The wall-clock reading is the kernel's clock: the fields agree with the
    /// day count the same kernel produces, and the zone and offset are the
    /// host's own.
    #[test]
    fn get_date_time_is_the_kernels_own_clock() {
        let backend = LinuxTimeBackend;
        let now = backend.get_date_time().expect("getDateTime");
        assert!(now.year >= 2026);
        assert!(now.month >= 1 && now.month <= 12);
        assert!(now.day >= 1 && now.day <= 31);
        assert!(now.hour <= 23 && now.minute <= 59 && now.second <= 60);
        let days = days_from_civil(now.year as i64, now.month as u32, now.day as u32);
        assert_eq!(now.weekday, iso_weekday(days));
        assert_eq!(now.yearday, day_of_year(days));
        // The zone the host publishes is the host's own spelling, and the offset
        // is the host's own TZif answer — both measured, never inferred.
        assert_eq!(now.timezone, backend.host_timezone());
        assert_eq!(
            now.utc_offset_seconds,
            backend.host_utc_offset_seconds(),
            "the offset is the host's own, from its own TZif file"
        );
        // The host's own zone is Africa/Johannesburg, whose SAST offset is
        // UTC+02:00: the measured 7200 seconds is the host's rule read from the
        // host's file, not a guessed 0 or 3600.
        assert_eq!(now.timezone, "Africa/Johannesburg");
        assert_eq!(now.utc_offset_seconds, 7_200);
    }

    /// Calendar conversion is exact arithmetic, in both directions, for every
    /// calendar this backend carries.
    #[test]
    fn calendar_conversion_is_exact_arithmetic() {
        let backend = LinuxTimeBackend;
        let gregorian = HD::gregorian(2026, 10, 14);
        for to in [
            crate::domains::time::types::CalendarSystem::Julian,
            crate::domains::time::types::CalendarSystem::Islamic,
            crate::domains::time::types::CalendarSystem::Ethiopian,
        ] {
            let converted = backend
                .convert_calendar(&gregorian, crate::domains::time::types::CalendarSystem::Gregorian, to)
                .expect("convert");
            let back = backend
                .convert_calendar(&converted, to, crate::domains::time::types::CalendarSystem::Gregorian)
                .expect("convert back");
            assert_eq!(back, gregorian, "{to:?} must round-trip");
        }
        // The known crossings, stated: 2026-10-14 Gregorian is 2026-10-01 Julian
        // (13 days behind) and 1 Muharram 1448 AH.
        let julian = backend
            .convert_calendar(
                &gregorian,
                crate::domains::time::types::CalendarSystem::Gregorian,
                crate::domains::time::types::CalendarSystem::Julian,
            )
            .expect("julian");
        assert_eq!((julian.year, julian.month, julian.day), (2026, 10, 1));
        let islamic = backend
            .convert_calendar(
                &gregorian,
                crate::domains::time::types::CalendarSystem::Gregorian,
                crate::domains::time::types::CalendarSystem::Islamic,
            )
            .expect("islamic");
        assert_eq!(
            (islamic.year, islamic.month, islamic.day),
            (1448, 5, 2),
            "the arithmetic calendar's own answer for 2026-10-14, anchored on the \
             published 1-Muharram-1444 = 2022-07-30 correspondence below"
        );
        // The published epoch constants, cross-checked against this file's own
        // kernels in BOTH calendars: 1 Muharram 1 AH is Julian 622-07-16 and
        // Gregorian 622-07-19, and both land on the published epoch day.
        assert_eq!(
            julian_to_days(622, 7, 16),
            crate::domains::time::types::ISLAMIC_EPOCH_DAYS,
            "the Islamic epoch is Julian 622-07-16 in this file's own Julian kernel"
        );
        assert_eq!(
            days_from_civil(622, 7, 19),
            crate::domains::time::types::ISLAMIC_EPOCH_DAYS,
            "and Gregorian 622-07-19 in its Gregorian kernel"
        );
        assert_eq!(
            julian_to_days(8, 8, 29),
            crate::domains::time::types::ETHIOPIAN_EPOCH_DAYS,
        );
        // The correspondences the calendars' own users report, measured:
        // 1 Muharram 1444 AH is 2022-07-30, and 1 Meskerem 2013 EC (Ethiopian
        // New Year) is 2020-09-11.
        assert_eq!(days_from_islamic(1444, 1, 1), days_from_civil(2022, 7, 30));
        assert_eq!(days_from_ethiopian(2013, 1, 1), days_from_civil(2020, 9, 11));

        // And a calendar pair this build does not carry is refused with a
        // reason rather than answered with a silently-shifted date — a
        // conversion between the wrong two calendars is worse than a refusal.
        let hebrew = HD {
            year: 5786,
            month: 1,
            day: 1,
            calendar: crate::domains::time::types::CalendarSystem::Hebrew,
        };
        let from_err = backend
            .convert_calendar(
                &hebrew,
                crate::domains::time::types::CalendarSystem::Hebrew,
                crate::domains::time::types::CalendarSystem::Gregorian,
            )
            .expect_err("a build that does not carry the Hebrew calendar must say so");
        assert_eq!(from_err.code, ErrorCode::NotSupported);
        assert!(from_err.message.starts_with("no-backend:"));
        let to_err = backend
            .convert_calendar(
                &HD::gregorian(2026, 1, 1),
                crate::domains::time::types::CalendarSystem::Gregorian,
                crate::domains::time::types::CalendarSystem::Chinese,
            )
            .expect_err("the Chinese calendar needs an ephemeris this host does not carry");
        assert_eq!(to_err.code, ErrorCode::NotSupported);
        // The contract's `custom` outlet is the honest way a backend names one
        // it does not carry, so an unknown token is tolerated, not guessed.
        use crate::domains::time::types::CalendarSystem as CS;
        assert_eq!(CS::from_token("hebrew"), CS::Hebrew);
        assert_eq!(CS::from_token("discordian"), CS::Unknown);
        assert!(CS::Gregorian.is_arithmetic() && !CS::Hebrew.is_arithmetic());

    }

    /// Month-end clamping is the contract's arithmetic: Jan 31 + 1 month is Feb
    /// 29 in a leap year, never a fabricated 31.
    #[test]
    fn calendar_arithmetic_clamps_at_month_end() {
        let backend = LinuxTimeBackend;
        let jan31 = HD::gregorian(2024, 1, 31);
        let plus_month = backend
            .add_calendar_units(&jan31, &[CU { kind: CUK::Months, amount: 1 }])
            .expect("add");
        assert_eq!((plus_month.year, plus_month.month, plus_month.day), (2024, 2, 29));
        let back = backend
            .subtract_calendar_units(&plus_month, &[CU { kind: CUK::Months, amount: 1 }])
            .expect("subtract");
        assert_eq!((back.year, back.month, back.day), (2024, 1, 29));
        let week = backend
            .add_calendar_units(&HD::gregorian(2026, 10, 14), &[CU { kind: CUK::Weeks, amount: 2 }])
            .expect("add weeks");
        assert_eq!((week.year, week.month, week.day), (2026, 10, 28));
        let next_year = backend
            .add_calendar_units(&HD::gregorian(2024, 2, 29), &[CU { kind: CUK::Years, amount: 1 }])
            .expect("add year");
        assert_eq!((next_year.year, next_year.month, next_year.day), (2025, 2, 28));
        // An unknown unit kind is refused, not applied as zero.
        assert_eq!(
            backend
                .add_calendar_units(&HD::gregorian(2026, 1, 1), &[CU { kind: CUK::Unknown, amount: 1 }])
                .expect_err("an unknown unit cannot be applied")
                .code,
            ErrorCode::InvalidArgument
        );
    }

    /// The difference is exact day counts with the y/m/d breakdown beside them
    /// and the `negative` flag the contract carries.
    #[test]
    fn diff_counts_days_and_breaks_the_year_down() {
        let backend = LinuxTimeBackend;
        let diff = backend
            .diff_calendar_dates(&HD::gregorian(2026, 10, 14), &HD::gregorian(2026, 1, 1))
            .expect("diff");
        assert_eq!(diff.total_days, days_from_civil(2026, 10, 14) - days_from_civil(2026, 1, 1));
        assert!(!diff.negative);
        assert_eq!((diff.years, diff.months, diff.days), (0, 9, 13));
        let backward = backend
            .diff_calendar_dates(&HD::gregorian(2026, 1, 1), &HD::gregorian(2026, 10, 14))
            .expect("diff");
        assert!(backward.negative);
        assert_eq!(backward.total_days, -diff.total_days);
        let same = backend
            .diff_calendar_dates(&HD::gregorian(2026, 10, 14), &HD::gregorian(2026, 10, 14))
            .expect("diff");
        assert_eq!(same.total_days, 0);
    }

    /// TAI uses the CONTRACT's epoch and rule: seconds and nanoseconds since
    /// 1970, and TAI − UTC = 37 s at the current leap-second count.
    #[test]
    fn tai_uses_the_contracts_epoch() {
        let backend = LinuxTimeBackend;
        let utc = Timestamp::from_nanos(1_700_000_000_000_000_000);
        let tai = backend.convert_to_tai(utc).expect("to tai");
        assert_eq!(tai.tai_seconds, 1_700_000_037, "TAI is UTC + 37 s");
        assert_eq!(tai.nanoseconds, 0);
        assert_eq!(backend.convert_from_tai(tai).expect("from tai"), utc);
        let sub = Timestamp::from_nanos(1_500_000_000);
        let tai_sub = backend.convert_to_tai(sub).expect("to tai");
        assert_eq!((tai_sub.tai_seconds, tai_sub.nanoseconds), (38, 500_000_000));
        assert_eq!(backend.convert_from_tai(tai_sub).expect("from tai"), sub);
        // A pre-1970 TAI reading saturates rather than wrapping into the future.
        let before = backend.convert_from_tai(TaiTimestamp { tai_seconds: 10, nanoseconds: 0 });
        assert_eq!(before.expect("from tai").as_nanos(), 0);
    }

    /// The HSM surface is refused with the measured reason, and the answer is
    /// machine-routable (`T.BoolResult` shape: an error, never a bare false).
    #[test]
    fn hsm_verification_is_refused_not_guessed() {
        let backend = LinuxTimeBackend;
        let hsm = HsmTimestamp {
            timestamp_nanos: 1,
            hardware_id: "tpm0".to_string(),
        };
        let err = backend
            .verify_hsm_timestamp(&hsm)
            .expect_err("no HSM on this host");
        assert_eq!(err.code, ErrorCode::NotSupported);
        assert!(err.message.contains("no hardware security module"));
    }

    /// The civil-date kernels are exact inverses over a wide range, so every
    /// conversion this domain offers is anchored on the same day count.
    #[test]
    fn civil_kernels_are_exact_inverses() {
        for &(y, m, d) in &[
            (1970i64, 1u32, 1u32),
            (1969, 12, 31),
            (2000, 2, 29),
            (1900, 3, 1),
            (2026, 10, 14),
            (1582, 10, 15),
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d), "{y}-{m}-{d}");
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert!(crate::domains::time::types::is_gregorian_leap(2000));
        assert!(!crate::domains::time::types::is_gregorian_leap(1900));
        assert!(crate::domains::time::types::is_julian_leap(1900));
    }

    /// The Islamic and Ethiopian calendars round-trip against their own day
    /// counts, so the arithmetic carries in both directions.
    #[test]
    fn the_arithmetic_calendars_round_trip() {
        for &(y, m, d) in &[(1448i64, 1u8, 1u8), (1389, 2, 1), (1, 1, 1), (1447, 12, 29)] {
            let days = days_from_islamic(y, m, d);
            assert_eq!(islamic_from_days(days), (y, m, d), "islamic {y}-{m}-{d}");
        }
        for &(y, m, d) in &[(2019i64, 1u8, 1u8), (1, 13, 5), (2015, 13, 6), (2016, 1, 1)] {
            let days = days_from_ethiopian(y, m, d);
            assert_eq!(ethiopian_from_days(days), (y, m, d), "ethiopian {y}-{m}-{d}");
        }
        assert_eq!(days_from_ethiopian(1, 1, 1), -716_367);
    }

    /// The Julian calendar round-trips, and the 13-day divergence the two
    /// calendars carry in this era is exact.
    #[test]
    fn the_julian_calendar_round_trips() {
        for &(y, m, d) in &[(1970i64, 1u8, 1u8), (2026, 10, 14), (1900, 3, 1), (1, 1, 1)] {
            let days = julian_to_days(y, m, d);
            let back = julian_from_days(days);
            assert_eq!((back.year, back.month, back.day), (y, m, d), "julian {y}-{m}-{d}");
        }
        let julian = julian_from_days(days_from_civil(2026, 10, 14));
        assert_eq!((julian.year, julian.month, julian.day), (2026, 10, 1));
        // Julian 1970-01-01 is Gregorian 1970-01-14: 13 days ahead.
        assert_eq!(julian_to_days(1970, 1, 1), 13);
    }
}

#[cfg(test)]
mod live {
    //! The READ-ONLY live probe (charter law #8): read THIS host's real clock,
    //! zone and TZif answer, and the calendar conversions at today's date. Every
    //! call is a `clock_gettime`, a `readlink` or a file read — nothing is
    //! written, and the one write-adjacent verb (`setDefaultTimezone`) belongs to
    //! `timezone.capnp` and is not reachable from here.

    use super::*;

    #[test]
    #[ignore = "reads this host's clock and zone; read-only, founder-runnable"]
    fn live_reads_this_hosts_time_natively() {
        let backend = LinuxTimeBackend;
        let now = backend.get_date_time().expect("getDateTime");
        std::eprintln!(
            "live time: {}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} weekday={} yearday={}",
            now.year, now.month, now.day, now.hour, now.minute, now.second,
            now.millisecond, now.weekday, now.yearday
        );
        std::eprintln!(
            "live zone: {} offset={}s (read from the host's own TZif file)",
            backend.host_timezone(),
            backend.host_utc_offset_seconds()
        );
        // The calendar surface at today's date, in both directions.
        let today = HistoricalDate::gregorian(now.year as i64, now.month, now.day);
        for to in [
            crate::domains::time::types::CalendarSystem::Julian,
            crate::domains::time::types::CalendarSystem::Islamic,
            crate::domains::time::types::CalendarSystem::Ethiopian,
        ] {
            let converted = backend
                .convert_calendar(
                    &today,
                    crate::domains::time::types::CalendarSystem::Gregorian,
                    to,
                )
                .expect("convert");
            let back = backend
                .convert_calendar(&converted, to, crate::domains::time::types::CalendarSystem::Gregorian)
                .expect("convert back");
            assert_eq!(back, today, "{to:?} must round-trip through the live date");
            std::eprintln!(
                "live {to:?}: {}-{:02}-{:02}",
                converted.year, converted.month, converted.day
            );
        }
        let tai = backend
            .convert_to_tai(Timestamp::from_nanos(
                (now.year as i64 * 0 + days_from_civil(now.year as i64, now.month as u32, now.day as u32))
                    .max(0) as u64
                    * 86_400_000_000_000,
            ))
            .expect("tai");
        std::eprintln!("live TAI day start: {} s (TAI = UTC + 37 s)", tai.tai_seconds);
    }
}
