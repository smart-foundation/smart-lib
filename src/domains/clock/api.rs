//! The public clock API: a thin, borrowing wrapper over [`ClockBackend`]
//! exposing the handful of calls a client actually touches. Borrowed and
//! zero-cost — it adds no state over `&B`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::values::{Duration, Timestamp};
use crate::core::Result;

use super::backend::ClockBackend;
use super::types::{Alarm, Page, Pagination, TimerEvent, TimerHandle, TimerSpec, SubscriptionId};

/// The combined clock reading the CLI's `clock` renders: both kernel clocks
/// sampled together with the measured resolution between them.
#[derive(Clone, Debug, PartialEq)]
pub struct ClockSummary {
    /// The wall-clock reading (`CLOCK_REALTIME`).
    pub realtime: Timestamp,
    /// The monotonic reading (`CLOCK_MONOTONIC`).
    pub monotonic: Timestamp,
    /// The measured resolution of the realtime clock.
    pub resolution: Duration,
    /// The backend's open token for the clock source.
    pub source_name: String,
}

/// A borrowing handle to any [`ClockBackend`].
pub struct Clock<'a, B: ClockBackend + ?Sized>(&'a B);

impl<'a, B: ClockBackend + ?Sized> Clock<'a, B> {
    /// Wrap a backend (which the caller has already acquired authority to hold
    /// — law #5: the handle is the authority).
    pub fn new(backend: &'a B) -> Self {
        Clock(backend)
    }

    /// Both kernel clocks and the measured resolution, one read.
    #[must_use = "the reading is the answer; ignoring it is never useful"]
    pub fn summary(&self) -> Result<ClockSummary> {
        let info = self.0.info()?;
        Ok(ClockSummary {
            realtime: info.realtime,
            monotonic: info.monotonic,
            resolution: info.resolution,
            source_name: info.source_name,
        })
    }

    /// The wall-clock reading.
    #[must_use]
    pub fn realtime_now(&self) -> Result<Timestamp> {
        self.0.realtime_now()
    }

    /// The monotonic reading.
    #[must_use]
    pub fn monotonic_now(&self) -> Result<Timestamp> {
        self.0.monotonic_now()
    }

    /// Arm a timer.
    pub fn create_timer(&self, spec: &TimerSpec) -> Result<TimerHandle> {
        self.0.create_timer(spec)
    }

    /// Ask a timer whether it fired; the answer is the kernel's own count.
    #[must_use]
    pub fn check_timer(&self, timer: &TimerHandle) -> Result<bool> {
        self.0.check_timer(timer)
    }

    /// Disarm a timer.
    pub fn cancel_timer(&self, timer: &TimerHandle) -> Result<()> {
        self.0.cancel_timer(timer)
    }

    /// Sleep on the caller's own thread.
    pub fn sleep(&self, duration: Duration) -> Result<()> {
        self.0.sleep(duration)
    }

    /// Subscribe to firings, pull once under the caller's bound, and return
    /// both the token and the first batch.
    #[must_use]
    pub fn timer_stream(
        &self,
        labels: &[&str],
    ) -> Result<(SubscriptionId, Vec<TimerEvent>)> {
        let filter = super::types::TimerFilter {
            labels: labels.iter().map(|l| (*l).to_string()).collect(),
        };
        let subscription = self.0.subscribe_timers(&filter)?;
        let first = self.0.pull_timers(subscription)?;
        Ok((subscription, first))
    }

    /// Retire a timer subscription.
    pub fn close_timer_stream(&self, subscription: SubscriptionId) -> Result<()> {
        self.0.cancel_timers(subscription)
    }

    /// The first page of the alarm store.
    #[must_use]
    pub fn alarms(&self, page: Pagination) -> Result<Page<Alarm>> {
        self.0.alarms()?.list_alarms(page)
    }

    /// The backend's published capability tokens (law #6).
    #[must_use]
    pub fn caps(&self) -> alloc::vec::Vec<&'static str> {
        self.0.clock_caps()
    }
}
