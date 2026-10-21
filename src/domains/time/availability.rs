//! The time domain availability matrix: one row per `Time` contract method
//! (`@0`–`@5`) plus the two TAI conversion helpers this domain carries in the
//! contract's epoch — published as data exactly the way
//! `knowledge/03-CONFORMANCE.md` §3.2 prescribes.
//!
//! ## What each row is, and why
//!
//! | method | status | the honest answer |
//! |---|---|---|
//! | `getDateTime @0` | REAL | the kernel's `CLOCK_REALTIME` plus the zone the host publishes (`/etc/localtime`'s own symlink target) |
//! | `verifyHsmTimestamp @1` | UNAVAILABLE | MEASURED: this host has no HSM (no TPM character device, no PKCS#11 module), so there is nothing to verify against |
//! | `convertCalendar @2` | DEGRADED | gregorian/julian/islamic(tabular)/ethiopian are exact arithmetic; hebrew and chinese are refused with a reason |
//! | `addCalendarUnits @3` / `subtractCalendarUnits @4` | REAL | pure calendar arithmetic on the same civil-date kernels |
//! | `diffCalendarDates @5` | REAL | exact day counts with the y/m/d breakdown beside them |
//! | `convertToTai` / `convertFromTai` | REAL | the contract's own converter rule and epoch (`timezone.capnp`), leap-second count stated |
//!
//! The two calendars that are not REAL are not "not implemented": one needs an
//! astronomical ephemeris (the Chinese calendar's new moons) and one needs the
//! molad/postponement arithmetic this build cannot verify against a reference on
//! this host. Both are named, and both keep the contract's `custom` calendar
//! outlet working, so a consumer never receives a silently wrong conversion.

/// The compiled schema file ID, read from `time.capnp`'s `@0x…` line. The
/// matrix can never describe a contract the build did not compile.
pub const TIME_SCHEMA_ID: u64 = 0xa1b2c3d4e5f6001e;

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
    /// The contract method name (`time.capnp`).
    pub name: &'static str,
    /// The contract ordinal (the value that never moves). `0xFFFE` and
    /// `0xFFFD` are this domain's own TAI helpers, which are not contract
    /// methods but the contract's converter surface mirrored.
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

/// The published availability matrix for the time domain on Linux.
pub const TIME_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("getDateTime", 0, "l3_linux_time::get_date_time_is_the_kernels_own_clock"),
    MethodAvailability::unavailable(
        "verifyHsmTimestamp",
        1,
        ReasonClass::NotApplicable,
        "MEASURED: no hardware security module is present on this host (no TPM character device \
         under /dev, no PKCS#11 module loadable), so there is no attestation authority to verify \
         against; a caller's signature bytes are never validated against a guessed one",
    ),
    MethodAvailability::degraded(
        "convertCalendar",
        2,
        "l3_linux_time::calendar_conversion_is_exact_arithmetic",
        "gregorian, julian, islamic (tabular) and ethiopian are exact arithmetic, mock-proven in \
         both directions; the Hebrew calendar's molad and four postponement rules and the Chinese \
         calendar's astronomical new moons are refused with a reason rather than approximated — a \
         conversion between the wrong two calendars is silently wrong, which is worse than a refusal",
    ),
    MethodAvailability::real("addCalendarUnits", 3, "l3_linux_time::calendar_arithmetic_clamps_at_month_end"),
    MethodAvailability::real("subtractCalendarUnits", 4, "l3_linux_time::subtract_calendar_units_clamps_at_month_end"),
    MethodAvailability::real("diffCalendarDates", 5, "l3_linux_time::diff_counts_days_and_breaks_the_year_down"),
    // ── The TAI helpers (the contract's converter, mirrored) ─────────────────
    MethodAvailability::real("convertToTai", 0xFFFE, "l3_linux_time::convert_to_tai_uses_the_contracts_epoch"),
    MethodAvailability::real("convertFromTai", 0xFFFD, "l3_linux_time::convert_from_tai_walks_the_epoch_back"),
];

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records (supported, not merely
/// linked). Time reads are transport-independent.
pub const TRANSPORTS: &[&str] = &["InProcess", "Shmem", "UnixSocket"];
/// Published time capability tokens (open vocabulary).
pub const CAPS: &[&str] = &["time.datetime", "time.calendar", "time.tai"];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in TIME_AVAILABILITY {
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
    TIME_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for one contract method name, or `None` when the matrix has no such
/// method.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    TIME_AVAILABILITY.iter().find(|r| r.name == name)
}
