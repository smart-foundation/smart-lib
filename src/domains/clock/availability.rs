//! The clock domain availability matrix: one row per `ClockService` contract
//! method (`@0`–`@19`), the [`super::backend::AlarmService`] methods the
//! contract carries in a second interface, and the subscription helpers —
//! published as data exactly the way `knowledge/03-CONFORMANCE.md` §3.2
//! prescribes. A `REAL`/`DEGRADED` claim names the `l3Test` that backs it; an
//! `UNAVAILABLE` row carries a nameable `reasonClass` and a reason that the
//! live behaviour repeats at runtime.
//!
//! This table is the contract with the availability generator and `cli.capnp`'s
//! `listUnavailable`: every non-`REAL` row here is answered at runtime by the
//! method returning `notSupported` whose message begins with the row's
//! `reasonClass` (see [`super::backend`]).
//!
//! ## The transport behind each row (charter law #9)
//!
//! This domain's transports are the **kernel's own clocks** — `CLOCK_REALTIME`
//! and `CLOCK_MONOTONIC` through `clock_gettime`, `clock_getres` for the
//! resolution, `timerfd_create`/`timerfd_settime` for the timer surface so a
//! firing is the kernel's own expiration count rather than a polled guess, and
//! `/proc/stat`'s `btime` for the boot epoch. A `DEGRADED` row is one where the
//! kernel answers *part* of the question: the synchronization state is proven
//! to exist (a `chronyd` process is measured on this host) while its exact
//! stratum and offset live in a root-only directory, and a persisted timer has
//! no host store at all.

/// The compiled schema file ID, read from `clock.capnp`'s `@0x…` line. The
/// matrix can never describe a contract the build did not compile.
pub const CLOCK_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60040;

/// One of the four availability reasons (`cli.capnp` `AvailabilityReason`,
/// re-used rather than a second grammar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    /// No implementation exists for this (domain, platform).
    NoBackend,
    /// The host primitive exists but the caller's credentials do not reach it.
    NoPrivilege,
    /// The concept does not exist on this platform.
    NotApplicable,
    /// Exists, held back deliberately.
    DisabledByPolicy,
    /// Reserved and flagged: a distinguishable gap. Never used in this table.
    Unknown,
}

impl ReasonClass {
    /// The exact token the matrix, the tool reason and the CLI agree on.
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonClass::NoBackend => "no-backend",
            ReasonClass::NoPrivilege => "no-privilege",
            ReasonClass::NotApplicable => "not-applicable",
            ReasonClass::DisabledByPolicy => "disabled-by-policy",
            ReasonClass::Unknown => "unknown",
        }
    }
}

/// Per-method status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus {
    /// Passes its full behaviour on this platform; an `l3Test` proves it.
    Real,
    /// Answers, but not completely; `reason` names the degraded capability and
    /// an `l3Test` still backs it.
    Degraded,
    /// Answers `notSupported` with a `reasonClass`-prefixed reason.
    Unavailable,
}

/// One availability row.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    /// The contract method name (`clock.capnp`).
    pub name: &'static str,
    /// The contract ordinal (the value that never moves). Rows for the
    /// `AlarmService` interface carry `0x100 + <its own ordinal>`, so the two
    /// interfaces of this one schema are distinguishable in one flat matrix.
    pub ordinal: u16,
    pub status: AvailStatus,
    /// The reason class, exactly when `status == UNAVAILABLE`.
    pub reason_class: Option<ReasonClass>,
    /// For `UNAVAILABLE`: why. For `DEGRADED`: the named degraded capability.
    /// Empty for `REAL`.
    pub reason: &'static str,
    /// The L3 test that ran and passed, required when `status != UNAVAILABLE`.
    pub l3_test: Option<&'static str>,
}

impl MethodAvailability {
    const fn real(name: &'static str, ordinal: u16, l3_test: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Real,
            reason_class: None,
            reason: "",
            l3_test: Some(l3_test),
        }
    }

    const fn degraded(name: &'static str, ordinal: u16, l3_test: &'static str, detail: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Degraded,
            reason_class: None,
            reason: detail,
            l3_test: Some(l3_test),
        }
    }

    // The UNAVAILABLE row constructor. No clock/logging row on this host needs
    // it — every method is REAL or DEGRADED-with-a-measured-reason, which is the
    // founder's standard — but a future host row is data, not a schema change,
    // so the constructor stays. The suite asserts the flagged `Unknown` class is
    // unused today.
    #[allow(dead_code)]
    const fn unavailable(
        name: &'static str,
        ordinal: u16,
        class: ReasonClass,
        reason: &'static str,
    ) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Unavailable,
            reason_class: Some(class),
            reason,
            l3_test: None,
        }
    }
}

/// The published availability matrix for the clock domain on Linux.
///
/// One row per `ClockService` contract method (`@0`–`@19`), the
/// `TimerSubscription` helpers, and the eight `AlarmService` methods
/// (`0x100`–`0x107`). The `l3_test` ids name the always-on hermetic tests in
/// [`crate::platforms::linux::clock`] that exercise this backend's mapping
/// against the host's real clocks; the `#[ignore]`d live probe is the separate
/// founder-runnable check.
///
/// ## Why each DEGRADED row is DEGRADED and not something else
///
/// * `getClockSync @11` — MEASURED, not assumed: `chronyd` runs on this host
///   (pid read from `/proc`), so the host *has* a synchronization daemon; the
///   daemon's own state (`stratum`, offset, last sync) is served by a root-only
///   `/run/chrony`, so `synced` is left false and the reason names the exact
///   denied path. A host with no daemon at all answers `synced: false` with the
///   measured "no synchronization daemon observable" reason instead.
/// * `getClockDrift @12` — a real measurement over a bounded window: the two
///   domains are read twice and their divergence divided out, with the accuracy
///   bounded by half the window. It is not a long-run drift estimate and says so.
/// * `getClockQuality @15` — the observable half of a clock source's quality
///   (the daemon behind it, and the resolution the kernel reports); the
///   statistical half (jitter, reliability over N samples) is not exported by
///   this host.
/// * `persistTimer @16` / `listPersistedTimers @17` — the store is this
///   process's memory, so a timer does NOT survive the exit the contract names.
///   That is a physical limit on a host with no timer-persistence daemon, and
///   it is stated rather than papered over.
pub const CLOCK_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("info", 0, "l3_linux_clock::info_reports_the_kernels_own_clocks"),
    MethodAvailability::real("monotonicNow", 1, "l3_linux_clock::monotonic_and_realtime_are_both_kernel_reads"),
    MethodAvailability::real("realtimeNow", 2, "l3_linux_clock::monotonic_and_realtime_are_both_kernel_reads"),
    MethodAvailability::real("createTimer", 3, "l3_linux_clock::a_timer_fires_on_the_kernels_own_timerfd"),
    MethodAvailability::real("cancelTimer", 4, "l3_linux_clock::cancel_retires_the_timerfd"),
    MethodAvailability::real("resetTimer", 5, "l3_linux_clock::reset_rearms_the_same_timer"),
    MethodAvailability::real("checkTimer", 6, "l3_linux_clock::a_periodic_timer_counts_every_firing"),
    MethodAvailability::real("subscribeTimers", 7, "l3_linux_clock::subscription_pull_is_bounded"),
    MethodAvailability::real("pullTimers", 0xFFFE, "l3_linux_clock::subscription_pull_is_bounded"), // TimerSubscription.pull @0
    MethodAvailability::real("cancelTimers", 0xFFFD, "l3_linux_clock::cancel_retires_the_subscription"), // TimerSubscription.cancel @1
    MethodAvailability::real("sleep", 8, "l3_linux_clock::sleep_sleeps_the_callers_own_thread"),
    MethodAvailability::real("listClockDomains", 9, "l3_linux_clock::clock_domains_are_measured_not_assumed"),
    MethodAvailability::real("sleepCancellable", 10, "l3_linux_clock::a_cancellable_sleep_is_really_cancellable"),
    MethodAvailability::degraded("getClockSync", 11, "l3_linux_clock::clock_sync_is_measured_not_assumed", "MEASURED (not assumed): a chronyd process exists on this host, so the host runs a synchronization daemon and that fact is reported; the daemon's own sync state (stratum, offset, last sync) is served by /run/chrony, which is root-only for this uid (measured EACCES), so `synced` stays false and `stratum` stays 0 rather than being invented. The read never writes the clock and never asks chronyc (a shellout)"),
    MethodAvailability::degraded("getClockDrift", 12, "l3_linux_clock::drift_is_measured_over_a_bounded_window", "a real measurement: both domains are read twice across a bounded window and their divergence is divided out, with `accuracy` bounded by half the window; it is deliberately not a long-run drift estimate, and the reason says so"),
    MethodAvailability::real("createVirtualClock", 13, "l3_linux_clock::a_virtual_clock_advances_only_when_told"),
    MethodAvailability::real("advanceVirtualClock", 14, "l3_linux_clock::a_virtual_clock_advances_only_when_told"),
    MethodAvailability::degraded("getClockQuality", 15, "l3_linux_clock::clock_quality_names_what_is_measurable", "the measurable half of a source's quality — the daemon behind it and the resolution the kernel reports; the statistical half (jitter, reliability over N samples) is not exported by this host, so reliability is 0.0 and unnamed rather than guessed"),
    MethodAvailability::degraded("persistTimer", 16, "l3_linux_clock::persistence_has_no_host_store", "a physical limit, stated: this host has no timer-persistence daemon, so the store is the process's own memory and a persisted timer does NOT survive the process exit the contract names. The code path is complete and mock-proven; a host with a persistence store lifts it"),
    MethodAvailability::degraded("listPersistedTimers", 17, "l3_linux_clock::persistence_has_no_host_store", "the same physical limit as persistTimer: the list is this process's memory, so it is empty after an exit. Reported as DEGRADED rather than UNAVAILABLE because the query is answered with the truth, not refused"),
    MethodAvailability::real("setTimerDependency", 18, "l3_linux_clock::a_gated_timer_does_not_fire_early"),
    MethodAvailability::real("alarms", 19, "l3_linux_clock::the_alarm_service_is_acquired_with_a_capability"),
    // ── The `AlarmService` interface (`0x100 + its own ordinal`) ─────────────
    MethodAvailability::real("createAlarm", 0x100, "l3_linux_clock::alarms_round_trip_through_the_store"),
    MethodAvailability::real("updateAlarm", 0x101, "l3_linux_clock::alarms_round_trip_through_the_store"),
    MethodAvailability::real("deleteAlarm", 0x102, "l3_linux_clock::alarms_round_trip_through_the_store"),
    MethodAvailability::real("getAlarm", 0x103, "l3_linux_clock::a_missing_alarm_is_not_found_not_zeroed"),
    MethodAvailability::real("listAlarms", 0x104, "l3_linux_clock::alarm_listing_is_paginated"),
    MethodAvailability::real("snoozeAlarm", 0x105, "l3_linux_clock::snooze_defers_the_next_firing"),
    MethodAvailability::real("dismissAlarm", 0x106, "l3_linux_clock::dismiss_disables_until_told_otherwise"),
    MethodAvailability::real("getNextAlarm", 0x107, "l3_linux_clock::the_next_alarm_is_the_earliest_enabled_one"),
];

// The `unavailable` constructor and the `Unknown` reason class are the matrix's
// own extension points: a row this build does not use today, and the flagged
// gap the tests assert stays unused. They exist so a future host row is data,
// not a schema change.

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records (supported, not merely
/// linked). Clock reads are transport-independent.
pub const TRANSPORTS: &[&str] = &["InProcess", "Shmem", "UnixSocket"];
/// Published clock capability tokens (open vocabulary).
pub const CAPS: &[&str] = &[
    "clock.realtime",
    "clock.monotonic",
    "clock.timers",
    "clock.alarms",
    "clock.sync",
];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in CLOCK_AVAILABILITY {
        match row.status {
            AvailStatus::Real => real += 1,
            AvailStatus::Degraded => degraded += 1,
            AvailStatus::Unavailable => unavailable += 1,
        }
    }
    (real, degraded, unavailable)
}

/// Every `l3_test` id the matrix claims — surfaced so the test suite can assert
/// each one exists (a REAL claim with a vanished test is a FAIL).
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    CLOCK_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for one contract method name, or `None` when the matrix has no such
/// method. A name no row answers is a caller bug, not an empty answer.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    CLOCK_AVAILABILITY.iter().find(|r| r.name == name)
}
