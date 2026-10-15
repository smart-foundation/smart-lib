//! The time domain's rendering: the wall clock, the contract's TAI converter,
//! the arithmetic calendars — every value from the engine's owned types, every
//! width a display width under the terminal-faithful measure.

use crate::color::ColorExt;
use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::widgets::{Cell, Sgr, Table};
use crate::OutputFormat;

use smart_lib::core::values::Timestamp;
use smart_lib::domains::time::TimeBackend;
use smart_lib::domains::time::types::{
    CalendarSystem, DateTime, HistoricalDate, LEAP_SECONDS_TAI_MINUS_UTC,
};

use super::{TimeAction as A, TimeCmd};

/// One wall-clock reading as a record: every field the host measured.
fn now_value_inner(dt: &DateTime) -> Value {
    let wall = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second, dt.millisecond
    );
    let offset = format_offset(dt.utc_offset_seconds);
    let weekday = weekday_name(dt.weekday);
    Value::Object(vec![
        ("wall".into(), Value::String(wall)),
        ("year".into(), Value::Number(f64::from(dt.year))),
        ("month".into(), Value::Number(f64::from(dt.month))),
        ("day".into(), Value::Number(f64::from(dt.day))),
        ("hour".into(), Value::Number(f64::from(dt.hour))),
        ("minute".into(), Value::Number(f64::from(dt.minute))),
        ("second".into(), Value::Number(f64::from(dt.second))),
        ("millisecond".into(), Value::Number(f64::from(dt.millisecond))),
        ("weekday".into(), Value::String(weekday)),
        ("yearday".into(), Value::Number(f64::from(dt.yearday))),
        ("timezone".into(), Value::String(dt.timezone.clone())),
        ("utc_offset_seconds".into(), Value::Number(f64::from(dt.utc_offset_seconds))),
        ("utc_offset".into(), Value::String(offset)),
    ])
}

pub fn now_value(backend: &dyn TimeBackend) -> SmartResult<Value> {
    let dt = backend.get_date_time().map_err(SmartError::from)?;
    Ok(now_value_inner(&dt))
}

fn weekday_name(iso: u8) -> String {
    match iso {
        1 => "Monday".to_string(),
        2 => "Tuesday".to_string(),
        3 => "Wednesday".to_string(),
        4 => "Thursday".to_string(),
        5 => "Friday".to_string(),
        6 => "Saturday".to_string(),
        7 => "Sunday".to_string(),
        _ => "—".to_string(),
    }
}

fn format_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { "-" } else { "+" };
    let abs = seconds.unsigned_abs();
    format!("{sign}{:02}:{:02}", abs / 3_600, (abs % 3_600) / 60)
}

/// The contract's TAI converter over now, rendered in the shell: UTC nanos →
/// TaiTimestamp → back. The leap-second constant is the contract's own.
pub fn tai_value(backend: &dyn TimeBackend) -> SmartResult<Value> {
    let dt = backend.get_date_time().map_err(SmartError::from)?;
    let utc_nanos = super::utc_nanos_of(&dt);
    let utc = Timestamp::from_nanos(utc_nanos);
    let tai = backend.convert_to_tai(utc).map_err(SmartError::from)?;
    let back = backend.convert_from_tai(tai).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("utc_nanos".into(), Value::String(utc.as_nanos().to_string())),
        ("tai_seconds".into(), Value::String(tai.tai_seconds.to_string())),
        ("tai_nanoseconds".into(), Value::Number(f64::from(tai.nanoseconds))),
        ("round_trip_utc_nanos".into(), Value::String(back.as_nanos().to_string())),
        ("leap_seconds_tai_minus_utc".into(), Value::Number(LEAP_SECONDS_TAI_MINUS_UTC as f64)),
        (
            "note".into(),
            Value::String(format!(
                "the contract's converter via timezone.capnp's rule — TAI = UTC + {} s, seconds \
                 and nanoseconds since 1970 (never chrono's epoch math)",
                LEAP_SECONDS_TAI_MINUS_UTC
            )),
        ),
    ]))
}

/// The arithmetic calendars for a date, through the engine's own converter:
/// one row per calendar in the CONTRACT'S OWN ORDER, and the calendars this
/// build refuses (the Hebrew and Chinese astronomies) named with the engine's
/// own reason rather than approximated.
fn cal_value(backend: &dyn TimeBackend, date: Option<&str>, to: Option<&str>) -> SmartResult<Value> {
    let gregorian = match date {
        Some(text) => super::parse_date(text)?,
        None => {
            let dt = backend.get_date_time().map_err(SmartError::from)?;
            HistoricalDate::gregorian(dt.year as i64, dt.month, dt.day)
        }
    };
    // The calendars the contract names, in the contract's own order: the four
    // the arithmetic kernels serve exactly, and the two whose astronomies
    // (Hebrew, Chinese) this build refuses rather than approximating. The
    // `unknown` sentinel and the `custom` forward-compat outlet are not
    // calendars a caller asks to convert into.
    const CALENDARS: [CalendarSystem; 6] = [
        CalendarSystem::Gregorian,
        CalendarSystem::Julian,
        CalendarSystem::Islamic,
        CalendarSystem::Hebrew,
        CalendarSystem::Chinese,
        CalendarSystem::Ethiopian,
    ];
    let wanted: Vec<CalendarSystem> = match to {
        Some(token) => match CalendarSystem::from_token(token) {
            // An unrecognised calendar is refused, never guessed at: a
            // conversion between the wrong two calendars is silently wrong in a
            // way no consumer can detect.
            CalendarSystem::Unknown => {
                return Err(SmartError::invalid_argument(format!(
                    "a calendar is gregorian/julian/islamic/ethiopian (the calendars this host's \
                     own arithmetic serves), got {token:?}"
                )))
            }
            system => vec![system],
        },
        None => CALENDARS.to_vec(),
    };
    let mut rows = vec![Value::Object(vec![
        ("calendar".into(), Value::String("gregorian".into())),
        ("year".into(), Value::Number(gregorian.year as f64)),
        ("month".into(), Value::Number(f64::from(gregorian.month))),
        ("day".into(), Value::Number(f64::from(gregorian.day))),
    ])];
    for target in wanted {
        if matches!(target, CalendarSystem::Gregorian) {
            continue;
        }
        match backend.convert_calendar(&gregorian, CalendarSystem::Gregorian, target) {
            Ok(converted) => rows.push(Value::Object(vec![
                ("calendar".into(), Value::String(target.as_token().to_string())),
                ("year".into(), Value::Number(converted.year as f64)),
                ("month".into(), Value::Number(f64::from(converted.month))),
                ("day".into(), Value::Number(f64::from(converted.day))),
            ])),
            Err(e) => rows.push(Value::Object(vec![
                ("calendar".into(), Value::String(target.as_token().to_string())),
                ("error".into(), Value::String(e.to_string())),
            ])),
        }
    }
    Ok(Value::List(rows))
}

fn add_value(backend: &dyn TimeBackend, date: &str, unit: &str) -> SmartResult<Value> {
    let start = super::parse_date(date)?;
    let units = [super::parse_unit(unit)?];
    let result = backend
        .add_calendar_units(&start, &units)
        .map_err(SmartError::from)?;
    Ok(date_record(&start, &result, unit, "+"))
}

/// `subtractCalendarUnits` — the same arithmetic kernel, the other direction.
fn sub_value(backend: &dyn TimeBackend, date: &str, unit: &str) -> SmartResult<Value> {
    let start = super::parse_date(date)?;
    let units = [super::parse_unit(unit)?];
    let result = backend
        .subtract_calendar_units(&start, &units)
        .map_err(SmartError::from)?;
    Ok(date_record(&start, &result, unit, "−"))
}

fn diff_value(backend: &dyn TimeBackend, date1: &str, date2: &str) -> SmartResult<Value> {
    let a = super::parse_date(date1)?;
    let b = super::parse_date(date2)?;
    let diff = backend.diff_calendar_dates(&a, &b).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("years".into(), Value::Number(f64::from(diff.years))),
        ("months".into(), Value::Number(f64::from(diff.months))),
        ("days".into(), Value::Number(f64::from(diff.days))),
        ("total_days".into(), Value::Number(diff.total_days as f64)),
        ("negative".into(), Value::Bool(diff.negative)),
    ]))
}

fn date_record(start: &HistoricalDate, result: &HistoricalDate, unit: &str, op: &str) -> Value {
    Value::Object(vec![
        ("from".into(), Value::String(format!(
            "{:04}-{:02}-{:02}", start.year, start.month, start.day
        ))),
        ("op".into(), Value::String(format!("{op}{unit}"))),
        ("to".into(), Value::String(format!(
            "{:04}-{:02}-{:02}", result.year, result.month, result.day
        ))),
    ])
}

pub fn caps_value(backend: &dyn TimeBackend) -> Value {
    Value::List(backend.time_caps().into_iter().map(|c| Value::String(c.to_string())).collect())
}

// ── the command data half ─────────────────────────────────────────────────

pub fn evaluate_time(cmd: &TimeCmd, backend: &dyn TimeBackend) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(A::Now) => now_value(backend),
        Some(A::Tai) => tai_value(backend),
        Some(A::Cal { date, to }) => cal_value(backend, date.as_deref(), to.as_deref()),
        Some(A::Add { date, unit }) => add_value(backend, date, unit),
        Some(A::Sub { date, unit }) => sub_value(backend, date, unit),
        Some(A::Diff { date1, date2 }) => diff_value(backend, date1, date2),
        Some(A::Caps) => Ok(caps_value(backend)),
    }
}

// ── the aesthetic ─────────────────────────────────────────────────────────

pub fn render_action(cmd: &TimeCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &TimeCmd, value: &Value) -> String {
    match &cmd.action {
        None | Some(A::Now) => render_now(value),
        Some(A::Tai) => render_tai(value),
        Some(A::Cal { to, .. }) => render_cal(value, to.as_deref()),
        Some(A::Add { .. }) | Some(A::Sub { .. }) => render_add(value),
        Some(A::Diff { .. }) => render_diff(value),
        Some(A::Caps) => render_caps(value),
    }
}

fn render_now(value: &Value) -> String {
    if value.field("error").as_str().is_some() {
        return format!("  {} time — {}", "✗".red(), value.field_str("error"));
    }
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "🕐".blue().bold(), "Now".blue().bold()));
    out.push_str(&format!("  ├─ Wall       {}\n", value.field_str("wall").bold()));
    out.push_str(&format!(
        "  ├─ Weekday    {} (day {} of the year)\n",
        value.field_str("weekday"),
        value.field_str("yearday")
    ));
    let tz = value.field_str("timezone");
    let tz_display = if tz.is_empty() { "— the host publishes no zone".to_string() } else { tz };
    out.push_str(&format!(
        "  ├─ Zone       {} {}\n",
        tz_display,
        format!("(UTC{})", value.field_str("utc_offset")).dimmed()
    ));
    out.push_str(&format!(
        "  └─ Source     {}",
        "CLOCK_REALTIME, zone from the host's own TZif file — no `date` fork".dimmed()
    ));
    out
}

fn render_tai(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — the contract's own converter\n",
        "⏱".blue().bold(),
        "TAI".blue().bold()
    ));
    out.push_str(&format!("  ├─ UTC        {} nanos (since 1970)\n", value.field_str("utc_nanos")));
    out.push_str(&format!(
        "  ├─ TAI        {} s + {} ns\n",
        value.field_str("tai_seconds"),
        value.field_str("tai_nanoseconds")
    ));
    out.push_str(&format!(
        "  ├─ Offset     {} s (TAI − UTC, the contract's constant)\n",
        value.field_str("leap_seconds_tai_minus_utc")
    ));
    out.push_str(&format!(
        "  └─ Round-trip back to UTC {} {}",
        value.field_str("round_trip_utc_nanos"),
        if value.field_str("round_trip_utc_nanos") == value.field_str("utc_nanos") {
            "◉ exact".green().to_string()
        } else {
            "✗ drift".red().to_string()
        }
    ));
    out
}

fn render_cal(value: &Value, to: Option<&str>) -> String {
    let Value::List(rows) = value else {
        return String::new();
    };
    let mut out = String::new();
    let scope = match to {
        Some(token) => format!("→ {token}"),
        None => "every calendar in the contract's own order".to_string(),
    };
    out.push_str(&format!(
        "  {} {} {} — exact arithmetic, never an approximation\n",
        "📅".blue().bold(),
        "Calendars".blue().bold(),
        scope.dimmed()
    ));
    let mut table = Table::new(vec!["".into(), "CALENDAR".into(), "DATE".into()]);
    for row in rows {
        let cal = row.field_str("calendar");
        let date = if let Some(err) = row.field("error").as_str() {
            // The Hebrew and Chinese astronomies are refused by the engine, and
            // the refusal is the answer — never an approximation. The reason is
            // cut like every other cell, and the full one is in the JSON.
            format!("✗ {}", crate::widgets::truncate_to(err, 64))
        } else {
            format!("{:04}-{:02}-{:02}", fnum(&row, "year"), fnum(&row, "month"), fnum(&row, "day"))
        };
        let glyph = match cal.as_str() {
            "gregorian" => Cell::styled("▣", Sgr::Cyan),
            "julian" => Cell::styled("▤", Sgr::Blue),
            "islamic" => Cell::styled("◍", Sgr::Yellow),
            "ethiopian" => Cell::styled("◈", Sgr::Green),
            _ => Cell::plain("▢"),
        };
        table = table.row(vec![glyph, Cell::plain(cal), Cell::plain(date)]);
    }
    out.push_str(&table.render());
    out
}

fn render_add(value: &Value) -> String {
    format!(
        "  {} {} {} → {} {}",
        "📅".blue().bold(),
        value.field_str("from"),
        value.field_str("op").dimmed(),
        value.field_str("to").bold(),
        "(month-end clamped)".dimmed()
    )
}

fn render_diff(value: &Value) -> String {
    let arrow = if value.field_bool("negative") { "before".to_string() } else { "after".to_string() };
    format!(
        "  {} {} years, {} months, {} days — {} day(s) {}",
        "📏".blue().bold(),
        value.field_str("years"),
        value.field_str("months"),
        value.field_str("days"),
        value.field_str("total_days"),
        arrow.dimmed()
    )
}

fn render_caps(value: &Value) -> String {
    let Value::List(caps) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!("  {} {} — what this backend serves\n", "🔑".blue().bold(), "Time caps".blue().bold()));
    for cap in caps {
        out.push_str(&format!("  ├─ {} {}\n", "◉".green(), cap.display()));
    }
    out.trim_end().to_string()
}

fn fnum(record: &Value, key: &str) -> i64 {
    record.field(key).as_f64().unwrap_or(0.0) as i64
}
