//! The `ClockBackend` trait: one object-safe method per `ClockService` contract
//! method (`@0`–`@19`), each returning [`crate::core::Result`].
//!
//! ## Capability story (law #5)
//!
//! There is **no permission string on any method**. Authority is the *held
//! service reference*: a caller either holds a `ClockService` capability and may
//! call the whole surface, or it does not and every call is refused before a
//! backend is reached. The one capability-shaped accessor, `alarms @19`, is the
//! contract's *acquisition* point: a caller presents a capability and receives
//! an [`AlarmService`]; holding the returned reference IS the authority to
//! schedule an alarm.
//!
//! ## Object safety, and the honest default surface
//!
//! Every method borrows `&self` and returns owned values, so the trait is
//! object-safe and dispatchable as `dyn ClockBackend` at the transport seam.
//! Each method has a default body that answers `notSupported` **with a
//! reasonClass-prefixed reason** — the law #6 baseline. A platform backend
//! overrides exactly what it serves (the Linux backend overrides all of them,
//! because the kernel's own clocks answer this contract), and a host that
//! cannot serve one keeps the honest refusal rather than a zeroed value.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::core::ids::AlarmId;
use crate::core::values::{Duration, Timestamp};
use crate::core::{Error, Result};

use super::types::{
    Alarm, ClockDomainKind, ClockDriftInfo, ClockInfo, ClockQualityMetrics, ClockSyncInfo,
    ClockSecurityInfo, ClockDomainInfo, Page, Pagination, SleepHandle, SubscriptionId, TimerEvent,
    TimerFilter, TimerHandle, TimerPersistence, TimerSpec, VirtualClockConfig, VirtualClockHandle,
};

/// A protocol-compliant clock backend: one method per contract method, with an
/// honest `notSupported`-with-reason default for everything a host cannot
/// serve.
pub trait ClockBackend {
    /// The measured state of the host's clocks (`info @0`).
    fn info(&self) -> Result<ClockInfo> {
        Err(Error::not_supported(
            "no-backend: clock info is not served on this backend",
        ))
    }

    /// The monotonic reading (`monotonicNow @1`) — the kernel's own
    /// `CLOCK_MONOTONIC`, immune to NTP and DST adjustments.
    fn monotonic_now(&self) -> Result<Timestamp> {
        Err(Error::not_supported(
            "no-backend: the monotonic clock is not served on this backend",
        ))
    }

    /// The wall-clock reading (`realtimeNow @2`) — the kernel's own
    /// `CLOCK_REALTIME`.
    fn realtime_now(&self) -> Result<Timestamp> {
        Err(Error::not_supported(
            "no-backend: the realtime clock is not served on this backend",
        ))
    }

    /// Arm a timer (`createTimer @3`) and return its handle. The Linux backend
    /// arms a real `timerfd`; the anchor semantics of [`TimerSpec`] decide
    /// whether the first expiry is absolute or relative.
    fn create_timer(&self, _spec: &TimerSpec) -> Result<TimerHandle> {
        Err(Error::not_supported(
            "no-backend: timers are not served on this backend",
        ))
    }

    /// Disarm and retire a timer (`cancelTimer @4`).
    fn cancel_timer(&self, _timer: &TimerHandle) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: timers are not served on this backend",
        ))
    }

    /// Re-arm a timer with a new spec (`resetTimer @5`), returning the (possibly
    /// new) handle the contract promises.
    fn reset_timer(&self, _timer: &TimerHandle, _spec: &TimerSpec) -> Result<TimerHandle> {
        Err(Error::not_supported(
            "no-backend: timers are not served on this backend",
        ))
    }

    /// Did this timer fire since the last pull? (`checkTimer @6`.) The sanctioned
    /// poll path: the answer is the kernel's own expiration count, so a
    /// periodic timer that fired three times reports three, never one.
    fn check_timer(&self, _timer: &TimerHandle) -> Result<bool> {
        Err(Error::not_supported(
            "no-backend: timers are not served on this backend",
        ))
    }

    /// Subscribe to timer firings (`subscribeTimers @7`). Holding the returned
    /// id IS the authority to pull.
    fn subscribe_timers(&self, _filter: &TimerFilter) -> Result<SubscriptionId> {
        Err(Error::not_supported(
            "no-backend: timer subscriptions are not served on this backend",
        ))
    }

    /// Pull the next firings for a subscription (`TimerSubscription.pull @0`).
    fn pull_timers(&self, _subscription: SubscriptionId) -> Result<Vec<TimerEvent>> {
        Err(Error::not_supported(
            "no-backend: timer subscriptions are not served on this backend",
        ))
    }

    /// Retire a timer subscription (`TimerSubscription.cancel @1`).
    fn cancel_timers(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: timer subscriptions are not served on this backend",
        ))
    }

    /// Sleep for a duration (`sleep @8`) on the caller's own thread.
    fn sleep(&self, _duration: Duration) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: sleep is not served on this backend",
        ))
    }

    /// The clock domains this host can serve (`listClockDomains @9`).
    fn list_clock_domains(&self) -> Result<Vec<ClockDomainInfo>> {
        Err(Error::not_supported(
            "no-backend: clock domains are not served on this backend",
        ))
    }

    /// A cancellable sleep (`sleepCancellable @10`).
    fn sleep_cancellable(&self, _duration: Duration) -> Result<SleepHandle> {
        Err(Error::not_supported(
            "no-backend: a cancellable sleep is not served on this backend",
        ))
    }

    /// Cancel a sleep started with [`ClockBackend::sleep_cancellable`].
    fn cancel_sleep(&self, _handle: SleepHandle) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: a cancellable sleep is not served on this backend",
        ))
    }

    /// The measured NTP/PTP synchronization state (`getClockSync @11`).
    fn get_clock_sync(&self) -> Result<ClockSyncInfo> {
        Err(Error::not_supported(
            "no-backend: clock synchronization state is not served on this backend",
        ))
    }

    /// The measured drift between two clock domains (`getClockDrift @12`).
    fn get_clock_drift(
        &self,
        _domain1: ClockDomainKind,
        _domain2: ClockDomainKind,
    ) -> Result<ClockDriftInfo> {
        Err(Error::not_supported(
            "no-backend: clock drift measurement is not served on this backend",
        ))
    }

    /// Create a virtual (software) clock for testing
    /// (`createVirtualClock @13`). A virtual clock IS software, so this is a
    /// REAL surface rather than a degraded host read.
    fn create_virtual_clock(&self, _config: &VirtualClockConfig) -> Result<VirtualClockHandle> {
        Err(Error::not_supported(
            "no-backend: virtual clocks are not served on this backend",
        ))
    }

    /// Advance a virtual clock (`advanceVirtualClock @14`) and return its new
    /// time.
    fn advance_virtual_clock(
        &self,
        _clock: &VirtualClockHandle,
        _duration: Duration,
    ) -> Result<Timestamp> {
        Err(Error::not_supported(
            "no-backend: virtual clocks are not served on this backend",
        ))
    }

    /// Quality metrics for one clock source (`getClockQuality @15`).
    fn get_clock_quality(&self, _source_name: &str) -> Result<ClockQualityMetrics> {
        Err(Error::not_supported(
            "no-backend: clock quality metrics are not served on this backend",
        ))
    }

    /// Persist a timer so it survives process exit (`persistTimer @16`). A host
    /// that has no timer-persistence store answers honestly.
    fn persist_timer(&self, _timer: &TimerHandle, _persistence: &TimerPersistence) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: timer persistence is not served on this backend",
        ))
    }

    /// The timers that survived a process exit (`listPersistedTimers @17`).
    fn list_persisted_timers(&self, _page: Pagination) -> Result<Page<TimerHandle>> {
        Err(Error::not_supported(
            "no-backend: timer persistence is not served on this backend",
        ))
    }

    /// Set a dependency between two timers (`setTimerDependency @18`).
    fn set_timer_dependency(&self, _timer: &TimerHandle, _depends_on: &TimerHandle) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: timer dependencies are not served on this backend",
        ))
    }

    /// Acquire the alarm service (`alarms @19`). The returned reference IS the
    /// authority; there is no per-method token inside [`AlarmService`].
    fn alarms(&self) -> Result<Box<dyn AlarmService>> {
        Err(Error::not_supported(
            "no-backend: the alarm service is not served on this backend",
        ))
    }

    /// Publish this backend's honest clock capability tokens (law #6).
    fn clock_caps(&self) -> Vec<&'static str> {
        alloc::vec!["clock.realtime", "clock.monotonic", "clock.timers", "clock.alarms"]
    }

    // ── Diagnostics (not contract methods) ─────────────────────────────────

    /// A read-shaped measurement of this host's synchronization state. A backend
    /// that cannot observe any synchronization daemon answers `synced: false`
    /// with a reason, never a guessed verdict — the same law the network
    /// domain's privilege probe follows (charter law #8: a read, never a write).
    fn measure_clock_sync(&self) -> Result<ClockSyncInfo> {
        Err(Error::unavailable(
            "no-backend: no synchronization daemon is observable on this backend",
        ))
    }

    /// The measured words behind [`ClockBackend::measure_clock_sync`]: which
    /// daemon, which pid, and what remains unreadable. Empty when nothing was
    /// measurable.
    fn sync_evidence(&self) -> String {
        String::new()
    }

    /// The measured clock-security state (tamper/secure-source/hardware-RNG).
    fn security_state(&self) -> ClockSecurityInfo {
        ClockSecurityInfo::default()
    }
}

/// The alarm surface (`AlarmService`), acquired through
/// [`ClockBackend::alarms`]. Holding this reference IS the authority to
/// schedule, snooze and dismiss an alarm — there is no per-method token.
pub trait AlarmService {
    /// Create an alarm (`createAlarm @0`) and return its id.
    fn create_alarm(&self, _alarm: &Alarm) -> Result<AlarmId> {
        Err(Error::not_supported(
            "no-backend: alarms are not served on this backend",
        ))
    }

    /// Update an alarm (`updateAlarm @1`).
    fn update_alarm(&self, _alarm: &Alarm) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: alarms are not served on this backend",
        ))
    }

    /// Delete an alarm (`deleteAlarm @2`).
    fn delete_alarm(&self, _id: AlarmId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: alarms are not served on this backend",
        ))
    }

    /// Read one alarm (`getAlarm @3`); a missing alarm is `notFound`, never a
    /// zeroed struct.
    fn get_alarm(&self, _id: AlarmId) -> Result<Alarm> {
        Err(Error::not_supported(
            "no-backend: alarms are not served on this backend",
        ))
    }

    /// The paginated alarm list (`listAlarms @4`).
    fn list_alarms(&self, _page: Pagination) -> Result<Page<Alarm>> {
        Err(Error::not_supported(
            "no-backend: alarms are not served on this backend",
        ))
    }

    /// Snooze an alarm (`snoozeAlarm @5`).
    fn snooze_alarm(&self, _id: AlarmId, _duration: Duration) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: alarms are not served on this backend",
        ))
    }

    /// Dismiss an alarm (`dismissAlarm @6`).
    fn dismiss_alarm(&self, _id: AlarmId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: alarms are not served on this backend",
        ))
    }

    /// The next alarm that will fire (`getNextAlarm @7`).
    fn get_next_alarm(&self) -> Result<Alarm> {
        Err(Error::not_supported(
            "no-backend: alarms are not served on this backend",
        ))
    }
}
