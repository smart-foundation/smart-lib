//! The **appindex** availability matrix: what this platform really serves.
//!
//! The read half is served from the host's own freedesktop desktop-entry
//! corpus — the `.desktop` files under the XDG data directories — so the
//! answers are the host's own applications, counted, not invented.

/// The canonical schema id of `appindex.capnp` (`@0xa1b2c3d4e5f6001a`).
pub const APPINDEX_SCHEMA_ID: u64 = 11651590505119481882;

/// Why a method is not served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    /// The platform has no surface for this method at all.
    NoBackend,
    /// The surface exists but this uid cannot reach it.
    NoPrivilege,
    /// The method does not apply to this platform.
    NotApplicable,
    /// A policy turned it off.
    DisabledByPolicy,
    /// The reason is not one of the named ones.
    Unknown,
}

impl ReasonClass {
    /// The reason class as the contract's spelling.
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

/// How well a method is served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus {
    /// Served, completely, by this platform's own mechanism.
    Real,
    /// Served, but with a stated limit.
    Degraded,
    /// Not served; the reason says why.
    Unavailable,
}

/// One row of the matrix.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    /// The contract's method name.
    pub name: &'static str,
    /// The method's ordinal in the contract.
    pub ordinal: u16,
    /// How well it is served.
    pub status: AvailStatus,
    /// Why it is not `Real` (empty when it is).
    pub reason_class: Option<ReasonClass>,
    /// The stated limit (empty when there is none).
    pub reason: &'static str,
    /// The `L3` test that proves the row.
    pub l3_test: Option<&'static str>,
}

impl MethodAvailability {
    /// A method this platform really serves.
    const fn real(name: &'static str, ordinal: u16, l3: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Real,
            reason_class: None,
            reason: "",
            l3_test: Some(l3),
        }
    }

    /// A method this platform serves with a stated limit.
    const fn degraded(name: &'static str, ordinal: u16, l3: &'static str, detail: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Degraded,
            reason_class: None,
            reason: detail,
            l3_test: Some(l3),
        }
    }

    /// A method this platform does not serve, and why.
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

/// The matrix: one row per method `appindex.capnp`'s `AppIndexService`
/// declares, in ordinal order.
pub const APPINDEX_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real(
        "listApps",
        0,
        "l3_linux_appindex::the_index_lists_the_hosts_own_desktop_entries",
    ),
    MethodAvailability::real(
        "getApp",
        1,
        "l3_linux_appindex::one_entry_is_read_by_its_own_id",
    ),
    MethodAvailability::unavailable(
        "launch",
        2,
        ReasonClass::NoPrivilege,
        "no-privilege: launching an application is a host-mutating act this library never performs (law #8); the host's own launcher owns that verb, and the path is code-complete and gated for the founder",
    ),
    MethodAvailability::unavailable(
        "launchUrl",
        3,
        ReasonClass::NoPrivilege,
        "no-privilege: the same host-mutating limit as @launch (law #8) — handing a URL to an application starts it",
    ),
    MethodAvailability::degraded(
        "getPinned",
        4,
        "l3_linux_appindex::get_pinned_answers_with_the_engines_own_pinned_set",
        "the pinned set is this engine's own in-process set: the host's pinned applications live in the desktop shell's own store (dconf), which this backend neither reads nor writes, so the answer is the engine's memory and not the launcher's",
    ),
    MethodAvailability::degraded(
        "pinApp",
        5,
        "l3_linux_appindex::pin_app_records_the_pin_in_the_engines_own_memory",
        "the same in-process pinned set as @getPinned: the engine records the pin in its own memory (it does not survive the process exit) and never writes the host launcher’s store, which is a host-mutating act (law #8)",
    ),
    MethodAvailability::degraded(
        "unpinApp",
        6,
        "l3_linux_appindex::unpin_app_takes_the_pin_back_out_of_the_engines_own_memory",
        "the same in-process pinned set as @getPinned",
    ),
    MethodAvailability::real(
        "searchApps",
        7,
        "l3_linux_appindex::a_search_answers_from_the_same_corpus",
    ),
    MethodAvailability::unavailable(
        "getRecentApps",
        8,
        ReasonClass::NoBackend,
        "no host surface: the recently used applications are the desktop shell's own recents (its XDG user-dirs file), which this backend does not read, and nothing else on this host publishes them",
    ),
    MethodAvailability::unavailable(
        "clearRecentApps",
        9,
        ReasonClass::NoBackend,
        "no host surface, and a host-mutating one besides: the recents live in the desktop shell's own file, which this library never writes (law #8)",
    ),
    MethodAvailability::unavailable(
        "subscribe",
        10,
        ReasonClass::NoBackend,
        "no change feed: the index is a snapshot of the host's desktop-entry corpus, and the host publishes no watch over it (the desktop entry database's monitor is not a contract surface)",
    ),
    MethodAvailability::unavailable(
        "pull",
        11,
        ReasonClass::NoBackend,
        "no change feed to pull from — the same limit as @subscribe",
    ),
    MethodAvailability::unavailable(
        "cancel",
        12,
        ReasonClass::NoBackend,
        "no change feed to cancel — the same limit as @subscribe",
    ),
    MethodAvailability::real(
        "getStats",
        13,
        "l3_linux_appindex::the_stats_count_the_hosts_own_corpus",
    ),
    MethodAvailability::unavailable(
        "listCapabilities",
        14,
        ReasonClass::NoBackend,
        "no surface: a desktop entry declares actions and MIME types, not the contract's capability vocabulary, so there is nothing to list that would mean the same thing",
    ),
    MethodAvailability::unavailable(
        "getDependents",
        15,
        ReasonClass::NoBackend,
        "no surface: the host publishes no dependency graph between desktop entries — a desktop entry names what it opens, not what needs it",
    ),
    MethodAvailability::real(
        "batchGet",
        16,
        "l3_linux_appindex::a_batch_answers_from_the_same_corpus",
    ),
    MethodAvailability::unavailable(
        "pull",
        17,
        ReasonClass::NoBackend,
        "no change feed to pull from — the same limit as @subscribe",
    ),
    MethodAvailability::unavailable(
        "cancel",
        18,
        ReasonClass::NoBackend,
        "no change feed to cancel — the same limit as @subscribe",
    ),
];

/// The platform this matrix describes.
pub const PLATFORM: &str = "linux";

/// The transports the domain is served over.
pub const TRANSPORTS: &[&str] = &["InProcess"];

/// The contract caps this domain publishes.
pub const CAPS: &[&str] = &[];

/// How many rows are `Real`, `Degraded` and `Unavailable`.
pub fn status_counts() -> (usize, usize, usize) {
    let mut r = 0;
    let mut d = 0;
    let mut u = 0;
    for row in APPINDEX_AVAILABILITY {
        match row.status {
            AvailStatus::Real => r += 1,
            AvailStatus::Degraded => d += 1,
            AvailStatus::Unavailable => u += 1,
        }
    }
    (r, d, u)
}

/// Every `Real`/`Degraded` row's backing test, paired with the method it
/// proves.
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    APPINDEX_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for a method name.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    APPINDEX_AVAILABILITY.iter().find(|r| r.name == name)
}
