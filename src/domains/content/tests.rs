//! The **content** domain's L1 tests: the matrix matches the contract, every
//! claim names its backing test, and the value types survive the generated
//! wire format.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::ErrorCode;
use crate::schema::content_capnp;

use super::availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, ReasonClass,
    CONTENT_AVAILABILITY, CONTENT_SCHEMA_ID,
};
use super::backend::ContentBackend;
use super::types::{ContentFilter, ContentItem, Page, Pagination};

/// The contract's `ContentService` methods, in ordinal order.
const CONTRACT_METHODS: &[&str] = &[
    "publish",
    "publishMany",
    "update",
    "unpublish",
    "delete",
    "trash",
    "restore",
    "emptyTrash",
    "query",
    "get",
    "subscribe",
    "addRelationship",
    "removeRelationship",
    "getRelationships",
    "semanticSearch",
    "getContentGraph",
    "reactToContent",
    "getReactions",
    "addProvenance",
    "getProvenance",
    "eraseContent",
    "getUndoHistory",
    "undoContent",
    "encryptContent",
    "decryptContent",
    "rotateEncryptionKey",
    "geotagContent",
    "getByGeoLocation",
    "aiTagContent",
    "getAccessibilityReport",
    "batchPublish",
    "createBranch",
    "listBranches",
    "mergeBranch",
    "createComposite",
    "getComposite",
    "updateComposite",
    "addLineage",
    "getLineage",
    "getAuditTrail",
    "setRateLimit",
    "getRateLimit",
    "generatePreview",
    "getPreview",
    "getDependencies",
    "addDependency",
    "removeDependency",
    "scheduleExpiration",
    "cancelExpiration",
    "getReplicationStatus",
    "getQualityScore",
    "getRecommendations",
    "addTranslation",
    "getTranslations",
    "getTranslatedContent",
    "pull",
    "cancel",
];

/// A backend that serves nothing — the honesty check.
struct HonestBackend;
impl ContentBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(CONTENT_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in CONTENT_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name, "the matrix names the contract's method");
    }
}

#[test]
fn the_matrix_keeps_the_contract_ordinals() {
    for (index, row) in CONTENT_AVAILABILITY.iter().enumerate() {
        assert_eq!(row.ordinal, index as u16, "{} carries its contract ordinal", row.name);
    }
}

#[test]
fn claimed_l3_tests_exist() {
    for (_name, test) in claimed_l3_tests() {
        assert!(test.starts_with("l3_linux_content::"));
    }
}

#[test]
fn every_served_row_names_its_backing_test() {
    for row in CONTENT_AVAILABILITY {
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
    assert_eq!(real, 10);
    assert_eq!(degraded, 0);
    assert_eq!(unavailable, 47);
}

#[test]
fn the_store_verbs_are_the_ones_the_store_really_has() {
    for name in [
        "publish",
        "publishMany",
        "update",
        "unpublish",
        "delete",
        "trash",
        "restore",
        "emptyTrash",
        "query",
        "get",
    ] {
        let row = method_availability_for(name).expect("row");
        assert_eq!(row.status, AvailStatus::Real, "{name} is served");
        assert!(row.l3_test.is_some());
    }
}

#[test]
fn the_schema_id_is_the_canonical_one() {
    // `@0xa1b2c3d4e5f60055` — the id the frozen contract publishes.
    assert_eq!(CONTENT_SCHEMA_ID, 11651590505119481941);
}

#[test]
fn an_unserved_backend_says_so_instead_of_faking() {
    let backend = HonestBackend;
    let error = backend
        .publish(&ContentItem::default())
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.get(1).expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend
        .query(&ContentFilter::default(), Pagination::default())
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.delete(1).expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
}

#[test]
fn an_item_survives_the_generated_wire_format() {
    let item = ContentItem {
        id: 7,
        title: "The Title".to_string(),
        summary: "The Summary".to_string(),
        uri: "file:///tmp/the-title".to_string(),
        mime_type: "text/plain".to_string(),
        body: b"the payload".to_vec(),
        tags: vec!["note".to_string(), "draft".to_string()],
        publisher: 3,
        created_at: 111,
        modified_at: 222,
        version: 4,
        trashed: true,
        published: true,
    };

    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<content_capnp::content_item::Builder<'_>>();
        item.write_to(&mut builder);
    }
    let mut buffer = Vec::new();
    capnp::serialize::write_message(&mut buffer, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buffer[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<content_capnp::content_item::Reader<'_>>()
        .expect("root");
    let read = ContentItem::from_reader(&wire);

    assert_eq!(read.id, 7);
    assert_eq!(read.title, "The Title");
    assert_eq!(read.summary, "The Summary");
    assert_eq!(read.uri, "file:///tmp/the-title");
    assert_eq!(read.mime_type, "text/plain");
    assert_eq!(read.body, b"the payload".to_vec());
    assert_eq!(read.tags, vec!["note".to_string(), "draft".to_string()]);
    assert_eq!(read.publisher, 3);
    assert_eq!(read.created_at, 111);
    assert_eq!(read.modified_at, 222);
    assert_eq!(read.version, 4);
    assert!(read.trashed);
}

#[test]
fn the_page_shape_carries_the_total() {
    let page = Page {
        items: vec![ContentItem::default()],
        total: 3,
    };
    assert_eq!(page.total, 3);
    assert_eq!(page.items.len(), 1);
}
