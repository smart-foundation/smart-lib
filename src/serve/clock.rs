//! The clock domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `clock.capnp`'s `interface ClockService` block (`@0`–`@19`), its
//! `TimerSubscription` interface (`@0`–`@2`) and its `AlarmService`
//! interface (`@0`–`@7`). The dispatch is the generated `ServerDispatch`;
//! there is no ordinal, no method table and no envelope of this layer's own
//! anywhere on the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached (law #5).
//!
//! # The two capabilities the contract hands back
//!
//! `subscribeTimers @7` hands back a `TimerSubscription` reference and
//! `alarms @19` hands back an `AlarmService` reference. In both cases the
//! reference IS the authority — there is no token to present, and no
//! per-method permission name anywhere in this file.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use crate::core::values::Duration;
use crate::core::ErrorCode;
use crate::domains::clock::availability;
use crate::domains::clock::backend::{AlarmService, ClockBackend};
use crate::domains::clock::types::{
    ClockDomainKind, ClockDriftInfo, ClockQualityMetrics, Page, Pagination, SleepHandle,
    SubscriptionId, TimerAnchor, TimerFilter, TimerHandle, TimerPersistence, TimerSpec,
    VirtualClockConfig, VirtualClockHandle,
};
use crate::schema::clock_capnp::alarm_service as alarms;
use crate::schema::clock_capnp::clock_service as clk;
use crate::schema::clock_capnp::timer_subscription as tsub;
use crate::schema::types_capnp;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

use super::availability::matrix_reader;

matrix_reader!(availability);

/// The clock service: the domain's backend, and the class this connection
/// earned.
pub struct ClockService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for ClockService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> ClockService<B>
where
    B: ClockBackend + 'static,
{
    /// A service over this backend, at this class.
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }

    /// The class's answer for a mutating verb on a connection that did not
    /// earn the right to change this host's clock state (law #5). The verb is
    /// refused *before* the backend is reached: a change that is refused
    /// afterwards has already happened.
    fn gate(&self, verb: &str) -> Option<capnp::Error> {
        if self.class.is_control() {
            return None;
        }
        Some(errors::decline(
            ErrorCode::PermissionDenied,
            &format!(
                "permissionDenied: the {verb} verb changes this host's timers and alarms, and \
                 this connection holds the read-only class"
            ),
        ))
    }
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
////// Write the engine's `VoidResult` into the contract's `T.VoidResult`.
fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

/// The contract's pagination as the domain's.
fn pagination(reader: &types_capnp::pagination::Reader<'_>) -> Pagination {
    Pagination {
        offset: reader.get_offset(),
        limit: reader.get_limit(),
    }
}

/// The contract's duration as the domain's.
fn duration(reader: &types_capnp::duration::Reader<'_>) -> Duration {
    Duration::from_nanos(reader.get_nanoseconds())
}

/// Read a `Text` field that may be absent, into an owned string.
fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => super::text_of(text),
        Err(_) => String::new(),
    }
}

/// The contract's `TimerSpec` as the domain's spec.
fn timer_spec(reader: &crate::schema::clock_capnp::timer_spec::Reader<'_>) -> TimerSpec {
    let mut spec = TimerSpec::from_reader(&reader);
    // The contract's `TimerAnchor` union carries the anchor's MEANING; the
    // engine's spec carries it in its own `anchor_type`.
    let anchor = reader.get_anchor_type();
    if let Ok(anchor) = anchor {
        use crate::schema::clock_capnp::timer_anchor::Which;
        match anchor.which() {
            Ok(Which::Absolute(stamp)) => {
                spec.anchor_type = TimerAnchor::Absolute(crate::core::values::Timestamp::from_nanos(
                    stamp.map(|t| t.get_nanoseconds()).unwrap_or(0),
                ));
            }
            Ok(Which::Relative(delta)) => {
                spec.anchor_type = TimerAnchor::Relative(Duration::from_nanos(
                    delta.map(|d| d.get_nanoseconds()).unwrap_or(0),
                ));
            }
            Ok(Which::Zero(())) => {
                spec.anchor_type = TimerAnchor::Zero;
            }
            Ok(Which::Custom(_)) => {
                spec.anchor_type = TimerAnchor::Custom;
            }
            Err(_) => {}
        }
    }
    spec
}

/// The contract's `VirtualClockConfig` as the domain's config.
fn virtual_clock_config(
    reader: &crate::schema::clock_capnp::virtual_clock_config::Reader<'_>,
) -> VirtualClockConfig {
    VirtualClockConfig {
        initial_time: crate::core::values::Timestamp::from_nanos(
            reader
                .get_initial_time()
                .map(|t| t.get_nanoseconds())
                .unwrap_or(0),
        ),
        domain: ClockDomainKind::from_generated(reader.get_domain().unwrap_or(
            crate::schema::clock_capnp::ClockDomain::Realtime,
        )),
        rate: reader.get_rate(),
        name: text_field(reader.get_name()),
    }
}

/// The contract's `TimerPersistence` as the domain's persistence.
fn timer_persistence(
    reader: &crate::schema::clock_capnp::timer_persistence::Reader<'_>,
) -> TimerPersistence {
    TimerPersistence {
        name: text_field(reader.get_name()),
        re_fire_on_restart: reader.get_re_fire_on_restart(),
        max_re_fires: reader.get_max_re_fires(),
    }
}

/// Write the domain's drift measurement into the contract's struct.
fn write_drift(
    drift: &ClockDriftInfo,
    builder: &mut crate::schema::clock_capnp::clock_drift_info::Builder<'_>,
) {
    builder.reborrow().init_drift().set_nanoseconds(drift.drift.as_nanos());
    builder
        .reborrow()
        .init_measured_at()
        .set_nanoseconds(drift.measured_at.as_nanos());
    builder
        .reborrow()
        .init_accuracy()
        .set_nanoseconds(drift.accuracy.as_nanos());
}

/// Write the domain's quality metrics into the contract's struct.
fn write_quality(
    quality: &ClockQualityMetrics,
    builder: &mut crate::schema::clock_capnp::clock_quality_metrics::Builder<'_>,
) {
    builder.set_source_name(&quality.source_name);
    builder
        .reborrow()
        .init_accuracy()
        .set_nanoseconds(quality.accuracy.as_nanos());
    builder
        .reborrow()
        .init_stability()
        .set_nanoseconds(quality.stability.as_nanos());
    builder.set_reliability(quality.reliability);
    builder
        .reborrow()
        .init_measured_at()
        .set_nanoseconds(quality.measured_at.as_nanos());
    builder.set_sample_count(quality.sample_count);
}

/// Write the domain's virtual clock handle into the contract's struct.
fn write_virtual_clock(
    handle: &VirtualClockHandle,
    builder: &mut crate::schema::clock_capnp::virtual_clock_handle::Builder<'_>,
) {
    builder.set_id(handle.id);
    builder.set_name(&handle.name);
}

/// The timer subscription the service hands back for `subscribeTimers @7`.
///
/// Holding the reference IS the authority to pull — the contract's own rule.
pub struct TimerSubscriptionService<B> {
    backend: Arc<B>,
    subscription: SubscriptionId,
}

impl<B> Clone for TimerSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> tsub::Server for TimerSubscriptionService<B>
where
    B: ClockBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        _params: tsub::PullParams,
        mut results: tsub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let events = backend
                .pull_timers(subscription)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_events(events.len() as u32);
            for (index, event) in events.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                event.write_to(&mut item);
            }
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: tsub::CancelParams,
        mut results: tsub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_timers(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn pull_paginated(
        self: Rc<Self>,
        params: tsub::PullPaginatedParams,
        mut results: tsub::PullPaginatedResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let params = params.get()?;
            let page = pagination(&params.get_pagination()?);
            let events = backend
                .pull_timers(subscription)
                .map_err(|error| errors::to_capnp(&error))?;
            // The engine's pull is a DRAIN: it hands over the events that are
            // pending and takes them. So the page window is applied to what
            // the drain produced, and the total is that drain's count — the
            // number the caller may page through, not a count of a list that
            // is gone.
            let total = events.len();
            let offset = page.offset as usize;
            let limit = page.effective_limit() as usize;
            let window: Vec<_> = events
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect();
            let builder = results.get();
            let mut result = builder.init_result();
            let mut list = result.reborrow().init_events(window.len() as u32);
            for (index, event) in window.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                event.write_to(&mut item);
            }
            result.set_total_count(total as u32);
            Ok(())
        }
    }
}

/// The alarm service the service hands back for `alarms @19`.
///
/// The reference IS the authority to schedule an alarm. The engine's alarm
/// service is acquired per call from the shared backend — the store behind
/// it is the same store, so the alarm a caller creates and the alarm a later
/// call reads are the same alarm.
pub struct AlarmServiceServer<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for AlarmServiceServer<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> AlarmServiceServer<B>
where
    B: ClockBackend + 'static,
{
    /// The engine's alarm service, or the engine's own honest error.
    fn alarms(&self) -> Result<Box<dyn AlarmService>, capnp::Error> {
        self.backend
            .alarms()
            .map_err(|error| errors::to_capnp(&error))
    }
}

impl<B> alarms::Server for AlarmServiceServer<B>
where
    B: ClockBackend + 'static,
{
    fn create_alarm(
        self: Rc<Self>,
        params: alarms::CreateAlarmParams,
        mut results: alarms::CreateAlarmResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let alarm = read_alarm(&params.get_alarm()?);
            let service = self.alarms()?;
            let id = service
                .create_alarm(&alarm)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_id().set_id(id.get());
            Ok(())
        }
    }

    fn update_alarm(
        self: Rc<Self>,
        params: alarms::UpdateAlarmParams,
        mut results: alarms::UpdateAlarmResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let alarm = read_alarm(&params.get_alarm()?);
            let service = self.alarms()?;
            let outcome = service.update_alarm(&alarm);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn delete_alarm(
        self: Rc<Self>,
        params: alarms::DeleteAlarmParams,
        mut results: alarms::DeleteAlarmResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let id = crate::core::ids::AlarmId::new(params.get_id()?.get_id());
            let service = self.alarms()?;
            let outcome = service.delete_alarm(id);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn get_alarm(
        self: Rc<Self>,
        params: alarms::GetAlarmParams,
        mut results: alarms::GetAlarmResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let id = crate::core::ids::AlarmId::new(params.get_id()?.get_id());
            let service = self.alarms()?;
            let alarm = service
                .get_alarm(id)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            alarm.write_to(&mut builder.init_alarm());
            Ok(())
        }
    }

    fn list_alarms(
        self: Rc<Self>,
        params: alarms::ListAlarmsParams,
        mut results: alarms::ListAlarmsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let page = pagination(&params.get_pagination()?);
            let service = self.alarms()?;
            let alarms: Page<crate::domains::clock::types::Alarm> = service
                .list_alarms(page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_alarms(alarms.items.len() as u32);
            for (index, alarm) in alarms.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                alarm.write_to(&mut item);
            }
            builder.set_total_count(alarms.total);
            Ok(())
        }
    }

    fn snooze_alarm(
        self: Rc<Self>,
        params: alarms::SnoozeAlarmParams,
        mut results: alarms::SnoozeAlarmResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let id = crate::core::ids::AlarmId::new(params.get_id()?.get_id());
            let defer = duration(&params.get_duration()?);
            let service = self.alarms()?;
            let outcome = service.snooze_alarm(id, defer);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn dismiss_alarm(
        self: Rc<Self>,
        params: alarms::DismissAlarmParams,
        mut results: alarms::DismissAlarmResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let id = crate::core::ids::AlarmId::new(params.get_id()?.get_id());
            let service = self.alarms()?;
            let outcome = service.dismiss_alarm(id);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn get_next_alarm(
        self: Rc<Self>,
        _params: alarms::GetNextAlarmParams,
        mut results: alarms::GetNextAlarmResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let service = self.alarms()?;
            let alarm = service
                .get_next_alarm()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            alarm.write_to(&mut builder.init_alarm());
            Ok(())
        }
    }
}

/// Read the contract's `Alarm` into the domain's alarm.
fn read_alarm(reader: &crate::schema::clock_capnp::alarm::Reader<'_>) -> crate::domains::clock::types::Alarm {
    use crate::domains::clock::types::{Alarm, AlarmRecurrence, HolidayBehavior, HolidayMode};
    let mut alarm = Alarm {
        id: crate::core::ids::AlarmId::new(reader.get_id().map(|i| i.get_id()).unwrap_or(0)),
        label: text_field(reader.get_label()),
        fire_time: crate::core::values::Timestamp::from_nanos(
            reader
                .get_fire_time()
                .map(|t| t.get_nanoseconds())
                .unwrap_or(0),
        ),
        recurrence: None,
        snooze_duration: Duration::from_nanos(
            reader
                .get_snooze_duration()
                .map(|d| d.get_nanoseconds())
                .unwrap_or(0),
        ),
        enabled: reader.get_enabled(),
        holiday_behavior: HolidayBehavior::default(),
    };
    if let Ok(recurrence) = reader.get_recurrence() {
        use crate::schema::clock_capnp::alarm_recurrence::Which;
        alarm.recurrence = match recurrence.which() {
            Ok(Which::Daily(())) => Some(AlarmRecurrence::Daily),
            Ok(Which::Weekly(weekly)) => {
                let mut days = Vec::new();
                let weekly = match weekly {
                    Ok(weekly) => weekly,
                    Err(_) => return alarm,
                };
                if let Ok(list) = weekly.get_days_of_week() {
                    for day in list.iter() {
                        days.push(match day {
                            Ok(crate::schema::calendar_capnp::DayOfWeek::Sunday) => 0,
                            Ok(crate::schema::calendar_capnp::DayOfWeek::Monday) => 1,
                            Ok(crate::schema::calendar_capnp::DayOfWeek::Tuesday) => 2,
                            Ok(crate::schema::calendar_capnp::DayOfWeek::Wednesday) => 3,
                            Ok(crate::schema::calendar_capnp::DayOfWeek::Thursday) => 4,
                            Ok(crate::schema::calendar_capnp::DayOfWeek::Friday) => 5,
                            Ok(crate::schema::calendar_capnp::DayOfWeek::Saturday) => 6,
                            // An unnamed day is the contract's own `custom`
                            // member — never a guessed weekday.
                            _ => 7,
                        });
                    }
                }
                Some(AlarmRecurrence::Weekly(days))
            }
            Ok(Which::Monthly(monthly)) => Some(AlarmRecurrence::Monthly(
                monthly.map(|m| m.get_day_of_month()).unwrap_or(0),
            )),
            Ok(Which::Cron(expr)) => Some(AlarmRecurrence::Cron(text_field(expr))),
            Ok(Which::Custom(_)) => Some(AlarmRecurrence::Custom),
            Err(_) => None,
        };
    }
    if let Ok(behavior) = reader.get_holiday_behavior() {
        let mode = match behavior.get_mode() {
            Ok(crate::schema::clock_capnp::HolidayMode::Skip) => HolidayMode::Skip,
            Ok(crate::schema::clock_capnp::HolidayMode::Adjust) => HolidayMode::Adjust,
            Ok(crate::schema::clock_capnp::HolidayMode::Snooze) => HolidayMode::Snooze,
            Ok(crate::schema::clock_capnp::HolidayMode::Custom) => HolidayMode::Custom,
            _ => HolidayMode::Fire,
        };
        let adjusted = crate::core::values::Timestamp::from_nanos(
            behavior
                .get_adjusted_time()
                .map(|t| t.get_nanoseconds())
                .unwrap_or(0),
        );
        let mut calendars = Vec::new();
        if let Ok(list) = behavior.get_holiday_calendars() {
            for calendar in list.iter() {
                calendars.push(text_field(calendar));
            }
        }
        alarm.holiday_behavior = HolidayBehavior {
            mode,
            adjusted_time: adjusted,
            holiday_calendars: calendars,
        };
    }
    alarm
}

impl<B> clk::Server for ClockService<B>
where
    B: ClockBackend + 'static,
{
    // ── The reads ────────────────────────────────────────────────────────

    fn info(
        self: Rc<Self>,
        _params: clk::InfoParams,
        mut results: clk::InfoResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let info = backend
                .info()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            info.write_to(&mut builder.init_info());
            Ok(())
        }
    }

    fn monotonic_now(
        self: Rc<Self>,
        _params: clk::MonotonicNowParams,
        mut results: clk::MonotonicNowResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let stamp = backend
                .monotonic_now()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_time().set_nanoseconds(stamp.as_nanos());
            Ok(())
        }
    }

    fn realtime_now(
        self: Rc<Self>,
        _params: clk::RealtimeNowParams,
        mut results: clk::RealtimeNowResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let stamp = backend
                .realtime_now()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_time().set_nanoseconds(stamp.as_nanos());
            Ok(())
        }
    }

    fn list_clock_domains(
        self: Rc<Self>,
        _params: clk::ListClockDomainsParams,
        mut results: clk::ListClockDomainsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let domains = backend
                .list_clock_domains()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_domains(domains.len() as u32);
            for (index, domain) in domains.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                domain.write_to(&mut item);
            }
            Ok(())
        }
    }

    fn get_clock_sync(
        self: Rc<Self>,
        _params: clk::GetClockSyncParams,
        mut results: clk::GetClockSyncResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let sync = backend
                .get_clock_sync()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            sync.write_to(&mut builder.init_sync());
            Ok(())
        }
    }

    fn get_clock_drift(
        self: Rc<Self>,
        params: clk::GetClockDriftParams,
        mut results: clk::GetClockDriftResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let first = ClockDomainKind::from_generated(params.get_domain1()?);
            let second = ClockDomainKind::from_generated(params.get_domain2()?);
            let drift = backend
                .get_clock_drift(first, second)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_drift(&drift, &mut builder.init_drift());
            Ok(())
        }
    }

    fn get_clock_quality(
        self: Rc<Self>,
        params: clk::GetClockQualityParams,
        mut results: clk::GetClockQualityResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let source = super::text_of(params.get_source_name()?);
            let quality = backend
                .get_clock_quality(&source)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_quality(&quality, &mut builder.init_quality());
            Ok(())
        }
    }

    fn list_persisted_timers(
        self: Rc<Self>,
        params: clk::ListPersistedTimersParams,
        mut results: clk::ListPersistedTimersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = pagination(&params.get_pagination()?);
            let timers: Page<TimerHandle> = backend
                .list_persisted_timers(page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_timers(timers.items.len() as u32);
            for (index, timer) in timers.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                timer.write_to(&mut item);
            }
            builder.set_total_count(timers.total);
            Ok(())
        }
    }

    // ── The timers ───────────────────────────────────────────────────────

    fn create_timer(
        self: Rc<Self>,
        params: clk::CreateTimerParams,
        mut results: clk::CreateTimerResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("createTimer") {
                return Err(refusal);
            }
            let params = params.get()?;
            let spec = timer_spec(&params.get_spec()?);
            let timer = backend
                .create_timer(&spec)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            timer.write_to(&mut builder.init_timer());
            Ok(())
        }
    }

    fn cancel_timer(
        self: Rc<Self>,
        params: clk::CancelTimerParams,
        mut results: clk::CancelTimerResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("cancelTimer") {
                return Err(refusal);
            }
            let params = params.get()?;
            let timer = TimerHandle::from_reader(&params.get_timer()?);
            let outcome = backend.cancel_timer(&timer);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn reset_timer(
        self: Rc<Self>,
        params: clk::ResetTimerParams,
        mut results: clk::ResetTimerResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("resetTimer") {
                return Err(refusal);
            }
            let params = params.get()?;
            let timer = TimerHandle::from_reader(&params.get_timer()?);
            let spec = timer_spec(&params.get_spec()?);
            let handle = backend
                .reset_timer(&timer, &spec)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            handle.write_to(&mut builder.init_timer());
            Ok(())
        }
    }

    fn check_timer(
        self: Rc<Self>,
        params: clk::CheckTimerParams,
        mut results: clk::CheckTimerResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let timer = TimerHandle::from_reader(&params.get_timer()?);
            let fired = backend
                .check_timer(&timer)
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_fired(fired);
            Ok(())
        }
    }

    fn subscribe_timers(
        self: Rc<Self>,
        params: clk::SubscribeTimersParams,
        mut results: clk::SubscribeTimersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let filter = TimerFilter {
                labels: {
                    let mut labels = Vec::new();
                    for label in params.get_filter()?.get_labels()?.iter() {
                        labels.push(super::text_of(label?));
                    }
                    labels
                },
            };
            let subscription = backend
                .subscribe_timers(&filter)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = TimerSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: tsub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn sleep(
        self: Rc<Self>,
        params: clk::SleepParams,
        mut results: clk::SleepResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let wait = duration(&params.get_duration()?);
            let outcome = backend.sleep(wait);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn sleep_cancellable(
        self: Rc<Self>,
        params: clk::SleepCancellableParams,
        mut results: clk::SleepCancellableResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("sleepCancellable") {
                return Err(refusal);
            }
            let params = params.get()?;
            let wait = duration(&params.get_duration()?);
            let handle: SleepHandle = backend
                .sleep_cancellable(wait)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_handle().set_id(handle.id);
            Ok(())
        }
    }

    fn persist_timer(
        self: Rc<Self>,
        params: clk::PersistTimerParams,
        mut results: clk::PersistTimerResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("persistTimer") {
                return Err(refusal);
            }
            let params = params.get()?;
            let timer = TimerHandle::from_reader(&params.get_timer()?);
            let persistence = timer_persistence(&params.get_persistence()?);
            let outcome = backend.persist_timer(&timer, &persistence);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_timer_dependency(
        self: Rc<Self>,
        params: clk::SetTimerDependencyParams,
        mut results: clk::SetTimerDependencyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setTimerDependency") {
                return Err(refusal);
            }
            let params = params.get()?;
            let timer = TimerHandle::from_reader(&params.get_timer()?);
            let depends_on = TimerHandle::from_reader(&params.get_depends_on()?);
            let outcome = backend.set_timer_dependency(&timer, &depends_on);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    // ── The virtual clocks ───────────────────────────────────────────────

    fn create_virtual_clock(
        self: Rc<Self>,
        params: clk::CreateVirtualClockParams,
        mut results: clk::CreateVirtualClockResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("createVirtualClock") {
                return Err(refusal);
            }
            let params = params.get()?;
            let config = virtual_clock_config(&params.get_config()?);
            let handle = backend
                .create_virtual_clock(&config)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_virtual_clock(&handle, &mut builder.init_clock());
            Ok(())
        }
    }

    fn advance_virtual_clock(
        self: Rc<Self>,
        params: clk::AdvanceVirtualClockParams,
        mut results: clk::AdvanceVirtualClockResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("advanceVirtualClock") {
                return Err(refusal);
            }
            let params = params.get()?;
            let handle = VirtualClockHandle {
                id: params.get_clock()?.get_id(),
                name: String::new(),
            };
            let step = duration(&params.get_duration()?);
            let stamp = backend
                .advance_virtual_clock(&handle, step)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_time().set_nanoseconds(stamp.as_nanos());
            Ok(())
        }
    }

    // ── The alarms ───────────────────────────────────────────────────────

    fn alarms(
        self: Rc<Self>,
        _params: clk::AlarmsParams,
        mut results: clk::AlarmsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let server = AlarmServiceServer {
                backend: Arc::clone(&backend),
                class: self.class,
            };
            let client: alarms::Client = capnp_rpc::new_client(server);
            results.get().set_service(client);
            Ok(())
        }
    }
}

/// The clock domain, as a served domain: the contract interface it serves,
/// the descriptor discovery publishes for it, and the per-connection server.
pub struct ClockDomain<B> {
    backend: Arc<B>,
}

impl<B> ClockDomain<B>
where
    B: ClockBackend + 'static,
{
    /// The clock domain over this backend. The backend is shared: the same
    /// host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for ClockDomain<B>
where
    B: ClockBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        clk::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::CLOCK_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "clock"
    }

    fn category(&self) -> &'static str {
        "clock"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    /// The service's own state: the clock domain answers its whole contract
    /// on this host, so it is `online`.
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
    /// The paths are the PRODUCT's own words — what `smart clock …` types —
    /// and each one names the contract method that answers it. A verb that
    /// needs several methods publishes the one that produces its value and
    /// lists the others under the stable `composes` key.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "clock info",
                "ClockService",
                "info",
                "the measured state of this host's clocks",
                0,
                true,
            )
            .with_row(matrix_row("info")),
            DomainCommand::new(
                "clock now",
                "ClockService",
                "realtimeNow",
                "the wall-clock reading",
                1,
                true,
            )
            .with_row(matrix_row("realtimeNow")),
            DomainCommand::new(
                "clock monotonic",
                "ClockService",
                "monotonicNow",
                "the monotonic reading, immune to NTP and DST",
                2,
                true,
            )
            .with_row(matrix_row("monotonicNow")),
            DomainCommand::new(
                "clock uptime",
                "ClockService",
                "info",
                "time since the last boot",
                3,
                true,
            )
            .with_row(matrix_row("info")),
            DomainCommand::new(
                "clock resolution",
                "ClockService",
                "info",
                "the measured clock resolution",
                4,
                true,
            )
            .with_row(matrix_row("info")),
            DomainCommand::new(
                "clock sync",
                "ClockService",
                "getClockSync",
                "the measured NTP/PTP synchronization state",
                5,
                true,
            )
            .with_row(matrix_row("getClockSync")),
            DomainCommand::new(
                "clock security",
                "ClockService",
                "info",
                "the measured clock-security state",
                6,
                true,
            )
            .with_row(matrix_row("info")),
            DomainCommand::new(
                "clock domains",
                "ClockService",
                "listClockDomains",
                "the clock domains this host can serve",
                7,
                true,
            )
            .with_row(matrix_row("listClockDomains")),
            DomainCommand::new(
                "clock quality",
                "ClockService",
                "getClockQuality",
                "quality metrics for one clock source",
                8,
                false,
            )
            .with_row(matrix_row("getClockQuality")),
            DomainCommand::new(
                "clock drift",
                "ClockService",
                "getClockDrift",
                "the measured drift between two clock domains",
                9,
                false,
            )
            .with_row(matrix_row("getClockDrift")),
            DomainCommand::new(
                "clock sleep",
                "ClockService",
                "sleep",
                "sleep for a duration on the caller's own thread",
                10,
                false,
            )
            .with_row(matrix_row("sleep")),
            DomainCommand::new(
                "clock sleep-cancellable",
                "ClockService",
                "sleepCancellable",
                "a cancellable sleep, and its handle",
                11,
                false,
            )
            .with_row(matrix_row("sleepCancellable")),
            DomainCommand::new(
                "clock timer create",
                "ClockService",
                "createTimer",
                "arm a timer",
                12,
                false,
            )
            .with_row(matrix_row("createTimer")),
            DomainCommand::new(
                "clock timer check",
                "ClockService",
                "checkTimer",
                "did this timer fire since the last pull?",
                13,
                false,
            )
            .with_row(matrix_row("checkTimer")),
            DomainCommand::new(
                "clock timer reset",
                "ClockService",
                "resetTimer",
                "re-arm a timer with a new spec",
                14,
                false,
            )
            .with_row(matrix_row("resetTimer")),
            DomainCommand::new(
                "clock timer cancel",
                "ClockService",
                "cancelTimer",
                "disarm and retire a timer",
                15,
                false,
            )
            .with_row(matrix_row("cancelTimer")),
            DomainCommand::new(
                "clock timers",
                "ClockService",
                "listPersistedTimers",
                "the timers this host persisted — they survive process exit",
                16,
                true,
            )
            .with_row(matrix_row("listPersistedTimers")),
            DomainCommand::new(
                "clock persist-timer",
                "ClockService",
                "persistTimer",
                "persist a timer so it survives process exit",
                17,
                false,
            )
            .with_row(matrix_row("persistTimer")),
            DomainCommand::new(
                "clock set-timer-dependency",
                "ClockService",
                "setTimerDependency",
                "gate one timer on another",
                18,
                false,
            )
            .with_row(matrix_row("setTimerDependency")),
            DomainCommand::composing(
                "clock watch",
                "ClockService",
                "subscribeTimers",
                "the timer event stream, pulled in bounded rounds",
                19,
                true,
                "pull,cancel,pullPaginated",
            )
            .with_row(matrix_row("subscribeTimers")),
            DomainCommand::new(
                "clock timer pull",
                "TimerSubscription",
                "pull",
                "take the timer events that have fired",
                20,
                true,
            )
            .with_row(matrix_row("pullTimers")),
            DomainCommand::new(
                "clock subscription cancel",
                "TimerSubscription",
                "cancel",
                "retire a timer subscription",
                21,
                false,
            )
            .with_row(matrix_row("cancelTimers")),
            DomainCommand::new(
                "clock timer pull-paginated",
                "TimerSubscription",
                "pullPaginated",
                "take one page of the timer events that have fired",
                22,
                false,
            )
            .with_row(matrix_row("pullTimers")),
            DomainCommand::new(
                "clock virtual create",
                "ClockService",
                "createVirtualClock",
                "create a virtual (software) clock for testing",
                23,
                false,
            )
            .with_row(matrix_row("createVirtualClock")),
            DomainCommand::new(
                "clock virtual advance",
                "ClockService",
                "advanceVirtualClock",
                "advance a virtual clock and read its new time",
                24,
                false,
            )
            .with_row(matrix_row("advanceVirtualClock")),
            DomainCommand::new(
                "clock alarms",
                "ClockService",
                "alarms",
                "acquire the alarm service",
                25,
                true,
            )
            .with_row(matrix_row("alarms")),
            DomainCommand::new(
                "clock alarm list",
                "AlarmService",
                "listAlarms",
                "the alarms this process's store holds",
                26,
                true,
            )
            .with_row(matrix_row("listAlarms")),
            DomainCommand::new(
                "clock alarm next",
                "AlarmService",
                "getNextAlarm",
                "the next alarm that will fire",
                27,
                true,
            )
            .with_row(matrix_row("getNextAlarm")),
            DomainCommand::new(
                "clock alarm get",
                "AlarmService",
                "getAlarm",
                "read one alarm",
                28,
                false,
            )
            .with_row(matrix_row("getAlarm")),
            DomainCommand::new(
                "clock alarm create",
                "AlarmService",
                "createAlarm",
                "create an alarm",
                29,
                false,
            )
            .with_row(matrix_row("createAlarm")),
            DomainCommand::new(
                "clock alarm update",
                "AlarmService",
                "updateAlarm",
                "update an alarm",
                30,
                false,
            )
            .with_row(matrix_row("updateAlarm")),
            DomainCommand::new(
                "clock alarm snooze",
                "AlarmService",
                "snoozeAlarm",
                "snooze an alarm",
                31,
                false,
            )
            .with_row(matrix_row("snoozeAlarm")),
            DomainCommand::new(
                "clock alarm dismiss",
                "AlarmService",
                "dismissAlarm",
                "dismiss an alarm",
                32,
                false,
            )
            .with_row(matrix_row("dismissAlarm")),
            DomainCommand::new(
                "clock alarm delete",
                "AlarmService",
                "deleteAlarm",
                "delete an alarm",
                33,
                false,
            )
            .with_row(matrix_row("deleteAlarm")),
        ]
    }

    /// The clock schema's node tree: what a client that never compiled
    /// `clock.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(env!("OUT_DIR"), "/schemas/clock.schema.bin")))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = ClockService::new(Arc::clone(&self.backend), class);
        let client: clk::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
