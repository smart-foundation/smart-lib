//! The **vcard** domain's L1 tests: the matrix matches the contract, every
//! claim names its backing test, and the value types survive the generated
//! wire format.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::vcard_capnp;

use super::availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, ReasonClass,
    VCARD_AVAILABILITY, VCARD_SCHEMA_ID,
};
use super::backend::VcardBackend;
use super::types::{
    DuplicateHandling, ExportRequest, FormatRequest, ImportRequest, OutputFormat, VCard,
    VCardParameter, VCardProperty, VCardValue, VCardVersion,
};

/// The contract's `VCardService` methods, in ordinal order.
const CONTRACT_METHODS: &[&str] = &[
    "importVCard",
    "exportVCard",
    "parseVCard",
    "formatVCard",
    "getSupportedVersions",
    "getSyncToken",
    "syncChanges",
    "resolveConflict",
];

/// A backend that serves nothing — the honesty check.
struct HonestBackend;
impl VcardBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(VCARD_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in VCARD_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn the_matrix_keeps_the_contract_ordinals() {
    for (index, row) in VCARD_AVAILABILITY.iter().enumerate() {
        assert_eq!(row.ordinal, index as u16, "{} carries its contract ordinal", row.name);
    }
}

#[test]
fn claimed_l3_tests_exist() {
    for (_name, test) in claimed_l3_tests() {
        assert!(test.starts_with("l3_linux_vcard::"));
    }
}

#[test]
fn every_real_row_names_its_backing_test() {
    for row in VCARD_AVAILABILITY {
        match row.status {
            AvailStatus::Real => {
                assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
                assert!(row.reason_class.is_none());
            }
            AvailStatus::Unavailable => {
                assert!(row.l3_test.is_none(), "{} claims no test it does not have", row.name);
                assert!(row.reason_class.is_some(), "{} names why", row.name);
                assert!(!row.reason.is_empty(), "{} states the reason", row.name);
            }
            AvailStatus::Degraded => {
                assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
                assert!(!row.reason.is_empty(), "{} states the limit", row.name);
            }
        }
    }
}

#[test]
fn the_counts_add_up() {
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, CONTRACT_METHODS.len());
    assert_eq!(real, 7);
    assert_eq!(degraded, 0);
    assert_eq!(unavailable, 1);
}

#[test]
fn the_only_unavailable_method_names_its_reason() {
    let row = method_availability_for("resolveConflict").expect("row");
    assert_eq!(row.status, AvailStatus::Unavailable);
    assert_eq!(row.reason_class, Some(ReasonClass::NoBackend));
    assert!(row.reason.contains("no remote peer"));
}

#[test]
fn the_schema_id_is_the_canonical_one() {
    // `@0xa1b2c3d4e5f60108` — the id the frozen contract publishes.
    assert_eq!(VCARD_SCHEMA_ID, 11651590505119482120);
}

#[test]
fn an_unserved_backend_says_so_instead_of_faking() {
    let backend = HonestBackend;
    let error = backend.parse_v_card(b"", VCardVersion::Auto).expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    assert!(error.message.contains("no-backend"));
    let error = backend
        .import_v_card(ImportRequest::default())
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend
        .export_v_card(ExportRequest::default())
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend
        .format_v_card(FormatRequest::default())
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.get_supported_versions().expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.get_sync_token(&[]).expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.sync_changes("token").expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend
        .resolve_conflict("conflict", super::ConflictResolution::Merge)
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
}

#[test]
fn a_parsed_card_survives_the_generated_wire_format() {
    let card = VCard {
        version: VCardVersion::V40,
        properties: vec![
            VCardProperty {
                name: "FN".to_string(),
                group: String::new(),
                parameters: Vec::new(),
                values: vec![VCardValue::Text("Ada Lovelace".to_string())],
            },
            VCardProperty {
                name: "TEL".to_string(),
                group: "item1".to_string(),
                parameters: vec![VCardParameter {
                    name: "TYPE".to_string(),
                    value: "WORK".to_string(),
                }],
                values: vec![VCardValue::Text("+442079460018".to_string())],
            },
            VCardProperty {
                name: "N".to_string(),
                group: String::new(),
                parameters: Vec::new(),
                values: vec![VCardValue::Structured(vec![
                    "Lovelace".to_string(),
                    "Ada".to_string(),
                    String::new(),
                    String::new(),
                    String::new(),
                ])],
            },
            VCardProperty {
                name: "PHOTO".to_string(),
                group: String::new(),
                parameters: Vec::new(),
                values: vec![VCardValue::Binary(vec![0xf7, 0xd8, 0xff])],
            },
            VCardProperty {
                name: "URL".to_string(),
                group: String::new(),
                parameters: Vec::new(),
                values: vec![VCardValue::Uri("https://example.org/ada".to_string())],
            },
            VCardProperty {
                name: "BDAY".to_string(),
                group: String::new(),
                parameters: Vec::new(),
                values: vec![VCardValue::Date("1815-12-10".to_string())],
            },
        ],
        raw_lines: vec!["FN:Ada Lovelace".to_string()],
    };

    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<vcard_capnp::v_card::Builder<'_>>();
        card.write_to(&mut builder);
    }
    let mut buffer = Vec::new();
    capnp::serialize::write_message(&mut buffer, &message).expect("serialize");
    let reader = capnp::serialize::read_message(
        &mut &buffer[..],
        capnp::message::ReaderOptions::new(),
    )
    .expect("deserialize");
    let wire = reader
        .get_root::<vcard_capnp::v_card::Reader<'_>>()
        .expect("root");
    let read = VCard::from_reader(&wire);

    assert_eq!(read.version, VCardVersion::V40);
    assert_eq!(read.properties.len(), card.properties.len());
    assert_eq!(read.properties[0].values, card.properties[0].values);
    assert_eq!(read.properties[1].group, "item1");
    assert_eq!(read.properties[1].parameters[0].name, "TYPE");
    assert_eq!(read.properties[1].parameters[0].value, "WORK");
    assert_eq!(read.properties[2].values, card.properties[2].values);
    assert_eq!(read.properties[3].values, card.properties[3].values);
    assert_eq!(read.properties[4].values, card.properties[4].values);
    assert_eq!(read.properties[5].values, card.properties[5].values);
    assert_eq!(read.raw_lines, card.raw_lines);
}

#[test]
fn the_version_vocabulary_is_the_published_one() {
    assert_eq!(VCardVersion::SUPPORTED.len(), 3);
    assert_eq!(VCardVersion::V21.as_str(), "2.1");
    assert_eq!(VCardVersion::V30.as_str(), "3.0");
    assert_eq!(VCardVersion::V40.as_str(), "4.0");
    assert_eq!(VCardVersion::parse("4.0"), Some(VCardVersion::V40));
    assert_eq!(VCardVersion::parse("5.0"), None);
    assert_eq!(
        VCardVersion::from_generated(vcard_capnp::VCardVersion::V30),
        VCardVersion::V30
    );
    assert_eq!(
        VCardVersion::to_generated(VCardVersion::V21),
        vcard_capnp::VCardVersion::V21
    );
}

#[test]
fn the_duplicate_and_output_vocabularies_round_trip() {
    assert_eq!(
        DuplicateHandling::from_generated(DuplicateHandling::to_generated(DuplicateHandling::Merge)),
        DuplicateHandling::Merge
    );
    assert_eq!(
        OutputFormat::from_generated(OutputFormat::to_generated(OutputFormat::Stream)),
        OutputFormat::Stream
    );
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

/// The vcard cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/vcard.rs");

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
            "{method} claims {test}, which is not a test function in ../../platforms/linux/vcard.rs"
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
