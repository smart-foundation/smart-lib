//! The **manifest** domain's L1 tests: the matrix matches the contract, every
//! claim names its backing test, and the value types survive the generated
//! wire format.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::core::ErrorCode;
use crate::schema::manifest_capnp;

use super::availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, ReasonClass,
    MANIFEST_AVAILABILITY, MANIFEST_SCHEMA_ID,
};
use super::backend::ManifestBackend;
use super::types::{ManifestDependency, ManifestEntry, Page, Pagination, ValidationReport};

/// The contract's `ManifestService` methods, in ordinal order.
const CONTRACT_METHODS: &[&str] = &[
    "resolve",
    "list",
    "subscribe",
    "pull",
    "compose",
    "diff",
    "validate",
    "sign",
    "verify",
    "migrate",
    "createTemplate",
    "composeWithConfig",
    "validateDependencies",
    "resolveByName",
    "pull",
    "cancel",
];

/// A backend that serves nothing — the honesty check.
struct HonestBackend;
impl ManifestBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(MANIFEST_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in MANIFEST_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn the_matrix_keeps_the_contract_ordinals() {
    for (index, row) in MANIFEST_AVAILABILITY.iter().enumerate() {
        assert_eq!(row.ordinal, index as u16, "{} carries its contract ordinal", row.name);
    }
}

#[test]
fn claimed_l3_tests_exist() {
    for (_name, test) in claimed_l3_tests() {
        assert!(test.starts_with("l3_linux_manifest::"));
    }
}

#[test]
fn every_served_row_names_its_backing_test() {
    for row in MANIFEST_AVAILABILITY {
        match row.status {
            AvailStatus::Real => {
                assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
                assert!(row.reason.is_empty(), "{} states no limit", row.name);
            }
            AvailStatus::Degraded => {
                assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
                assert!(!row.reason.is_empty(), "{} states the limit", row.name);
            }
            AvailStatus::Unavailable => {
                assert!(row.l3_test.is_none(), "{} claims no test it does not have", row.name);
                assert!(row.reason_class.is_some(), "{} names why", row.name);
                assert!(!row.reason.is_empty(), "{} states the reason", row.name);
            }
        }
    }
}

#[test]
fn the_counts_add_up() {
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, CONTRACT_METHODS.len());
    assert_eq!(real, 5);
    assert_eq!(degraded, 0);
    assert_eq!(unavailable, 11);
}

#[test]
fn signing_names_the_law() {
    let row = method_availability_for("sign").expect("row");
    assert_eq!(row.status, AvailStatus::Unavailable);
    assert_eq!(row.reason_class, Some(ReasonClass::NoPrivilege));
    assert!(row.reason.contains("law #8"));
}

#[test]
fn the_schema_id_is_the_canonical_one() {
    // `@0xa1b2c3d4e5f60037` — the id the frozen contract publishes.
    assert_eq!(MANIFEST_SCHEMA_ID, 11651590505119481911);
}

#[test]
fn an_unserved_backend_says_so_instead_of_faking() {
    let backend = HonestBackend;
    let error = backend.resolve("x").expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend
        .list(Pagination::default())
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.validate("x").expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend
        .resolve_by_name("", "x")
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
}

#[test]
fn a_manifest_survives_the_generated_wire_format() {
    let manifest = ManifestEntry {
        name: "smart.shell".to_string(),
        display_name: "Shell".to_string(),
        description: "The shell".to_string(),
        vendor: "Smart".to_string(),
        version: "1.2.3".to_string(),
        entries: vec!["main".to_string()],
        depends: vec![ManifestDependency {
            schema_id: 42,
            name: "schema.base".to_string(),
            optional: false,
        }],
        offers: vec!["schema.shell".to_string()],
        shape: "executable".to_string(),
    };

    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<manifest_capnp::manifest::Builder<'_>>();
        manifest.write_to(&mut builder);
    }
    let mut buffer = Vec::new();
    capnp::serialize::write_message(&mut buffer, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buffer[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<manifest_capnp::manifest::Reader<'_>>()
        .expect("root");
    let read = ManifestEntry::from_reader(&wire);

    assert_eq!(read.display_name, "Shell");
    assert_eq!(read.description, "The shell");
    assert_eq!(read.vendor, "Smart");
    assert_eq!(read.version, "1.2.3");
    assert_eq!(read.entries, vec!["main".to_string()]);
    assert_eq!(read.depends.len(), 1);
    assert_eq!(read.depends[0].name, "schema.base");
    assert_eq!(read.depends[0].schema_id, 42);
}

#[test]
fn the_validation_report_carries_its_errors() {
    let report = ValidationReport {
        valid: false,
        errors: vec!["the required dependency is missing".to_string()],
    };
    assert!(!report.valid);
    assert_eq!(report.errors.len(), 1);
    let page = Page {
        items: vec![ManifestEntry::default()],
        total: 1,
    };
    assert_eq!(page.total, 1);
}

// ── The proof gate ──────────────────────────────────────────────────────
//
// The matrix PUBLISHES a proof name per method, and a published name is a
// claim about this tree: "this test exists, and it proves this method".
// Two ways to lie about that are now impossible to ship:
//
//   1. a name that resolves to nothing — the test was renamed or deleted;
//   2. ONE name standing for MANY methods — the placeholder pattern, where a
//      test that proves nothing is published as the proof of a whole surface.
//
// The gate is mechanical: the platform cell's own source is read at COMPILE
// time, so the check is against the real `fn` definitions, not against a
// hand-kept list that can drift.

/// The manifest cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/manifest.rs");

/// Every source the cell's proofs may live in.
const PLATFORM_SOURCES: &[&str] = &[PLATFORM_SOURCE_0];


/// Does the platform cell really define this test function?
fn the_platform_defines(test: &str) -> bool {
    let name = test.rsplit("::").next().unwrap_or(test);
    PLATFORM_SOURCES.iter().any(|source| source.contains(&alloc::format!("fn {name}(")))
}

/// Every published proof name resolves to a real test function in the
/// platform cell. A name that resolves to nothing is a FAILURE.
#[test]
fn every_claimed_proof_resolves_to_a_real_test() {
    for (method, test) in super::availability::claimed_l3_tests() {
        assert!(
            the_platform_defines(test),
            "{method} claims {test}, which is not a test function in ../../platforms/linux/manifest.rs"
        );
    }
}

/// One proof names ONE method. A name published for many methods is the
/// placeholder pattern: the matrix would be publishing one test as if it
/// proved a whole surface.
#[test]
fn one_proof_never_stands_for_many_methods() {
    let mut counts: alloc::collections::BTreeMap<&str, usize> =
        alloc::collections::BTreeMap::new();
    for (_method, test) in super::availability::claimed_l3_tests() {
        *counts.entry(test).or_insert(0) += 1;
    }
    for (test, count) in counts {
        assert_eq!(
            count, 1,
            "{test} is published as the proof for {count} methods: a proof \
             names ONE method"
        );
    }
}
