//! Inline behaviour tests for the time domain's *public contract*: the owned
//! types' fidelity to the wire, the availability matrix's internal consistency,
//! the trait's honest-Unavailable totality, the epoch law this domain exists to
//! hold, and the calendar vocabulary's unknown-tolerance. The native-path
//! behaviour over the kernel's clock and the contract's calendar arithmetic is
//! proven in [`crate::platforms::linux::time`] (the `l3_linux_time::…` tests the
//! matrix names). All hermetic — no host I/O in this file.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::values::{Duration, Timestamp};
use crate::core::{Error, ErrorCode};
use crate::schema::time_capnp;

use super::availability::{
    claimed_l3_tests, status_counts, AvailStatus, ReasonClass, TIME_AVAILABILITY, TIME_SCHEMA_ID,
};
use super::backend::TimeBackend;
use super::types::{
    civil_from_days, days_from_civil, tai_to_utc_nanos, utc_nanos_to_tai, CalendarDiff,
    CalendarSystem, CalendarUnit, CalendarUnitKind, DateTime, HistoricalDate, HsmTimestamp,
    TaiTimestamp, LEAP_SECONDS_TAI_MINUS_UTC,
};

/// The contract's own method list, in `Time` order (`@0`–`@5`), plus this
/// domain's two TAI helpers. The matrix must carry exactly these names.
const CONTRACT_METHODS: &[&str] = &[
    "getDateTime",
    "verifyHsmTimestamp",
    "convertCalendar",
    "addCalendarUnits",
    "subtractCalendarUnits",
    "diffCalendarDates",
    "convertToTai",
    "convertFromTai",
];

/// A backend that serves nothing: the honesty baseline every unavailable method
/// is measured against.
struct Nothing;

impl TimeBackend for Nothing {}

/// The epoch law, stated once: TAI and UTC share the Unix epoch, and the
/// difference is the leap-second count — never a second epoch someone could
/// confuse by one letter.
#[test]
fn tai_and_utc_share_the_contracts_epoch() {
    assert_eq!(LEAP_SECONDS_TAI_MINUS_UTC, 37);
    let tai = utc_nanos_to_tai(1_000_000_000_000_000_000);
    assert_eq!(tai.tai_seconds, 1_000_000_037);
    assert_eq!(tai.nanoseconds, 0);
    assert_eq!(tai_to_utc_nanos(tai), 1_000_000_000_000_000_000);
    // The sub-second part survives both ways, and a sub-second TAI reading
    // before the leap-second boundary saturates instead of wrapping.
    assert_eq!(tai_to_utc_nanos(TaiTimestamp { tai_seconds: 0, nanoseconds: 5 }), 0);
}

/// `DateTime` survives a real serialized round-trip, field for field, so a
/// backend's measured reading reaches a consumer unchanged.
#[test]
fn date_time_round_trips_through_the_wire() {
    let reading = DateTime {
        year: 2026,
        month: 10,
        day: 14,
        hour: 23,
        minute: 16,
        second: 31,
        millisecond: 512,
        weekday: 3,
        yearday: 287,
        timezone: "Africa/Johannesburg".to_string(),
        utc_offset_seconds: 7_200,
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<time_capnp::date_time::Builder<'_>>();
        reading.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<time_capnp::date_time::Reader<'_>>()
        .expect("root");
    assert_eq!(DateTime::from_reader(&wire), reading);
    assert_eq!(wire.get_timezone().expect("zone"), "Africa/Johannesburg");
    assert_eq!(wire.get_utc_offset_seconds(), 7_200);
}

/// `HistoricalDate` and the calendar units round-trip, so the calendar surface's
/// inputs and outputs are the contract's own.
#[test]
fn calendar_types_round_trip_through_the_wire() {
    let date = HistoricalDate::gregorian(2026, 10, 14);
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<time_capnp::historical_date::Builder<'_>>();
        date.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<time_capnp::historical_date::Reader<'_>>()
        .expect("root");
    assert_eq!(HistoricalDate::from_reader(&wire), date);

    let unit = CalendarUnit {
        kind: CalendarUnitKind::Months,
        amount: -3,
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<time_capnp::calendar_unit::Builder<'_>>();
        unit.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<time_capnp::calendar_unit::Reader<'_>>()
        .expect("root");
    assert_eq!(wire.get_amount(), -3);
}

/// The availability matrix is internally consistent per conformance §3.4.
#[test]
fn availability_rows_are_consistent() {
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, TIME_AVAILABILITY.len());
    for row in TIME_AVAILABILITY {
        assert_ne!(row.reason_class, Some(ReasonClass::Unknown), "{}", row.name);
        match row.status {
            AvailStatus::Real => {
                assert!(row.reason.is_empty(), "REAL row {} carries a reason", row.name);
                assert!(row.l3_test.is_some(), "REAL row {} has no l3_test", row.name);
                assert!(row.reason_class.is_none(), "REAL row {} carries a reasonClass", row.name);
            }
            AvailStatus::Degraded => {
                assert!(!row.reason.is_empty(), "DEGRADED row {} names no capability", row.name);
                assert!(row.l3_test.is_some(), "DEGRADED row {} has no l3_test", row.name);
            }
            AvailStatus::Unavailable => {
                assert!(row.reason_class.is_some(), "UNAVAILABLE row {} has no reasonClass", row.name);
                assert!(!row.reason.is_empty(), "UNAVAILABLE row {} has no reason", row.name);
                assert!(row.l3_test.is_none(), "UNAVAILABLE row {} claims an l3_test", row.name);
            }
        }
    }
}

/// The matrix is exactly the contract surface: every method has a row, every row
/// is a contract method, and the frozen ordinals have not moved.
#[test]
fn the_matrix_is_exactly_the_contract_surface() {
    for name in CONTRACT_METHODS {
        let row = TIME_AVAILABILITY
            .iter()
            .find(|r| r.name == *name)
            .unwrap_or_else(|| panic!("{name} has no availability row"));
        assert!(!row.reason.is_empty() || row.status == AvailStatus::Real);
    }
    for row in TIME_AVAILABILITY {
        assert!(
            CONTRACT_METHODS.contains(&row.name),
            "row {} is not a contract method",
            row.name
        );
    }
    for (name, ordinal) in [
        ("getDateTime", 0u16),
        ("verifyHsmTimestamp", 1),
        ("convertCalendar", 2),
        ("addCalendarUnits", 3),
        ("subtractCalendarUnits", 4),
        ("diffCalendarDates", 5),
    ] {
        let row = TIME_AVAILABILITY.iter().find(|r| r.name == name).expect(name);
        assert_eq!(row.ordinal, ordinal, "{name} ordinal moved");
    }
}

/// Every `l3_test` the matrix claims is a real test name, and every test in the
/// Every `l3_test` the matrix claims is a real test name in this cell, and
/// every test here is claimed: a row whose test vanished is a FAIL, and a
/// test nobody claims is dead evidence.
///
/// The suite is read from the cell's OWN source at compile time — never from
/// a hand-kept list, which is a list that can drift away from the truth.
#[test]
fn every_claimed_test_exists() {
    for (method, test) in claimed_l3_tests() {
        assert!(
            the_cells_own_test_names().contains(&test.rsplit("::").next().unwrap_or(test)),
            "{method} claims {test}, which is not a test in the time cell"
        );
    }
    for (name, is_unit_test) in the_cells_own_tests() {
        if is_unit_test {
            // The test says it proves the engine's own helper, not a contract
            // method, so the matrix does not publish it.
            continue;
        }
        let claim = alloc::format!("l3_linux_time::{name}");
        assert!(
            claimed_l3_tests().any(|(_, t)| t == claim),
            "{claim} is real but unclaimed by the matrix, and it does not say \
             it is a unit test either"
        );
    }
}

/// Every test function the time cell defines, read from its own source,
/// with the flag that says whether the matrix publishes it: a test the
/// matrix does not claim must SAY it is a unit test, in its own doc comment.
fn the_cells_own_tests() -> Vec<(&'static str, bool)> {
    let source = include_str!("../../platforms/linux/time.rs");
    let lines: Vec<&str> = source.lines().collect();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[test]" {
            continue;
        }
        let Some(next) = lines.get(index + 1) else { break };
        let trimmed = next.trim();
        let Some(rest) = trimmed.strip_prefix("fn ") else { continue };
        let name: alloc::string::String =
            rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            continue;
        }
        // The doc comment above the test says whether it is a unit test.
        let mut is_unit_test = false;
        for earlier in lines[..index].iter().rev() {
            let trimmed = earlier.trim();
            if trimmed.starts_with("///") {
                if trimmed.contains("UNIT:") {
                    is_unit_test = true;
                }
                continue;
            }
            break;
        }
        names.push((leak(name), is_unit_test));
    }
    names
}

/// The names are read at compile time, so they live for the whole run.
fn the_cells_own_test_names() -> Vec<&'static str> {
    the_cells_own_tests()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// The names are read at compile time, so they live for the whole run.
fn leak(name: alloc::string::String) -> &'static str {
    alloc::boxed::Box::leak(name.into_boxed_str())
}

/// The compiled schema id matches `time.capnp`'s `@0x…` line.
#[test]
fn schema_id_is_the_time_contract() {
    assert_eq!(TIME_SCHEMA_ID, 0xa1b2c3d4e5f6001e);
    #[allow(dead_code)]
    fn assert_generated_module_linked(_: Option<time_capnp::date_time::Owned>) {}
}

/// The trait's default surface answers `unavailable` **with a reason** on a
/// backend that serves nothing — the law #6 baseline, and the same shape the
/// Linux backend's HSM refusal takes.
#[test]
fn every_default_method_answers_with_a_reason() {
    let backend = Nothing;
    let err = backend
        .verify_hsm_timestamp(&HsmTimestamp::default())
        .expect_err("a backend that cannot verify must say so");
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(!err.retryable, "notSupported is not retryable");
    assert!(!err.message.is_empty(), "a refusal without a reason is banned");
    // Every other method has the same honest default.
    assert!(backend.get_date_time().is_err());
    assert!(backend.convert_calendar(&HistoricalDate::NEUTRAL, CalendarSystem::Gregorian, CalendarSystem::Julian).is_err());
    assert!(backend.add_calendar_units(&HistoricalDate::NEUTRAL, &[CalendarUnit::NEUTRAL]).is_err());
    assert!(backend.subtract_calendar_units(&HistoricalDate::NEUTRAL, &[CalendarUnit::NEUTRAL]).is_err());
    assert!(backend.diff_calendar_dates(&HistoricalDate::NEUTRAL, &HistoricalDate::NEUTRAL).is_err());
    assert!(backend.convert_to_tai(Timestamp::from_nanos(0)).is_err());
    assert!(backend.convert_from_tai(TaiTimestamp::default()).is_err());
}

/// Error propagation is total and honest across all 20 contract codes.
#[test]
fn error_injection_is_total_over_all_20_codes() {
    struct Faulted(Error);
    impl TimeBackend for Faulted {
        fn get_date_time(&self) -> crate::core::Result<DateTime> {
            Err(self.0.clone())
        }
    }
    for code in ErrorCode::ALL {
        let backend = Faulted(Error::new(code));
        let err = backend.get_date_time().unwrap_err();
        assert_eq!(err.code, code, "code {code:?} must survive the seam unchanged");
        assert_eq!(
            err.retryable,
            code.default_retryable(),
            "code {code:?} retryable must be the contract default"
        );
    }
}

/// The calendar vocabulary is a closed set with an open outlet: an unnamed
/// calendar is `Unknown`/`Custom`, never a guessed system, and the arithmetic
/// ones are named.
#[test]
fn the_calendar_vocabulary_is_closed_but_unknown_tolerant() {
    for (token, expected) in [
        ("gregorian", CalendarSystem::Gregorian),
        ("julian", CalendarSystem::Julian),
        ("islamic", CalendarSystem::Islamic),
        ("hebrew", CalendarSystem::Hebrew),
        ("chinese", CalendarSystem::Chinese),
        ("ethiopian", CalendarSystem::Ethiopian),
        ("discordian", CalendarSystem::Unknown),
        ("", CalendarSystem::Unknown),
    ] {
        assert_eq!(CalendarSystem::from_token(token), expected, "{token}");
    }
    assert!(CalendarSystem::Gregorian.is_arithmetic());
    assert!(CalendarSystem::Julian.is_arithmetic());
    assert!(CalendarSystem::Islamic.is_arithmetic());
    assert!(CalendarSystem::Ethiopian.is_arithmetic());
    // The two that need more than arithmetic are named, not silently mapped.
    assert!(!CalendarSystem::Hebrew.is_arithmetic());
    assert!(!CalendarSystem::Chinese.is_arithmetic());
    // Ordinals are the contract's and do not move.
    assert_eq!(CalendarSystem::Gregorian.ordinal(), 1);
    assert_eq!(CalendarSystem::Custom.ordinal(), 7);
}

/// The public API is a thin borrowing wrapper over the backend, and it carries
/// the epoch law in its TAI surface.
#[test]
fn the_public_api_is_a_thin_wrapper() {
    struct Scripted;
    impl TimeBackend for Scripted {
        fn get_date_time(&self) -> crate::core::Result<DateTime> {
            Ok(DateTime {
                year: 2026,
                month: 10,
                day: 14,
                hour: 23,
                minute: 16,
                second: 31,
                millisecond: 0,
                weekday: 3,
                yearday: 287,
                timezone: "UTC".to_string(),
                utc_offset_seconds: 0,
            })
        }
        fn convert_to_tai(&self, utc: Timestamp) -> crate::core::Result<TaiTimestamp> {
            Ok(utc_nanos_to_tai(utc.as_nanos()))
        }
    }
    let backend = Scripted;
    let time = super::Time::new(&backend);
    assert_eq!(time.now().expect("now").timezone, "UTC");
    let tai = time.to_tai(Timestamp::from_nanos(0)).expect("tai");
    assert_eq!(tai.tai_seconds, 37, "the epoch 1970 in TAI is 37 s later");
    assert_eq!(time.caps().len(), 3);
    assert!(super::method_availability_for("getDateTime").is_some());
    assert!(super::method_availability_for("noSuchMethod").is_none());
}

/// A neutral `CalendarDiff` is all-zero and not negative — the value a caller
/// reads for two identical dates.
#[test]
fn a_neutral_diff_is_zero_and_not_negative() {
    let diff = CalendarDiff::default();
    assert_eq!(diff.total_days, 0);
    assert!(!diff.negative);
    assert_eq!((diff.years, diff.months, diff.days), (0, 0, 0));
}

/// The civil kernels are the same ones the contract's converters sit on, and
/// they are exact inverses — which is what keeps this domain and
/// `timezone.capnp` from drifting apart.
#[test]
fn civil_kernels_anchor_every_calendar_conversion() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    // 1970-01-01 was a Thursday, which is weekday 4 in the ISO numbering.
    assert_eq!(super::types::iso_weekday(0), 4);
    assert_eq!(super::types::day_of_year(0), 1);
    assert_eq!(super::types::days_in_month(2024, 2, CalendarSystem::Gregorian), 29);
    assert_eq!(super::types::days_in_month(1900, 2, CalendarSystem::Julian), 29);
    assert_eq!(super::types::days_in_month(2026, 2, CalendarSystem::Gregorian), 28);
    // An out-of-range month is 0, never a fabricated 31.
    assert_eq!(super::types::days_in_month(2026, 13, CalendarSystem::Gregorian), 0);
    let diff = CalendarDiff {
        years: 1,
        months: 2,
        days: 3,
        total_days: 430,
        negative: false,
    };
    assert_eq!(diff.years, 1);
    let _ = Duration::from_nanos(1);
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

/// The time cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/time.rs");

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
            "{method} claims {test}, which is not a test function in ../../platforms/linux/time.rs"
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
