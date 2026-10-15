//! The **clock** domain: the host's time sources and the timer/alarm surface —
//! over one object-safe [`ClockBackend`] trait with the frozen `clock.capnp`
//! surface (`info @0` through `alarms @19`, plus the `TimerSubscription`
//! helpers and the [`AlarmService`] interface the contract carries).
//!
//! What this domain is: the kernel's own clocks, read natively, and the
//! contract's canonical timer and alarm home (its alarm model is strictly
//! richer than the duplicate `time.capnp` carried, which is now deleted).
//!
//! ## The mechanisms on this backend (law #9)
//!
//! | contract surface | native mechanism |
//! |---|---|
//! | `info` / `monotonicNow` / `realtimeNow` | `clock_gettime(CLOCK_REALTIME)` / `clock_gettime(CLOCK_MONOTONIC)` |
//! | `info.resolution` | `clock_getres` on the same clock ids |
//! | `info.bootTime` | `/proc/stat`'s `btime` (the kernel's own boot epoch) |
//! | `createTimer` / `checkTimer` / `subscribeTimers` | `timerfd_create` + `timerfd_settime`; a firing is the kernel's own expiration count |
//! | `sleep` / `sleepCancellable` | `nanosleep` on the caller's own thread; the cancellable half waits on a condvar it can be signalled out of |
//! | `getClockSync` | a measured `/proc` scan for a synchronization daemon, never a `chronyc` shellout |
//! | alarms | the process's own alarm store, behind the capability the contract's `alarms @19` hands out |
//!
//! What is honestly DEGRADED, and why: the synchronization state (the daemon is
//! measured to exist; its stratum/offset live behind a root-only directory), the
//! drift measurement (bounded window, stated as such), the quality metrics (the
//! observable half only) and timer persistence (no host store exists, so a
//! persisted timer does not survive process exit — a physical limit, named).
//! Nothing in this contract mutates the host, so no method is gated on
//! privilege; the one write-adjacent verb is `setDefaultTimezone`, which is
//! `timezone.capnp`'s and not this domain's.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Clock;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, CLOCK_AVAILABILITY,
    CLOCK_SCHEMA_ID, MethodAvailability, ReasonClass,
};
pub use backend::{AlarmService, ClockBackend};
pub use types::{
    Alarm, AlarmRecurrence, ClockDomainInfo, ClockDomainKind, ClockDriftInfo, ClockInfo,
    ClockQualityMetrics, ClockSecurityInfo, ClockSyncInfo, HolidayBehavior, HolidayMode, Page,
    Pagination, SleepHandle, SubscriptionId, TimerAnchor, TimerEvent, TimerFilter, TimerHandle,
    TimerPersistence, TimerSpec, VirtualClockConfig, VirtualClockHandle,
};
