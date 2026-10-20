//! The time domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `time.capnp`'s `interface Time` block (`@0`–`@5`). The dispatch
//! itself is the generated `ServerDispatch` — there is no ordinal, no method
//! table and no envelope of this layer's own anywhere on the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason,
//!   read as data, never as code;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached (law #5). The time domain has
//!   no mutating verb, so the gate is empty here — but the class travels
//!   with the service, so a later verb cannot slip past it.
//!
//! # What this domain does NOT serve
//!
//! The contract's TAI converters (`convertToTai` / `convertFromTai`) live in
//! `timezone.capnp`'s `TimezoneService`, not in `time.capnp`'s `Time`. This
//! domain's engine mirrors them (so a caller holding only a `Time` can
//! convert), but a mirror is not a contract method: the `Time` interface
//! declares six methods, and those six are all this file serves. The
//! registry therefore publishes no TAI command for this domain — a
//! registration that named a method the interface does not declare would be
//! a command a client could not address.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use crate::core::ErrorCode;
use crate::domains::time::availability;
use crate::domains::time::backend::TimeBackend;
use crate::domains::time::types::{CalendarDiff, CalendarSystem, CalendarUnit, HistoricalDate};
use crate::schema::time_capnp::time as tm;
use crate::schema::types_capnp;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

use super::availability::matrix_reader;

matrix_reader!(availability);

/// The time service: the domain's backend, and the class this connection
/// earned.
///
/// The backend is shared behind an `Arc`: the *same* host state answers
/// every connection, and a per-connection server is a view of it, not a
/// copy.
pub struct TimeService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for TimeService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> TimeService<B>
where
    B: TimeBackend + 'static,
{
    /// A service over this backend, at this class.
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
///
/// `None` when the matrix says the method IS served here — the arm then
/// goes to the backend. The answer comes BEFORE the class's gate: what the
/// host cannot do, no class can do.
fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a Time contract method"),
        )),
    }
}
/// The contract's `HistoricalDate` as the domain's date.
fn historical_date(reader: &crate::schema::time_capnp::historical_date::Reader<'_>) -> HistoricalDate {
    HistoricalDate::from_reader(reader)
}

/// The contract's `CalendarUnit` as the domain's unit.
fn calendar_unit(reader: &crate::schema::time_capnp::calendar_unit::Reader<'_>) -> CalendarUnit {
    use crate::domains::time::types::CalendarUnitKind;
    let kind = match reader.get_kind() {
        Ok(crate::schema::time_capnp::CalendarUnitKind::Days) => CalendarUnitKind::Days,
        Ok(crate::schema::time_capnp::CalendarUnitKind::Weeks) => CalendarUnitKind::Weeks,
        Ok(crate::schema::time_capnp::CalendarUnitKind::Months) => CalendarUnitKind::Months,
        Ok(crate::schema::time_capnp::CalendarUnitKind::Years) => CalendarUnitKind::Years,
        Ok(crate::schema::time_capnp::CalendarUnitKind::Custom) => CalendarUnitKind::Custom,
        // An unknown arm is the contract's own `unknown`, never a guessed
        // unit: an addition in the wrong unit is silently wrong.
        Ok(_) | Err(_) => CalendarUnitKind::Unknown,
    };
    CalendarUnit {
        kind,
        amount: reader.get_amount(),
    }
}

/// The contract's `CalendarSystem` as the domain's calendar.
fn calendar_system(reader: crate::schema::time_capnp::CalendarSystem) -> CalendarSystem {
    CalendarSystem::from_generated(reader)
}

/// Write the domain's `CalendarDiff` into the contract's struct.
fn write_diff(
    diff: &CalendarDiff,
    builder: &mut crate::schema::time_capnp::calendar_diff::Builder<'_>,
) {
    builder.set_years(diff.years);
    builder.set_months(diff.months);
    builder.set_days(diff.days);
    builder.set_total_days(diff.total_days);
    builder.set_negative(diff.negative);
}

impl<B> tm::Server for TimeService<B>
where
    B: TimeBackend + 'static,
{
    // ── The reads ────────────────────────────────────────────────────────

    fn get_date_time(
        self: Rc<Self>,
        _params: tm::GetDateTimeParams,
        mut results: tm::GetDateTimeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let reading = backend
                .get_date_time()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            reading.write_to(&mut builder.init_date_time());
            Ok(())
        }
    }

    fn verify_hsm_timestamp(
        self: Rc<Self>,
        params: tm::VerifyHsmTimestampParams,
        mut results: tm::VerifyHsmTimestampResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("verifyHsmTimestamp") {
                return Err(refusal);
            }
            let params = params.get()?;
            let hsm = params.get_hsm()?;
            // The HSM reference is the hardware's, never the attestation
            // itself: the engine reads the timestamp and the hardware id, and
            // the host's own material stays where it is.
            let reference = crate::domains::time::types::HsmTimestamp {
                timestamp_nanos: hsm.get_timestamp()?.get_nanoseconds(),
                hardware_id: crate::serve::text_of(hsm.get_hardware_id()?),
            };
            let verdict = backend
                .verify_hsm_timestamp(&reference)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut result = builder.init_result();
            match verdict {
                Some(ok) => {
                    result.set_ok(ok);
                }
                None => {
                    // "This host cannot verify" is an answer, not a `false`
                    // verdict: a guessed refusal would be indistinguishable
                    // from a real one.
                    let mut failure = result.init_error();
                    failure.set_code(types_capnp::ErrorCode::NotSupported);
                    failure.set_message(
                        "notSupported: not-applicable: this host cannot verify an HSM \
                         attestation — there is no attestation authority to verify against",
                    );
                }
            }
            Ok(())
        }
    }

    fn convert_calendar(
        self: Rc<Self>,
        params: tm::ConvertCalendarParams,
        mut results: tm::ConvertCalendarResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let date = historical_date(&params.get_date()?);
            let from = calendar_system(params.get_from_system()?);
            let to = calendar_system(params.get_to_system()?);
            let converted = backend
                .convert_calendar(&date, from, to)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            converted.write_to(&mut builder.init_converted());
            Ok(())
        }
    }

    fn add_calendar_units(
        self: Rc<Self>,
        params: tm::AddCalendarUnitsParams,
        mut results: tm::AddCalendarUnitsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let date = historical_date(&params.get_date()?);
            let units: Vec<CalendarUnit> = params
                .get_units()?
                .iter()
                .map(|unit| calendar_unit(&unit))
                .collect();
            let result = backend
                .add_calendar_units(&date, &units)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            result.write_to(&mut builder.init_result());
            Ok(())
        }
    }

    fn subtract_calendar_units(
        self: Rc<Self>,
        params: tm::SubtractCalendarUnitsParams,
        mut results: tm::SubtractCalendarUnitsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let date = historical_date(&params.get_date()?);
            let units: Vec<CalendarUnit> = params
                .get_units()?
                .iter()
                .map(|unit| calendar_unit(&unit))
                .collect();
            let result = backend
                .subtract_calendar_units(&date, &units)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            result.write_to(&mut builder.init_result());
            Ok(())
        }
    }

    fn diff_calendar_dates(
        self: Rc<Self>,
        params: tm::DiffCalendarDatesParams,
        mut results: tm::DiffCalendarDatesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let date1 = historical_date(&params.get_date1()?);
            let date2 = historical_date(&params.get_date2()?);
            let diff = backend
                .diff_calendar_dates(&date1, &date2)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_diff(&diff, &mut builder.init_diff());
            Ok(())
        }
    }
}

/// The time domain, as a served domain: the contract interface it serves,
/// the descriptor discovery publishes for it, and the per-connection server.
pub struct TimeDomain<B> {
    backend: Arc<B>,
}

impl<B> TimeDomain<B>
where
    B: TimeBackend + 'static,
{
    /// The time domain over this backend. The backend is shared: the same
    /// host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for TimeDomain<B>
where
    B: TimeBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        // The contract's own interface id, from the generated code.
        tm::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::TIME_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "time"
    }

    fn category(&self) -> &'static str {
        "time"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    /// The service's own state: the time domain cannot answer one method of
    /// its contract on this host (the HSM attestation), so the service is
    /// `degraded` and the missing method is named in the registry.
    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    /// The commands this domain publishes through the registry (cli.capnp).
    ///
    /// The paths are the PRODUCT's own words — what `smart time …` types —
    /// and each one names the contract method that answers it. A verb that
    /// needs several methods publishes the one that produces its value and
    /// lists the others under the stable `composes` key.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "time now",
                "Time",
                "getDateTime",
                "the wall-clock reading in the host's zone",
                0,
                true,
            )
            .with_row(matrix_row("getDateTime")),
            DomainCommand::composing(
                "time cal",
                "Time",
                "convertCalendar",
                "today (or a named date) in another calendar",
                1,
                true,
                "getDateTime",
            )
            .with_row(matrix_row("convertCalendar")),
            DomainCommand::new(
                "time add",
                "Time",
                "addCalendarUnits",
                "add calendar units to a date",
                2,
                false,
            )
            .with_row(matrix_row("addCalendarUnits")),
            DomainCommand::new(
                "time sub",
                "Time",
                "subtractCalendarUnits",
                "subtract calendar units from a date",
                3,
                false,
            )
            .with_row(matrix_row("subtractCalendarUnits")),
            DomainCommand::new(
                "time diff",
                "Time",
                "diffCalendarDates",
                "the difference between two dates",
                4,
                false,
            )
            .with_row(matrix_row("diffCalendarDates")),
            DomainCommand::new(
                "time verify-hsm",
                "Time",
                "verifyHsmTimestamp",
                "verify an HSM-attested timestamp",
                5,
                false,
            )
            .with_row(matrix_row("verifyHsmTimestamp")),
        ]
    }

    /// The time schema's node tree: what a client that never compiled
    /// `time.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(env!("OUT_DIR"), "/schemas/time.schema.bin")))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = TimeService::new(Arc::clone(&self.backend), class);
        let client: tm::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
