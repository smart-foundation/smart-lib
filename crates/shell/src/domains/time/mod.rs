//! `time` — the domain command module on the NEW engine.
//!
//! It renders from the engine's OWNED types (`DateTime`, `TaiTimestamp`,
//! `HistoricalDate`, `CalendarDiff`) and the contract's OWN calendar kernels
//! (`days_from_civil`/`civil_from_days`, the TAI converter) — never chrono's
//! epoch math, never a `date` shellout. The wall clock is the kernel's own
//! `CLOCK_REALTIME`; the zone and offset are the host's own TZif answer. What
//! this host cannot attest (an HSM, a Hebrew/Chinese ephemeris) is answered
//! with the measured reason, never a guessed conversion.

use clap::{Parser, Subcommand};

use crate::error::SmartResult;
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::time::TimeBackend;
use smart_lib::domains::time::types::{
    days_from_civil, CalendarUnit, CalendarUnitKind, HistoricalDate,
};

/// The engine seam this shell speaks to — the per-domain backend trait object.
#[cfg(feature = "domain-time")]
pub type Backend = dyn TimeBackend;
#[cfg(not(feature = "domain-time"))]
pub type Backend = ();

/// `smart time` — what time it is, in which calendar, and the contract's own
/// TAI/Unix converters over the kernel's clock.
#[derive(Parser, Debug)]
pub struct TimeCmd {
    #[command(subcommand)]
    pub action: Option<TimeAction>,
}

#[derive(Subcommand, Debug)]
pub enum TimeAction {
    /// The wall-clock reading in the host's zone (the default view).
    Now,
    /// The contract's TAI converter: now as a TaiTimestamp, and back to UTC.
    Tai,
    /// Today (or a named date) in the arithmetic calendars, via the converters
    /// — one row per calendar in the contract's own order, with the calendars
    /// this build refuses named rather than approximated.
    Cal {
        /// A Gregorian date `YYYY-M-D` (defaults to today).
        #[arg(long)]
        date: Option<String>,
        /// One calendar to convert into (`julian`, `islamic`, `ethiopian`, …).
        #[arg(long)]
        to: Option<String>,
    },
    /// Add calendar units to a date (`addCalendarUnits`).
    Add {
        /// The Gregorian date `YYYY-M-D`.
        date: String,
        /// A unit+amount, e.g. `1month`, `2weeks`, `3days`.
        unit: String,
    },
    /// Subtract calendar units from a date (`subtractCalendarUnits`).
    Sub {
        /// The Gregorian date `YYYY-M-D`.
        date: String,
        /// A unit+amount, e.g. `1month`, `2weeks`, `3days`.
        unit: String,
    },
    /// The difference between two Gregorian dates (`diffCalendarDates`).
    Diff {
        /// The later date `YYYY-M-D`.
        date1: String,
        /// The earlier date `YYYY-M-D`.
        date2: String,
    },
    /// This backend's honest time capability tokens.
    Caps,
}

/// Parse a `YYYY-M-D` Gregorian date into a `HistoricalDate`.
fn parse_date(text: &str) -> SmartResult<HistoricalDate> {
    let parts: Vec<&str> = text.split('-').collect();
    if parts.len() != 3 {
        return Err(crate::error::SmartError::invalid_argument(format!(
            "a date is YYYY-M-D, got {text:?}"
        )));
    }
    let year = parts[0].parse::<i64>().map_err(|_| date_err(text))?;
    let month = parts[1].parse::<u8>().map_err(|_| date_err(text))?;
    let day = parts[2].parse::<u8>().map_err(|_| date_err(text))?;
    Ok(HistoricalDate::gregorian(year, month, day))
}

fn date_err(text: &str) -> crate::error::SmartError {
    crate::error::SmartError::invalid_argument(format!("a date is YYYY-M-D, got {text:?}"))
}

/// Parse a `2weeks`/`1month`/`3days` unit token into a `CalendarUnit`.
fn parse_unit(text: &str) -> SmartResult<CalendarUnit> {
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = &text[digits.len()..];
    let amount: u32 = if digits.is_empty() {
        1
    } else {
        digits.parse().map_err(|_| {
            crate::error::SmartError::invalid_argument(format!("cannot read an amount from {text:?}"))
        })?
    };
    let kind = match rest.trim() {
        "d" | "day" | "days" => CalendarUnitKind::Days,
        "w" | "week" | "weeks" => CalendarUnitKind::Weeks,
        "m" | "month" | "months" => CalendarUnitKind::Months,
        "y" | "year" | "years" => CalendarUnitKind::Years,
        other => {
            return Err(crate::error::SmartError::invalid_argument(format!(
                "a unit is days/weeks/months/years, got {other:?}"
            )))
        }
    };
    Ok(CalendarUnit { kind, amount: amount as i32 })
}

/// The UTC epoch nanoseconds of a `DateTime`, reconstructed with the ENGINE'S
/// OWN civil kernel (never chrono): local civil instant, minus the host offset,
/// as nanoseconds since 1970.
fn utc_nanos_of(dt: &smart_lib::domains::time::types::DateTime) -> u64 {
    let days = days_from_civil(dt.year as i64, dt.month as u32, dt.day as u32);
    let local_secs = days * 86_400
        + i64::from(dt.hour) * 3_600
        + i64::from(dt.minute) * 60
        + i64::from(dt.second);
    let utc_secs = local_secs - i64::from(dt.utc_offset_seconds);
    (utc_secs.max(0) as u64) * 1_000_000_000 + u64::from(dt.millisecond) * 1_000_000
}

/// Evaluate the command's data half; the value a pipe consumes.
pub fn evaluate(cmd: &TimeCmd, backend: &Backend) -> SmartResult<Value> {
    #[cfg(feature = "domain-time")]
    {
        render::evaluate_time(cmd, backend)
    }
    #[cfg(not(feature = "domain-time"))]
    {
        let _ = (cmd, backend);
        Err(crate::error::SmartError::unavailable(
            "the shell was built without the domain-time feature — rebuild with default features",
        ))
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(cmd: &TimeCmd, format: &OutputFormat, backend: &Backend) -> SmartResult<Value> {
    let value = evaluate(cmd, backend)?;
    #[cfg(feature = "domain-time")]
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

#[cfg(feature = "domain-time")]
pub mod render;

/// The time selector tree, from the engine's owned types — the same values the
/// command surface renders, so a selector and a command can never disagree.
/// `@time/now`, `@time/tai`, `@time/caps` resolve from here.
#[cfg(feature = "domain-time")]
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    Ok(Value::Object(vec![
        ("now".into(), render::now_value(backend)?),
        ("tai".into(), render::tai_value(backend)?),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

#[cfg(not(feature = "domain-time"))]
pub fn selector_root(_backend: &Backend) -> SmartResult<Value> {
    Ok(Value::Object(vec![
        ("error".into(), Value::String("the shell was built without the domain-time feature".into())),
    ]))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{TimeAction as Action, TimeCmd as Cmd};
