//! Owned value types the **time** domain speaks (the types a caller actually
//! touches), plus their conversions to and from the frozen `time.capnp`.
//!
//! Law #7: the capnp state structs are generated; this module is the
//! *hand-written* mirror used at the domain's public/backend seam.
//!
//! ## The epoch law this file exists to hold
//!
//! `time.capnp` declares **no** TAI type: `TaiTimestamp` lives in
//! `timezone.capnp`, on the **Unix epoch (1970)** — the same epoch as
//! `T.Timestamp`. The pre-revamp contract carried a second `TAITimestamp` on a
//! 1958 epoch, one letter-case away, both bare `UInt64` seconds, so a consumer
//! could pass one to the other's converter and receive a timestamp ~120 years
//! wrong with no compile error. This domain's TAI helpers therefore use the
//! contract's epoch and its converter's rule (TAI − UTC = 37 s at the current
//! leap-second count), and the canonical home of that surface is named in the
//! docs so a consumer never reaches for a second spelling.

use alloc::string::{String, ToString};

use crate::schema::time_capnp;

/// The calendar systems the contract names (`CalendarSystem`), as an enum in
/// Rust so the set is closed and typed. The `custom` arm is the contract's own
/// outlet for a calendar nobody has named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalendarSystem {
    /// The neutral sentinel: an absent calendar never means "gregorian".
    Unknown,
    /// The proleptic Gregorian calendar.
    Gregorian,
    /// The Julian calendar.
    Julian,
    /// The arithmetic (tabular) Islamic calendar.
    Islamic,
    /// The Hebrew calendar.
    Hebrew,
    /// The Chinese lunisolar calendar (needs astronomical new moons).
    Chinese,
    /// The Ethiopian calendar.
    Ethiopian,
    /// A calendar this schema was frozen before.
    Custom,
}

impl CalendarSystem {
    /// All eight, in schema order.
    pub const ALL: [CalendarSystem; 8] = [
        CalendarSystem::Unknown,
        CalendarSystem::Gregorian,
        CalendarSystem::Julian,
        CalendarSystem::Islamic,
        CalendarSystem::Hebrew,
        CalendarSystem::Chinese,
        CalendarSystem::Ethiopian,
        CalendarSystem::Custom,
    ];

    /// The contract ordinal (the value that never moves).
    pub const fn ordinal(self) -> u16 {
        match self {
            CalendarSystem::Unknown => 0,
            CalendarSystem::Gregorian => 1,
            CalendarSystem::Julian => 2,
            CalendarSystem::Islamic => 3,
            CalendarSystem::Hebrew => 4,
            CalendarSystem::Chinese => 5,
            CalendarSystem::Ethiopian => 6,
            CalendarSystem::Custom => 7,
        }
    }

    /// The open token the CLI renders.
    pub const fn as_token(self) -> &'static str {
        match self {
            CalendarSystem::Unknown => "unknown",
            CalendarSystem::Gregorian => "gregorian",
            CalendarSystem::Julian => "julian",
            CalendarSystem::Islamic => "islamic",
            CalendarSystem::Hebrew => "hebrew",
            CalendarSystem::Chinese => "chinese",
            CalendarSystem::Ethiopian => "ethiopian",
            CalendarSystem::Custom => "custom",
        }
    }

    /// Parse a token. An unrecognised token maps to `Unknown` — never to a
    /// guessed calendar, because a conversion between the wrong two calendars is
    /// silently wrong in a way no consumer can detect.
    pub fn from_token(token: &str) -> Self {
        match token {
            "gregorian" => CalendarSystem::Gregorian,
            "julian" => CalendarSystem::Julian,
            "islamic" => CalendarSystem::Islamic,
            "hebrew" => CalendarSystem::Hebrew,
            "chinese" => CalendarSystem::Chinese,
            "ethiopian" => CalendarSystem::Ethiopian,
            _ => CalendarSystem::Unknown,
        }
    }

    /// Write into the generated `CalendarSystem` field.
    pub fn to_generated(self) -> time_capnp::CalendarSystem {
        match self {
            CalendarSystem::Unknown => time_capnp::CalendarSystem::Unknown,
            CalendarSystem::Gregorian => time_capnp::CalendarSystem::Gregorian,
            CalendarSystem::Julian => time_capnp::CalendarSystem::Julian,
            CalendarSystem::Islamic => time_capnp::CalendarSystem::Islamic,
            CalendarSystem::Hebrew => time_capnp::CalendarSystem::Hebrew,
            CalendarSystem::Chinese => time_capnp::CalendarSystem::Chinese,
            CalendarSystem::Ethiopian => time_capnp::CalendarSystem::Ethiopian,
            CalendarSystem::Custom => time_capnp::CalendarSystem::Custom,
        }
    }

    /// Read out of the generated `CalendarSystem` enum.
    pub fn from_generated(system: time_capnp::CalendarSystem) -> Self {
        match system {
            time_capnp::CalendarSystem::Unknown => CalendarSystem::Unknown,
            time_capnp::CalendarSystem::Gregorian => CalendarSystem::Gregorian,
            time_capnp::CalendarSystem::Julian => CalendarSystem::Julian,
            time_capnp::CalendarSystem::Islamic => CalendarSystem::Islamic,
            time_capnp::CalendarSystem::Hebrew => CalendarSystem::Hebrew,
            time_capnp::CalendarSystem::Chinese => CalendarSystem::Chinese,
            time_capnp::CalendarSystem::Ethiopian => CalendarSystem::Ethiopian,
            time_capnp::CalendarSystem::Custom => CalendarSystem::Custom,
        }
    }

    /// Whether this build can convert to and from that calendar on its own (a
    /// pure-arithmetic calendar). The two that need more than arithmetic are
    /// named honestly in the availability matrix rather than guessed.
    pub const fn is_arithmetic(self) -> bool {
        matches!(
            self,
            CalendarSystem::Gregorian
                | CalendarSystem::Julian
                | CalendarSystem::Islamic
                | CalendarSystem::Ethiopian
        )
    }
}

/// The calendar units the contract names (`CalendarUnitKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalendarUnitKind {
    /// The neutral sentinel.
    Unknown,
    /// Days.
    Days,
    /// Weeks.
    Weeks,
    /// Months.
    Months,
    /// Years.
    Years,
    /// A unit nobody has named.
    Custom,
}

impl CalendarUnitKind {
    /// Write into the generated `CalendarUnitKind` field.
    pub fn to_generated(self) -> time_capnp::CalendarUnitKind {
        match self {
            CalendarUnitKind::Unknown => time_capnp::CalendarUnitKind::Unknown,
            CalendarUnitKind::Days => time_capnp::CalendarUnitKind::Days,
            CalendarUnitKind::Weeks => time_capnp::CalendarUnitKind::Weeks,
            CalendarUnitKind::Months => time_capnp::CalendarUnitKind::Months,
            CalendarUnitKind::Years => time_capnp::CalendarUnitKind::Years,
            CalendarUnitKind::Custom => time_capnp::CalendarUnitKind::Custom,
        }
    }
}

/// One calendar arithmetic unit (`CalendarUnit`): a kind and an amount (the
/// amount may be negative, which is a subtraction stated as an addition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalendarUnit {
    /// Which unit.
    pub kind: CalendarUnitKind,
    /// How many (may be negative).
    pub amount: i32,
}

impl CalendarUnit {
    /// The neutral unit: `unknown` with a zero amount — the value a caller reads
    /// when no unit was named, never a guessed "days".
    pub const NEUTRAL: CalendarUnit = CalendarUnit {
        kind: CalendarUnitKind::Unknown,
        amount: 0,
    };

    /// Write into a generated `CalendarUnit` builder.
    pub fn write_to(&self, builder: &mut time_capnp::calendar_unit::Builder<'_>) {
        builder.set_kind(self.kind.to_generated());
        builder.set_amount(self.amount);
    }
}

/// One date in one calendar (`HistoricalDate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoricalDate {
    /// The year in the named calendar's own numbering (may be negative).
    pub year: i64,
    /// The month number (1-based in every calendar this build converts).
    pub month: u8,
    /// The day of the month (1-based).
    pub day: u8,
    /// Which calendar this date is expressed in.
    pub calendar: CalendarSystem,
}

impl HistoricalDate {
    /// The neutral date: the epoch in no named calendar. A caller that reads this
    /// has been handed nothing, which is the honest answer — never a gregorian
    /// date it might mistake for a real one.
    pub const NEUTRAL: HistoricalDate = HistoricalDate {
        year: 0,
        month: 0,
        day: 0,
        calendar: CalendarSystem::Unknown,
    };

    /// A gregorian date — the common case, stated without ceremony.
    pub const fn gregorian(year: i64, month: u8, day: u8) -> Self {
        HistoricalDate {
            year,
            month,
            day,
            calendar: CalendarSystem::Gregorian,
        }
    }

    /// Write into a generated `HistoricalDate` builder.
    pub fn write_to(&self, builder: &mut time_capnp::historical_date::Builder<'_>) {
        builder.set_year(self.year);
        builder.set_month(self.month);
        builder.set_day(self.day);
        builder.set_calendar(self.calendar.to_generated());
    }

    /// Read from a generated `HistoricalDate` reader.
    pub fn from_reader(reader: &time_capnp::historical_date::Reader<'_>) -> Self {
        HistoricalDate {
            year: reader.get_year(),
            month: reader.get_month(),
            day: reader.get_day(),
            calendar: CalendarSystem::from_generated(
                reader.get_calendar().unwrap_or(time_capnp::CalendarSystem::Unknown),
            ),
        }
    }
}

/// The difference between two dates (`CalendarDiff`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalendarDiff {
    /// Whole years between the two dates.
    pub years: i32,
    /// Whole months beyond the years.
    pub months: i32,
    /// Days beyond the months.
    pub days: i32,
    /// The total day count (independent of the breakdown).
    pub total_days: i64,
    /// Which way the difference goes (date1 − date2).
    pub negative: bool,
}

/// A wall-clock reading in a named zone (`DateTime`), as the backend measured
/// it: every field is a reading, and a field the host does not publish is the
/// neutral value rather than a guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateTime {
    /// The year in the zone's calendar (gregorian, as the host numbers it).
    pub year: i32,
    /// 1–12.
    pub month: u8,
    /// 1–31.
    pub day: u8,
    /// 0–23.
    pub hour: u8,
    /// 0–59.
    pub minute: u8,
    /// 0–60 (the leap second the kernel itself can report).
    pub second: u8,
    /// 0–999.
    pub millisecond: u16,
    /// 1–7 with Monday = 1 (ISO numbering), 0 when the host did not say.
    pub weekday: u8,
    /// 1–366, 0 when the host did not say.
    pub yearday: u16,
    /// The zone the backend applied, as an open IANA-ish token ("UTC",
    /// "Africa/Johannesburg"); empty when the host publishes none.
    pub timezone: String,
    /// The offset the backend applied, in seconds east of UTC (0 when unknown).
    pub utc_offset_seconds: i32,
}

impl DateTime {
    /// Write into a generated `DateTime` builder.
    pub fn write_to(&self, builder: &mut time_capnp::date_time::Builder<'_>) {
        builder.set_year(self.year);
        builder.set_month(self.month);
        builder.set_day(self.day);
        builder.set_hour(self.hour);
        builder.set_minute(self.minute);
        builder.set_second(self.second);
        builder.set_millisecond(self.millisecond);
        builder.set_weekday(self.weekday);
        builder.set_yearday(self.yearday);
        builder.set_timezone(&self.timezone);
        builder.set_utc_offset_seconds(self.utc_offset_seconds);
    }

    /// Read from a generated `DateTime` reader.
    pub fn from_reader(reader: &time_capnp::date_time::Reader<'_>) -> Self {
        DateTime {
            year: reader.get_year(),
            month: reader.get_month(),
            day: reader.get_day(),
            hour: reader.get_hour(),
            minute: reader.get_minute(),
            second: reader.get_second(),
            millisecond: reader.get_millisecond(),
            weekday: reader.get_weekday(),
            yearday: reader.get_yearday(),
            timezone: text_field(reader.get_timezone()),
            utc_offset_seconds: reader.get_utc_offset_seconds(),
        }
    }
}

/// An HSM-attested timestamp (`HSMTimestamp`) — the hardware reference, never
/// the attestation itself, which stays in the host's own material.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HsmTimestamp {
    /// The moment the attestation covers.
    pub timestamp_nanos: u64,
    /// The hardware's identifier, as an open token.
    pub hardware_id: String,
}

/// The contract's TAI representation, as `timezone.capnp` declares it: seconds
/// **and** nanoseconds since the **Unix epoch (1970)**, metadatable. Mirrored
/// here so a caller holding only a `Time` reference can convert without
/// reaching for a 1958-epoched second spelling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaiTimestamp {
    /// Seconds since 1970-01-01 00:00:00 TAI.
    pub tai_seconds: u64,
    /// Nanoseconds within the second.
    pub nanoseconds: u32,
}

/// The current count of leap seconds between TAI and UTC, as the contract's
/// converter (and the IERS bulletin this host's kernel follows) states.
///
/// The last announced leap second was 2016-12-31 (the 27th, bringing the total
/// to 37); no announcement has added one since, and `timezone.capnp`'s
/// `getLeapSeconds` is the canonical home for a table a host may grow.
pub const LEAP_SECONDS_TAI_MINUS_UTC: u64 = 37;

/// Convert UTC nanoseconds-since-1970 to the contract's `TaiTimestamp`, using
/// **the contract's own epoch and rule** (TAI = UTC + 37 s at the current
/// leap-second count), never a chrono-style epoch of our own.
pub fn utc_nanos_to_tai(utc_nanos: u64) -> TaiTimestamp {
    let (seconds, nanos) = split_nanos(utc_nanos);
    TaiTimestamp {
        tai_seconds: seconds + LEAP_SECONDS_TAI_MINUS_UTC,
        nanoseconds: nanos,
    }
}

/// Convert the contract's `TaiTimestamp` back to UTC nanoseconds-since-1970.
pub fn tai_to_utc_nanos(tai: TaiTimestamp) -> u64 {
    if tai.tai_seconds < LEAP_SECONDS_TAI_MINUS_UTC {
        // A TAI reading that predates the UTC epoch has no UTC instant: it
        // saturates at 0 rather than wrapping into the far future.
        return 0;
    }
    (tai.tai_seconds - LEAP_SECONDS_TAI_MINUS_UTC)
        .saturating_mul(1_000_000_000)
        .saturating_add(tai.nanoseconds as u64)
}

/// Split nanoseconds into (whole seconds, nanoseconds-within-second) the way
/// the contract's two types do.
fn split_nanos(nanos: u64) -> (u64, u32) {
    (nanos / 1_000_000_000, (nanos % 1_000_000_000) as u32)
}

/// Days from the Unix epoch (1970-01-01) to a civil date — Howard Hinnant's
/// `days_from_civil`, the same arithmetic `timezone.capnp`'s converters sit on,
/// so this domain and the contract's own converters cannot drift apart.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let mp = ((month + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// A civil date from days-since-epoch — the exact inverse of
/// [`days_from_civil`], so every conversion in this domain round-trips.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Whether a year is a leap year in the proleptic Gregorian calendar.
pub const fn is_gregorian_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Whether a year is a leap year in the Julian calendar (every four years,
/// without the Gregorian century correction).
pub const fn is_julian_leap(year: i64) -> bool {
    year % 4 == 0
}

/// The number of days in a month, in one of the two solar calendars this build
/// converts.
pub fn days_in_month(year: i64, month: u32, calendar: CalendarSystem) -> u32 {
    let leap = match calendar {
        CalendarSystem::Julian => is_julian_leap(year),
        _ => is_gregorian_leap(year),
    };
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        // An out-of-range month is the caller's error, not a fabricated 31.
        _ => 0,
    }
}

/// The weekday of a day count, 1–7 with Monday = 1 (the contract's ISO
/// numbering): 1970-01-01 was a Thursday (4).
pub fn iso_weekday(days_since_epoch: i64) -> u8 {
    // 1970-01-01 was a Thursday, which is 4 in the ISO numbering where Monday is
    // 1; the rem_euclid keeps a pre-epoch day count from producing a negative.
    (((days_since_epoch + 3).rem_euclid(7)) + 1) as u8
}

/// The day of the year (1–366) of a day count.
pub fn day_of_year(days_since_epoch: i64) -> u16 {
    let (year, _, _) = civil_from_days(days_since_epoch);
    let start = days_from_civil(year, 1, 1);
    (days_since_epoch - start + 1) as u16
}

// ── The Islamic (tabular) and Ethiopian arithmetic calendars ────────────

/// The Islamic epoch: 1 Muharram 1 AH = **Julian 622-07-16** = Julian Day Number
/// 1 948 440 = day **−492 148** from the Unix epoch.
///
/// The constant is the published one and is cross-checked, not asserted: the
/// same instant in this file's own Julian kernel (`julian_to_days(622, 7, 16)`)
/// and in its Gregorian kernel (`days_from_civil(622, 7, 19)`) both land on it,
/// and 1 Muharram 1444 AH lands on 2022-07-30, which is the correspondence the
/// calendar's own users report.
pub const ISLAMIC_EPOCH_DAYS: i64 = -492_148;
/// The Ethiopian epoch: 1 Meskerem 1 EC = **Julian 0008-08-29** = Julian Day
/// Number 1 724 221 = day **−716 367** from the Unix epoch.
///
/// The published constant, cross-checked the same way: 1 Meskerem 2013 EC lands
/// on 2020-09-11 Gregorian, which is the Ethiopian New Year every calendar
/// user knows.
pub const ETHIOPIAN_EPOCH_DAYS: i64 = -716_367;
/// The Islamic tabular cycle: 30 years with 11 leap years.
pub const ISLAMIC_CYCLE_YEARS: i64 = 30;
/// Days in a 30-year Islamic cycle.
pub const ISLAMIC_CYCLE_DAYS: i64 = 10_631;

/// Days in one month of the tabular Islamic calendar: the odd months have 30
/// days, the even months 29, and the twelfth has 30 in a leap year.
pub const fn islamic_month_days(year: i64, month: u8) -> i64 {
    if month % 2 == 1 || (month == 12 && islamic_is_leap_year(year)) {
        30
    } else {
        29
    }
}

/// A days-from-epoch into the tabular Islamic calendar, walking forward from the
/// epoch (and backward for a date that precedes it) one year and one month at a
/// time — exact arithmetic, no table, no ephemeris.
pub fn islamic_from_days(days: i64) -> (i64, u8, u8) {
    let mut remaining = days - ISLAMIC_EPOCH_DAYS;
    let mut year: i64 = 1;
    if remaining >= 0 {
        while remaining >= islamic_year_days(year) {
            remaining -= islamic_year_days(year);
            year += 1;
        }
    } else {
        while remaining < 0 {
            year -= 1;
            remaining += islamic_year_days(year);
        }
    }
    let mut month: u8 = 1;
    while month < 12 && remaining >= islamic_month_days(year, month) {
        remaining -= islamic_month_days(year, month);
        month += 1;
    }
    (year, month, (remaining + 1) as u8)
}

/// Days from the epoch to a tabular Islamic date.
pub fn days_from_islamic(year: i64, month: u8, day: u8) -> i64 {
    let mut days = ISLAMIC_EPOCH_DAYS;
    for y in 1..year {
        days += islamic_year_days(y);
    }
    for m in 1..month {
        days += islamic_month_days(year, m);
    }
    days + day as i64 - 1
}

/// Whether a year is a leap year in the tabular Islamic cycle (year numbers
/// 2, 5, 7, 10, 13, 16, 18, 21, 24, 26 and 29 of each 30-year cycle).
pub const fn islamic_is_leap_year(year: i64) -> bool {
    let cycle_year = ((year - 1).rem_euclid(ISLAMIC_CYCLE_YEARS) + 1) as i64;
    matches!(cycle_year, 2 | 5 | 7 | 10 | 13 | 16 | 18 | 21 | 24 | 26 | 29)
}

/// Days in one tabular Islamic year.
pub const fn islamic_year_days(year: i64) -> i64 {
    if islamic_is_leap_year(year) {
        355
    } else {
        354
    }
}

/// A days-from-epoch to an Ethiopian date (13 months, the 13th of 5 or 6 days).
pub fn ethiopian_from_days(days: i64) -> (i64, u8, u8) {
    let d = days - ETHIOPIAN_EPOCH_DAYS;
    let mut year = (d / 1461) * 4;
    let mut remaining = d % 1461;
    while remaining >= ethiopian_year_days(year + 1) {
        remaining -= ethiopian_year_days(year + 1);
        year += 1;
    }
    let mut month = 1u8;
    while remaining >= 30 {
        remaining -= 30;
        month += 1;
    }
    (year + 1, month.min(13), remaining as u8 + 1)
}

/// Days from the epoch to an Ethiopian date.
pub fn days_from_ethiopian(year: i64, month: u8, day: u8) -> i64 {
    let mut days = ETHIOPIAN_EPOCH_DAYS;
    for y in 1..year {
        days += ethiopian_year_days(y);
    }
    for _ in 1..month {
        days += 30;
    }
    days + day as i64 - 1
}

/// Days in an Ethiopian year: 365, or 366 in the year before a leap year (the
/// Ethiopian rule, every four years without exception).
pub const fn ethiopian_year_days(year: i64) -> i64 {
    if year % 4 == 3 {
        366
    } else {
        365
    }
}

/// Read a `Text` field into an owned string (absent/invalid UTF-8 is empty —
/// the same equivalence the writers emit, so a round-trip stays total).
pub(crate) fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

#[allow(dead_code)]
fn assert_generated_types_linked() {
    // Naming (not constructing) the generated types proves the domain feature
    // compiled the time schema.
    let _: Option<time_capnp::date_time::Owned> = None;
}
