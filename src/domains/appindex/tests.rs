//! The **appindex** domain's L1 tests: the matrix matches the contract, every
//! claim names its backing test, and the value types survive the generated
//! wire format.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::ErrorCode;
use crate::schema::appindex_capnp;

use super::availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, ReasonClass,
    APPINDEX_AVAILABILITY, APPINDEX_SCHEMA_ID,
};
use super::backend::AppindexBackend;
use super::types::{AppEntry, AppSearchFilter, AppState, Page, Pagination};

/// The contract's `AppIndexService` methods, in ordinal order.
const CONTRACT_METHODS: &[&str] = &[
    "listApps",
    "getApp",
    "launch",
    "launchUrl",
    "getPinned",
    "pinApp",
    "unpinApp",
    "searchApps",
    "getRecentApps",
    "clearRecentApps",
    "subscribe",
    "pull",
    "cancel",
    "getStats",
    "listCapabilities",
    "getDependents",
    "batchGet",
    "pull",
    "cancel",
];

/// A backend that serves nothing — the honesty check.
struct HonestBackend;
impl AppindexBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(APPINDEX_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in APPINDEX_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn the_matrix_keeps_the_contract_ordinals() {
    for (index, row) in APPINDEX_AVAILABILITY.iter().enumerate() {
        assert_eq!(row.ordinal, index as u16, "{} carries its contract ordinal", row.name);
    }
}

#[test]
fn claimed_l3_tests_exist() {
    for (_name, test) in claimed_l3_tests() {
        assert!(test.starts_with("l3_linux_appindex::"));
    }
}

#[test]
fn every_served_row_names_its_backing_test() {
    for row in APPINDEX_AVAILABILITY {
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
    assert_eq!(degraded, 3);
    assert_eq!(unavailable, 11);
}

#[test]
fn the_host_mutating_verbs_name_the_law() {
    for name in ["launch", "launchUrl"] {
        let row = method_availability_for(name).expect("row");
        assert_eq!(row.status, AvailStatus::Unavailable);
        assert_eq!(row.reason_class, Some(ReasonClass::NoPrivilege));
        assert!(row.reason.contains("law #8"), "{} names the law", name);
    }
    // The recents are refused for the deeper reason first — the host's own
    // surface is not read — and the host-mutating half is named too.
    let row = method_availability_for("clearRecentApps").expect("row");
    assert_eq!(row.status, AvailStatus::Unavailable);
    assert_eq!(row.reason_class, Some(ReasonClass::NoBackend));
    assert!(row.reason.contains("law #8"));
}

#[test]
fn the_schema_id_is_the_canonical_one() {
    // `@0xa1b2c3d4e5f6001a` — the id the frozen contract publishes.
    assert_eq!(APPINDEX_SCHEMA_ID, 11651590505119481882);
}

#[test]
fn an_unserved_backend_says_so_instead_of_faking() {
    let backend = HonestBackend;
    let error = backend
        .list_apps(Pagination::default())
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.get_app("x").expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.get_stats().expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend.get_pinned().expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
    let error = backend
        .search_apps(&AppSearchFilter::default(), Pagination::default())
        .expect_err("no backend");
    assert_eq!(error.code, ErrorCode::NotSupported);
}

#[test]
fn an_entry_survives_the_generated_wire_format() {
    let entry = AppEntry {
        id: "alpha.desktop".to_string(),
        name: "Alpha Editor".to_string(),
        generic_name: "Editor".to_string(),
        comment: "Edit things".to_string(),
        exec: "alpha %U".to_string(),
        icon: "alpha".to_string(),
        terminal: false,
        no_display: false,
        hidden: false,
        categories: vec!["Utility".to_string(), "TextEditor".to_string()],
        keywords: vec!["write".to_string()],
        mime_types: vec!["text/plain".to_string()],
        source: "share-applications".to_string(),
        path: "/usr/share/applications/alpha.desktop".to_string(),
        state: AppState::Installed,
        pinned: true,
    };

    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<appindex_capnp::app_info::Builder<'_>>();
        entry.write_to(&mut builder);
    }
    let mut buffer = Vec::new();
    capnp::serialize::write_message(&mut buffer, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buffer[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<appindex_capnp::app_info::Reader<'_>>()
        .expect("root");
    let read = AppEntry::from_reader(&wire);

    assert_eq!(read.name, entry.name);
    assert_eq!(read.comment, entry.comment);
    assert_eq!(read.icon, entry.icon);
    assert_eq!(read.categories, entry.categories);
    assert_eq!(read.source, entry.source);
    assert_eq!(read.state, AppState::Installed);
    assert!(read.pinned);
}

#[test]
fn the_page_shape_carries_the_total() {
    let page = Page {
        items: vec![AppEntry::default()],
        total: 7,
    };
    assert_eq!(page.total, 7);
    assert_eq!(page.items.len(), 1);
}
