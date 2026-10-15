#![cfg(feature = "domain-time")]
//! The time command module over the scripted engine: the command surface's data
//! half, its human aesthetic, the contract's own TAI/Unix converters (never
//! chrono's epoch math), and the calendars the arithmetic kernels serve with
//! the refusals named. Nothing here touches the host (charter law #8): every
//! read is served from the script.

mod common;

use clap::Parser;
use common::MockTime;
use shell::domains::time::{evaluate, render, selector_root, TimeCmd};
use shell::selector;
use shell::{OutputFormat, Value};

/// A record field as the list of records it holds (a missing or non-list field
/// is the honest empty, never a panic).
fn list_of<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    match value.field(key) {
        Value::List(items) => items.iter().collect(),
        _ => Vec::new(),
    }
}

fn cmd(argv: &[&str]) -> TimeCmd {
    TimeCmd::try_parse_from(argv).unwrap()
}

/// The default view is the host's own wall clock: the kernel's CLOCK_REALTIME,
/// the host's own zone and offset, no `date` fork.
#[test]
fn the_default_view_is_the_kernels_own_wall_clock() {
    let backend = MockTime::default();
    let value = evaluate(&cmd(&["time"]), &backend).unwrap();
    assert_eq!(value.field_str("wall"), "2026-10-15 12:30:45.500");
    assert_eq!(value.field_str("weekday"), "Thursday");
    assert_eq!(value.field_str("yearday"), "288");
    assert_eq!(value.field_str("timezone"), "Africa/Johannesburg");
    assert_eq!(value.field_str("utc_offset"), "+02:00");
    let rendered = render::render_action(&cmd(&["time"]), &value, OutputFormat::Human);
    assert!(rendered.contains("CLOCK_REALTIME"), "{rendered}");
    assert!(rendered.contains("Africa/Johannesburg"), "{rendered}");
}

/// The TAI surface is the CONTRACT'S OWN converter: TAI = UTC + 37 s, in
/// seconds and nanoseconds since the Unix epoch, and the round trip is exact.
#[test]
fn the_tai_surface_is_the_contracts_own_converter() {
    let backend = MockTime::default();
    let value = evaluate(&cmd(&["time", "tai"]), &backend).unwrap();
    // The contract's own converter: TAI = UTC + 37 s, in seconds AND
    // nanoseconds since the Unix epoch — the mock's own arithmetic proves the
    // relationship rather than a hardcoded wall-clock reading.
    let utc_nanos: u64 = value.field_str("utc_nanos").parse().expect("utc nanos");
    let tai_seconds: u64 = value.field_str("tai_seconds").parse().expect("tai seconds");
    let tai_nanos: u32 = value.field_str("tai_nanoseconds").parse().expect("tai nanos");
    assert_eq!(tai_seconds * 1_000_000_000 + u64::from(tai_nanos), utc_nanos + 37_000_000_000);
    assert_eq!(value.field_str("leap_seconds_tai_minus_utc"), "37");
    assert_eq!(
        value.field_str("round_trip_utc_nanos"),
        value.field_str("utc_nanos"),
        "the contract's converter round-trips exactly"
    );
    assert!(value.field_str("note").contains("never chrono"));
    let rendered = render::render_action(&cmd(&["time", "tai"]), &value, OutputFormat::Human);
    assert!(rendered.contains("◉ exact"), "{rendered}");
}

/// The calendars: every calendar in the contract's own order, the arithmetic
/// ones served exactly and the astronomies refused — the refusal is the
/// answer, never an approximation.
#[test]
fn the_calendars_serve_the_arithmetic_ones_and_refuse_the_astronomies() {
    let backend = MockTime::default();
    let value = evaluate(&cmd(&["time", "cal"]), &backend).unwrap();
    let rows = match &value {
        Value::List(items) => items.clone(),
        other => panic!("expected a list, got {other:?}"),
    };
    assert_eq!(rows.len(), 6, "the contract's six named calendars");
    assert_eq!(rows[0].field_str("calendar"), "gregorian");
    assert_eq!(rows[1].field("year").as_f64(), Some(2026.0));
    assert_eq!(rows[1].field("month").as_f64(), Some(9.0));
    assert_eq!(rows[2].field_str("calendar"), "islamic");
    assert_eq!(rows[2].field("year").as_f64(), Some(1448.0));
    assert_eq!(rows[3].field_str("calendar"), "hebrew");
    assert!(rows[3].field("error").as_str().is_some(), "the Hebrew astronomy is refused");
    assert_eq!(rows[4].field_str("calendar"), "chinese");
    assert!(rows[4].field("error").as_str().is_some(), "the Chinese astronomy is refused");
    assert_eq!(rows[5].field_str("calendar"), "ethiopian");

    // One named calendar, and an unrecognised token refused rather than
    // guessed at — a conversion between the wrong two calendars is silently
    // wrong in a way no consumer can detect.
    let islamic = evaluate(&cmd(&["time", "cal", "--to", "islamic"]), &backend).unwrap();
    match &islamic {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[1].field_str("calendar"), "islamic");
        }
        other => panic!("expected a list, got {other:?}"),
    }
    let bad = evaluate(&cmd(&["time", "cal", "--to", "mayan"]), &backend);
    assert!(bad.is_err());
    assert!(bad.unwrap_err().to_string().contains("a calendar is"));

    // A named date is parsed through the engine's own HistoricalDate.
    let dated = evaluate(&cmd(&["time", "cal", "--date", "2026-03-31", "--to", "julian"]), &backend).unwrap();
    match &dated {
        Value::List(items) => assert_eq!(items[0].field("day").as_f64(), Some(31.0)),
        other => panic!("expected a list, got {other:?}"),
    }
    let bad_date = evaluate(&cmd(&["time", "cal", "--date", "today"]), &backend);
    assert!(bad_date.is_err());
}

/// The calendar arithmetic: add and subtract carry the month-end clamp, and the
/// diff is the contract's own CalendarDiff.
#[test]
fn the_calendar_arithmetic_carries_the_month_end_clamp() {
    let backend = MockTime::default();
    let added = evaluate(&cmd(&["time", "add", "2026-01-31", "1month"]), &backend).unwrap();
    assert_eq!(added.field_str("from"), "2026-01-31");
    assert_eq!(added.field_str("op"), "+1month");
    assert_eq!(added.field_str("to"), "2026-02-28");

    let subtracted = evaluate(&cmd(&["time", "sub", "2026-03-31", "1month"]), &backend).unwrap();
    assert_eq!(subtracted.field_str("op"), "−1month");
    assert_eq!(subtracted.field_str("to"), "2026-02-28");

    let diff = evaluate(&cmd(&["time", "diff", "2026-10-15", "2020-01-01"]), &backend).unwrap();
    assert_eq!(diff.field("total_days").as_f64(), Some(2479.0));
    assert!(!diff.field_bool("negative"));
    let reversed = evaluate(&cmd(&["time", "diff", "2020-01-01", "2026-10-15"]), &backend).unwrap();
    assert!(reversed.field_bool("negative"));

    // A unit token the contract does not name is refused, never guessed.
    let bad = evaluate(&cmd(&["time", "add", "2026-01-01", "3fortnights"]), &backend);
    assert!(bad.is_err());
}

/// The capability tokens are the backend's own published set (law #6).
#[test]
fn the_caps_are_the_backends_own_published_set() {
    let backend = MockTime::default();
    let value = evaluate(&cmd(&["time", "caps"]), &backend).unwrap();
    assert_eq!(value.display(), "[time.datetime, time.calendar, time.tai]");
}

/// The selector tree is the same values the command surface renders.
#[test]
fn the_selector_tree_resolves_from_the_engines_values() {
    let backend = MockTime::default();
    let root = selector_root(&backend).unwrap();
    assert_eq!(root.field("now").field_str("timezone"), "Africa/Johannesburg");
    assert_eq!(root.field("tai").field_str("leap_seconds_tai_minus_utc"), "37");
    assert_eq!(list_of(&root, "caps")[0].display(), "time.datetime");

    let now = selector::resolve(
        selector_root(&backend).unwrap(),
        &selector::parse("@time/now/wall").unwrap(),
    );
    assert_eq!(now.display(), "2026-10-15 12:30:45.500");
}

/// The JSON and raw forms carry the same values the human rendering draws.
#[test]
fn json_and_raw_carry_the_same_values() {
    let backend = MockTime::default();
    let value = evaluate(&cmd(&["time"]), &backend).unwrap();
    let json = render::render_action(&cmd(&["time"]), &value, OutputFormat::Json);
    assert!(json.contains("\"timezone\":\"Africa/Johannesburg\""), "{json}");
    let raw = render::render_action(&cmd(&["time"]), &value, OutputFormat::Raw);
    assert!(raw.contains("Africa/Johannesburg"), "{raw}");
}
