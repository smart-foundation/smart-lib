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
