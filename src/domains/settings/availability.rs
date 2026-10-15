//! The settings domain availability matrix: one row per `Settings` contract
//! method, published as data exactly the way `knowledge/03-CONFORMANCE.md` §3.2
//! prescribes. A `REAL`/`DEGRADED` claim names the `l3Test` that backs it; an
//! `UNAVAILABLE` row carries a nameable `reasonClass` and a reason that the
//! live behaviour repeats at runtime.
//!
//! ## The mechanism behind every row (charter law #9)
//!
//! The Linux backend speaks the host's own settings engine — glib's
//! `GSettings` machinery, through the maintained `gio` crate — which is exactly
//! what the host's own `gsettings` command is a thin CLI over. The schema and
//! its defaults are read from the host's own compiled schema store, and a
//! per-user override is read from the host's own dconf database. There is no
//! `gsettings` fork and no `dconf` fork anywhere in this backend (the archive
//! body's `Command::new("gsettings")` per key is the pattern it exists to end).

/// The compiled schema file ID, read from `settings.capnp`'s `@0x…` line.
pub const SETTINGS_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60022;

/// One of the four availability reasons (`cli.capnp` `AvailabilityReason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    NoBackend,
    NoPrivilege,
    NotApplicable,
    DisabledByPolicy,
    /// Reserved and flagged: a distinguishable gap. Never used in this table.
    Unknown,
}

impl ReasonClass {
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
    Real,
    Degraded,
    Unavailable,
}

/// One availability row.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    pub name: &'static str,
    pub ordinal: u16,
    pub status: AvailStatus,
    pub reason_class: Option<ReasonClass>,
    pub reason: &'static str,
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

/// The published availability matrix for the settings domain on Linux.
///
/// 20 rows: the 18 `Settings` methods (`@0`–`@17`) plus the subscription
/// interface's two.
pub const SETTINGS_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("get", 0, "l3_linux_settings::a_read_is_the_hosts_own_settings_engine"),
    MethodAvailability::degraded("set", 1, "l3_linux_settings::a_write_is_the_hosts_own_engine_and_gated", "the write goes through the host's own engine (a dconf write), which is exactly what `gsettings set` performs — so the mechanism is REAL — and it is GATED: writing a setting is a host-mutating act this library never performs in a build or a test (law #8), and `system.*` needs the platform capability the contract names"),
    MethodAvailability::real("getAll", 2, "l3_linux_settings::a_schema_read_lists_the_hosts_own_keys"),
    MethodAvailability::degraded("delete", 3, "l3_linux_settings::a_write_is_the_hosts_own_engine_and_gated", "the same gated dconf write as @set: the mechanism is the host's own engine and the act is the founder's"),
    MethodAvailability::degraded("clear", 4, "l3_linux_settings::a_write_is_the_hosts_own_engine_and_gated", "the same gated dconf write as @set"),
    MethodAvailability::real("listDomains", 5, "l3_linux_settings::a_domain_list_is_the_hosts_own_schema_source"),
    MethodAvailability::real("watch", 6, "l3_linux_settings::a_watch_reports_the_hosts_own_change_signal"),
    MethodAvailability::real("watchDomain", 7, "l3_linux_settings::a_watch_reports_the_hosts_own_change_signal"),
    MethodAvailability::degraded("bulkSet", 8, "l3_linux_settings::a_write_is_the_hosts_own_engine_and_gated", "the same gated dconf write as @set, applied per entry; the read half of the bulk pair is real"),
    MethodAvailability::real("bulkGet", 9, "l3_linux_settings::a_bulk_read_answers_every_key"),
    MethodAvailability::degraded("listByCategory", 10, "l3_linux_settings::a_category_listing_reports_the_hosts_own_keys", "MEASURED: the contract's category tokens are conventions, not contract, and this host's schema source does not publish a per-key category (glib's schema carries a summary and a range, not a category), so the listing answers the host's own keys and records the requested category beside each one rather than guessing which keys are `display`"),
    MethodAvailability::unavailable("setAppOverride", 11, ReasonClass::NoPrivilege, "no-privilege: the contract states it — whether app X may override setting Y is a capability decision made at ACQUISITION, not a parameter this service re-decides per call. This host's own engine has no per-app override store to write into either (MEASURED: dconf has no per-app namespace), so the verb refuses with the reason rather than inventing a hierarchy"),
    MethodAvailability::degraded("getAppOverride", 12, "l3_linux_settings::an_app_override_answers_exists", "`exists` rather than an error, because \"not set\" is an answer: this host's engine carries no per-app override (MEASURED), so every key answers exists=false through the contract's own `exists` field and never a fabricated value"),
    MethodAvailability::unavailable("clearAppOverride", 13, ReasonClass::NoPrivilege, "no-privilege: the same acquisition-time capability limit as setAppOverride, and the same measured absence of a per-app namespace in this host's engine"),
    MethodAvailability::real("getVersion", 14, "l3_linux_settings::a_version_is_the_backends_own_monotonic_counter"),
    MethodAvailability::degraded("setIfUnchanged", 15, "l3_linux_settings::an_optimistic_set_reports_the_conflict_it_finds", "the optimistic-concurrency check is real (the version the backend handed the caller is compared before anything is written) and the write itself is the same gated dconf write as @set, so a mismatched version answers a conflict without touching the host"),
    MethodAvailability::real("getHistory", 16, "l3_linux_settings::a_history_is_the_backends_own_recorded_changes"),
    MethodAvailability::real("getSettingsDiff", 17, "l3_linux_settings::a_delta_read_returns_the_changes_after_the_version"),
    MethodAvailability::real("SettingsSubscription.pull", 0x100, "l3_linux_settings::a_watch_pull_is_bounded_and_cancellable"),
    MethodAvailability::real("SettingsSubscription.cancel", 0x101, "l3_linux_settings::a_watch_pull_is_bounded_and_cancellable"),
];

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records.
pub const TRANSPORTS: &[&str] = &["InProcess", "Shmem", "UnixSocket"];
/// Published settings capability tokens (open vocabulary).
pub const CAPS: &[&str] = &["settings.read", "settings.subscribe", "settings.write"];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in SETTINGS_AVAILABILITY {
        match row.status {
            AvailStatus::Real => real += 1,
            AvailStatus::Degraded => degraded += 1,
            AvailStatus::Unavailable => unavailable += 1,
        }
    }
    (real, degraded, unavailable)
}

/// Every `l3_test` id the matrix claims.
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    SETTINGS_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for one contract method name, or `None`.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    SETTINGS_AVAILABILITY.iter().find(|r| r.name == name)
}
