//! The **vcard** availability matrix: what this platform really serves.
//!
//! Every row names the `L3` test that proves it (the `l3_linux_vcard::…`
//! tests live in `crate::platforms::linux::vcard`), and every row that is not
//! `Real` names the reason it is not — a fabricated `Real` is the one
//! unforgivable defect (law #6).

/// The canonical schema id of `vcard.capnp` (`@0xa1b2c3d4e5f60108`).
pub const VCARD_SCHEMA_ID: u64 = 11651590505119482120;

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

/// The matrix: one row per method `vcard.capnp`'s `VCardService` declares, in
/// ordinal order.
pub const VCARD_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real(
        "importVCard",
        0,
        "l3_linux_vcard::an_import_maps_the_fields_the_contract_names",
    ),
    MethodAvailability::real(
        "exportVCard",
        1,
        "l3_linux_vcard::an_export_writes_the_fields_the_contract_names",
    ),
    MethodAvailability::real(
        "parseVCard",
        2,
        "l3_linux_vcard::parse_v_card_reads_the_fields_the_bytes_carry",
    ),
    MethodAvailability::real(
        "formatVCard",
        3,
        "l3_linux_vcard::format_v_card_writes_the_fields_the_card_holds",
    ),
    MethodAvailability::real(
        "getSupportedVersions",
        4,
        "l3_linux_vcard::the_supported_versions_are_the_ones_the_engine_can_read",
    ),
    MethodAvailability::real(
        "getSyncToken",
        5,
        "l3_linux_vcard::get_sync_token_reports_the_engines_own_change_counter",
    ),
    MethodAvailability::real(
        "syncChanges",
        6,
        "l3_linux_vcard::sync_changes_hands_back_the_changes_since_the_token",
    ),
    MethodAvailability::unavailable(
        "resolveConflict",
        7,
        ReasonClass::NoBackend,
        "no conflict surface: the store is this engine's own memory, with no remote peer to conflict with, so no conflict is ever produced and there is nothing for this method to resolve — it refuses rather than pretending",
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
    for row in VCARD_AVAILABILITY {
        match row.status {
            AvailStatus::Real => r += 1,
            AvailStatus::Degraded => d += 1,
            AvailStatus::Unavailable => u += 1,
        }
    }
    (r, d, u)
}

/// Every `Real`/… row's backing test, paired with the method it proves.
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    VCARD_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for a method name.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    VCARD_AVAILABILITY.iter().find(|r| r.name == name)
}
