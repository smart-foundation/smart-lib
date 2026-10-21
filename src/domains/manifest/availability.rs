//! The **manifest** availability matrix: what this platform really serves.
//!
//! The manifests this platform holds are the engine's own — the contract's
//! manifests describe the engine's own bundles, and this host publishes no
//! bundle corpus — so the store's read verbs are real and the verbs that
//! need machinery this engine does not have are not.

/// The canonical schema id of `manifest.capnp` (`@0xa1b2c3d4e5f60037`).
pub const MANIFEST_SCHEMA_ID: u64 = 11651590505119481911;

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

/// The matrix: one row per method `manifest.capnp`'s `ManifestService`
/// declares (and the subscription's own `pull`/`cancel`), in ordinal order.
pub const MANIFEST_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real(
        "resolve",
        0,
        "l3_linux_manifest::a_registered_manifest_resolves",
    ),
    MethodAvailability::real(
        "list",
        1,
        "l3_linux_manifest::the_listing_is_the_stores_own",
    ),
    MethodAvailability::unavailable(
        "subscribe",
        2,
        ReasonClass::NoBackend,
        "no change feed: the store keeps no subscriber list and runs no publisher-side stream",
    ),
    MethodAvailability::unavailable(
        "pull",
        3,
        ReasonClass::NoBackend,
        "no change feed to pull from — the same limit as @subscribe",
    ),
    MethodAvailability::unavailable(
        "compose",
        4,
        ReasonClass::NoBackend,
        "no composition engine: the store holds manifests, it does not merge them into a bundle",
    ),
    MethodAvailability::unavailable(
        "diff",
        5,
        ReasonClass::NoBackend,
        "no diff surface: the store keeps one manifest per name, so there is no second revision to compare against",
    ),
    MethodAvailability::real(
        "validate",
        6,
        "l3_linux_manifest::a_validation_reports_the_dependency_it_cannot_find",
    ),
    MethodAvailability::unavailable(
        "sign",
        7,
        ReasonClass::NoPrivilege,
        "no-privilege: signing is a host-mutating act (it produces an attestation this library never writes, law #8) and the contract's key id names a keyring this host does not publish to the engine",
    ),
    MethodAvailability::unavailable(
        "verify",
        8,
        ReasonClass::NoBackend,
        "no signature to verify: nothing on this host attests the engine's manifests, so a verification would be a claim about nothing",
    ),
    MethodAvailability::unavailable(
        "migrate",
        9,
        ReasonClass::NoBackend,
        "no migration engine: the store holds manifests, it does not rewrite them into another bundle shape",
    ),
    MethodAvailability::unavailable(
        "createTemplate",
        10,
        ReasonClass::NoBackend,
        "no template store: the contract's template is a manifest, and this store holds manifests it was given — it does not mint new ones from a base",
    ),
    MethodAvailability::unavailable(
        "composeWithConfig",
        11,
        ReasonClass::NoBackend,
        "no composition engine: the same limit as @compose, with a merge configuration over it",
    ),
    MethodAvailability::real(
        "validateDependencies",
        12,
        "l3_linux_manifest::validate_dependencies_reports_the_dependency_it_cannot_find",
    ),
    MethodAvailability::real(
        "resolveByName",
        13,
        "l3_linux_manifest::a_human_name_resolves_through_its_vendor",
    ),
    MethodAvailability::unavailable(
        "pull",
        14,
        ReasonClass::NoBackend,
        "no change feed to pull from — the same limit as @subscribe",
    ),
    MethodAvailability::unavailable(
        "cancel",
        15,
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
    for row in MANIFEST_AVAILABILITY {
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
    MANIFEST_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for a method name.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    MANIFEST_AVAILABILITY.iter().find(|r| r.name == name)
}
