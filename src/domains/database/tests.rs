use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::database_capnp;

use super::availability::{status_counts, AvailStatus, DATABASE_AVAILABILITY, DATABASE_SCHEMA_ID};
use super::backend::DatabaseBackend;
use super::types::TransactionId;

const CONTRACT_METHODS: &[&str] = &[
    "get", "has", "listKeys", "count", "set", "delete", "clear", "setBatch", "getBatch", "deleteBatch", "listNamespaces", "deleteNamespace", "exportNamespace", "importNamespace", "subscribeChanges", "pullChanges", "getTyped", "setTyped", "setWithTtl", "getTtl", "setBatchWithTtl", "getMetadata", "getBatchMetadata", "beginTransaction", "commitTransaction", "rollbackTransaction", "getTransactionStatus", "listKeysByTimeRange", "getTimeSeries", "applySchema", "getSchema", "getQueryLanguages", "getNamespace", "watchKey", "compactNamespace", "getNamespaceStats", "backupNamespace", "restoreNamespace", "getKeyHistory", "encryptKey", "decryptKey", "subscribeChangeStream", "encryptNamespace", "getNamespaceEncryptionStatus", "subscribeAuditLog", "getResourceMonitor", "getConnectionPoolStatus", "query", "createIndex", "listIndexes", "addGraphEdge", "shortestPath", "detectCommunities", "getNeighbors", "insertVector", "searchVector", "deleteVector", "setCrdt", "getCrdt", "mergeCrdt", "startReplication", "getReplicationStatus", "stopReplication", "appendEvent", "replayEvents", "createSnapshot", "readFromSnapshot", "deleteSnapshot", "configureSharding", "getShardInfo", "pull", "cancel", "pull", "cancel", "pull", "cancel"
];

struct HonestBackend;
impl DatabaseBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(DATABASE_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in DATABASE_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn claimed_l3_tests_exist() {
    for (_name, test) in super::availability::claimed_l3_tests() {
        assert!(test.starts_with("l3_linux_database::"));
    }
}

#[test]
fn transaction_id_round_trips() {
    let t = TransactionId { id: 42 };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<database_capnp::transaction_id::Builder<'_>>();
        t.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new()).expect("deserialize");
    let wire = reader.get_root::<database_capnp::transaction_id::Reader<'_>>().expect("root");
    assert_eq!(TransactionId::from_reader(&wire), t);
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

/// The database cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/database.rs");

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
            "{method} claims {test}, which is not a test function in ../../platforms/linux/database.rs"
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
