//! The logging domain availability matrix: one row per `Logging` contract method
//! (`@0`–`@8`) plus the subscription helpers — published as data exactly the way
//! `knowledge/03-CONFORMANCE.md` §3.2 prescribes.
//!
//! ## What each row is, and why — measured on THIS host
//!
//! This host runs **runit** (measured: `/run/runit`, `/etc/sv`, 16 linked
//! services, no `systemd-journald` process and no `/run/systemd/journal`), has
//! **no `/dev/log`** (no syslog socket) and **no journald**, so the host's own
//! sinks are: the world-readable files under `/var/log` (measured:
//! `Xorg.0.log`, `Xorg.1.log` and their `.old` siblings) and, per service, the
//! svlogd directories under `/etc/sv/<n>/log` — whose *live* output lives under
//! `/run/runit/supervise.<n>-log`, which is **root-only for this uid** (measured
//! `EACCES`, mode `drwx------ root root`).
//!
//! | method | status | the honest answer |
//! |---|---|---|
//! | `log @0` | DEGRADED | writes this process's own stderr — the one sink this process owns on a host with no `/dev/log` and no journald; under a runit service that IS the service's svlogd input, so the entry really lands |
//! | `getEntries @1` | DEGRADED | this process's retained entries are a full, exact store; the host's own files are read natively with only the world-readable ones reachable (measured), and a denied one is named, never skipped silently |
//! | `subscribe @2` / `pull` / `cancel` | REAL | the pull model over this process's store, bounded by the caller |
//! | `setLevel @3` / `getLevel @4` | DEGRADED | configures this backend's filter; the host's daemons keep their own levels and this backend says so |
//! | `clear @5` | DEGRADED | clears this process's retention; truncating a root-owned `/var/log` file is refused with `no-privilege` |
//! | `export @6` | REAL | the retained entries, in the content type the caller names |
//! | `setSamplingRate @7` | REAL | declared sampling, applied by this backend and visible in `listDomains` |
//! | `listDomains @8` | REAL | the domains this backend knows, with their levels and sampling rates |
//!
//! No method on this backend parses a journal (there is none) and no method
//! forks `journalctl`/`sv`/`dmesg` — the archive body's shellouts are the
//! condemned past.

/// The compiled schema file ID, read from `logging.capnp`'s `@0x…` line. The
/// matrix can never describe a contract the build did not compile.
pub const LOGGING_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60024;

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
    /// The contract method name (`logging.capnp`).
    pub name: &'static str,
    /// The contract ordinal (the value that never moves).
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

/// The published availability matrix for the logging domain on Linux.
pub const LOGGING_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::degraded("log", 0, "l3_linux_logging::log_writes_the_one_sink_this_process_owns", "writes this process's own stderr (fd 2) — MEASURED: this host runs runit with no /dev/log and no systemd-journald, so the process's own stderr is the only sink this process owns; under a runit service that stream IS the svlogd input, so the entry really lands in the service's log. Retained in the backend's own store as well, bounded and declared, so the query path answers exactly"),
    MethodAvailability::degraded("getEntries", 1, "l3_linux_logging::entries_are_filtered_paginated_and_gap_detectable", "two sources, both honest: this process's retained entries (exact, complete) and the host's own log files, read natively — the world-readable ones are read and a root-only one is NAMED in the result's measured reason rather than silently skipped (this host's svlogd output lives under /run/runit/supervise.<n>-log, mode drwx------ root root, measured EACCES)"),
    MethodAvailability::real("subscribe", 2, "l3_linux_logging::a_subscription_pulls_bounded_and_cancels"),
    MethodAvailability::real("pull", 0xFFFE, "l3_linux_logging::a_subscription_pulls_bounded_and_cancels"), // LogSubscription.pull @0
    MethodAvailability::real("cancel", 0xFFFD, "l3_linux_logging::cancel_retires_the_subscription_token"), // LogSubscription.cancel @1
    MethodAvailability::degraded("setLevel", 3, "l3_linux_logging::levels_configure_this_backend_only", "configures THIS backend's per-domain filter; the host's own daemons (NetworkManager, Xorg, sshd) keep the levels they were started with, and the row says so rather than implying a host-wide change"),
    MethodAvailability::degraded("getLevel", 4, "l3_linux_logging::levels_configure_this_backend_only", "reads THIS backend's per-domain configuration; a host daemon's own level is not readable through a settings store on this host, so an unconfigured domain answers its default `info` and the reason names the limit"),
    MethodAvailability::degraded("clear", 5, "l3_linux_logging::clear_clears_this_process_and_refuses_root_files", "clears this process's retention; truncating a host log file the caller does not own (/var/log/dmesg.log is mode 0600 root:root, measured) is refused with no-privilege rather than attempted"),
    MethodAvailability::real("export", 6, "l3_linux_logging::export_carries_the_entries_in_the_named_content_type"),
    MethodAvailability::real("setSamplingRate", 7, "l3_linux_logging::declared_sampling_is_applied_and_visible"),
    MethodAvailability::real("listDomains", 8, "l3_linux_logging::domains_carry_their_level_and_sampling_rate"),
];

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records (supported, not merely
/// linked). Log writes are transport-independent.
pub const TRANSPORTS: &[&str] = &["InProcess", "Shmem", "UnixSocket"];
/// Published logging capability tokens (open vocabulary).
pub const CAPS: &[&str] = &[
    "logging.write",
    "logging.read",
    "logging.subscribe",
    "logging.export",
];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in LOGGING_AVAILABILITY {
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
    LOGGING_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for one contract method name, or `None` when the matrix has no such
/// method.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    LOGGING_AVAILABILITY.iter().find(|r| r.name == name)
}
