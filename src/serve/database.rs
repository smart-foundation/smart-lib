#![allow(missing_docs)]
//! The database domain, served through the contract's generated `Server` trait.
extern crate std;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::rc::Rc;
use std::sync::Arc;
use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;
use crate::core::ErrorCode;
use crate::domains::database::availability;
use crate::domains::database::backend::DatabaseBackend;
use crate::schema::database_capnp::database as db;
use crate::schema::types_capnp;
use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;
use super::availability::matrix_reader;
matrix_reader!(availability);

pub struct DatabaseService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for DatabaseService<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend), class: self.class }
    }
}

impl<B> DatabaseService<B>
where
    B: DatabaseBackend + 'static,
{
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() { return None; }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!("permissionDenied: the {verb} verb changes this host's database state, and this connection holds the read-only class"),
    ))
}

fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a Database contract method"),
        )),
    }
}

fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

impl<B> db::Server for DatabaseService<B>
where
    B: DatabaseBackend + 'static,
{
}

pub struct DatabaseDomain<B> {
    backend: Arc<B>,
}

impl<B> DatabaseDomain<B>
where
    B: DatabaseBackend + 'static,
{
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for DatabaseDomain<B>
where
    B: DatabaseBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        db::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::DATABASE_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "database"
    }

    fn category(&self) -> &'static str {
        "database"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "database get",
                "Database",
                "get",
                "get",
                0,
                true,
            )
            .with_row(matrix_row("get")),            DomainCommand::new(
                "database has",
                "Database",
                "has",
                "has",
                1,
                true,
            )
            .with_row(matrix_row("has")),            DomainCommand::new(
                "database list-keys",
                "Database",
                "listKeys",
                "listKeys",
                2,
                true,
            )
            .with_row(matrix_row("listKeys")),            DomainCommand::new(
                "database count",
                "Database",
                "count",
                "count",
                3,
                true,
            )
            .with_row(matrix_row("count")),            DomainCommand::new(
                "database set",
                "Database",
                "set",
                "set",
                4,
                true,
            )
            .with_row(matrix_row("set")),            DomainCommand::new(
                "database delete",
                "Database",
                "delete",
                "delete",
                5,
                true,
            )
            .with_row(matrix_row("delete")),            DomainCommand::new(
                "database clear",
                "Database",
                "clear",
                "clear",
                6,
                true,
            )
            .with_row(matrix_row("clear")),            DomainCommand::new(
                "database set-batch",
                "Database",
                "setBatch",
                "setBatch",
                7,
                true,
            )
            .with_row(matrix_row("setBatch")),            DomainCommand::new(
                "database get-batch",
                "Database",
                "getBatch",
                "getBatch",
                8,
                true,
            )
            .with_row(matrix_row("getBatch")),            DomainCommand::new(
                "database delete-batch",
                "Database",
                "deleteBatch",
                "deleteBatch",
                9,
                true,
            )
            .with_row(matrix_row("deleteBatch")),            DomainCommand::new(
                "database list-namespaces",
                "Database",
                "listNamespaces",
                "listNamespaces",
                10,
                true,
            )
            .with_row(matrix_row("listNamespaces")),            DomainCommand::new(
                "database delete-namespace",
                "Database",
                "deleteNamespace",
                "deleteNamespace",
                11,
                true,
            )
            .with_row(matrix_row("deleteNamespace")),            DomainCommand::new(
                "database export-namespace",
                "Database",
                "exportNamespace",
                "exportNamespace",
                12,
                true,
            )
            .with_row(matrix_row("exportNamespace")),            DomainCommand::new(
                "database import-namespace",
                "Database",
                "importNamespace",
                "importNamespace",
                13,
                true,
            )
            .with_row(matrix_row("importNamespace")),            DomainCommand::new(
                "database subscribe-changes",
                "Database",
                "subscribeChanges",
                "subscribeChanges",
                14,
                true,
            )
            .with_row(matrix_row("subscribeChanges")),            DomainCommand::new(
                "database pull-changes",
                "Database",
                "pullChanges",
                "pullChanges",
                15,
                true,
            )
            .with_row(matrix_row("pullChanges")),            DomainCommand::new(
                "database get-typed",
                "Database",
                "getTyped",
                "getTyped",
                16,
                true,
            )
            .with_row(matrix_row("getTyped")),            DomainCommand::new(
                "database set-typed",
                "Database",
                "setTyped",
                "setTyped",
                17,
                true,
            )
            .with_row(matrix_row("setTyped")),            DomainCommand::new(
                "database set-with-ttl",
                "Database",
                "setWithTtl",
                "setWithTtl",
                18,
                true,
            )
            .with_row(matrix_row("setWithTtl")),            DomainCommand::new(
                "database get-ttl",
                "Database",
                "getTtl",
                "getTtl",
                19,
                true,
            )
            .with_row(matrix_row("getTtl")),            DomainCommand::new(
                "database set-batch-with-ttl",
                "Database",
                "setBatchWithTtl",
                "setBatchWithTtl",
                20,
                true,
            )
            .with_row(matrix_row("setBatchWithTtl")),            DomainCommand::new(
                "database get-metadata",
                "Database",
                "getMetadata",
                "getMetadata",
                21,
                true,
            )
            .with_row(matrix_row("getMetadata")),            DomainCommand::new(
                "database get-batch-metadata",
                "Database",
                "getBatchMetadata",
                "getBatchMetadata",
                22,
                true,
            )
            .with_row(matrix_row("getBatchMetadata")),            DomainCommand::new(
                "database begin-transaction",
                "Database",
                "beginTransaction",
                "beginTransaction",
                23,
                true,
            )
            .with_row(matrix_row("beginTransaction")),            DomainCommand::new(
                "database commit-transaction",
                "Database",
                "commitTransaction",
                "commitTransaction",
                24,
                true,
            )
            .with_row(matrix_row("commitTransaction")),            DomainCommand::new(
                "database rollback-transaction",
                "Database",
                "rollbackTransaction",
                "rollbackTransaction",
                25,
                true,
            )
            .with_row(matrix_row("rollbackTransaction")),            DomainCommand::new(
                "database get-transaction-status",
                "Database",
                "getTransactionStatus",
                "getTransactionStatus",
                26,
                true,
            )
            .with_row(matrix_row("getTransactionStatus")),            DomainCommand::new(
                "database list-keys-by-time-range",
                "Database",
                "listKeysByTimeRange",
                "listKeysByTimeRange",
                27,
                true,
            )
            .with_row(matrix_row("listKeysByTimeRange")),            DomainCommand::new(
                "database get-time-series",
                "Database",
                "getTimeSeries",
                "getTimeSeries",
                28,
                true,
            )
            .with_row(matrix_row("getTimeSeries")),            DomainCommand::new(
                "database apply-schema",
                "Database",
                "applySchema",
                "applySchema",
                29,
                true,
            )
            .with_row(matrix_row("applySchema")),            DomainCommand::new(
                "database get-schema",
                "Database",
                "getSchema",
                "getSchema",
                30,
                true,
            )
            .with_row(matrix_row("getSchema")),            DomainCommand::new(
                "database get-query-languages",
                "Database",
                "getQueryLanguages",
                "getQueryLanguages",
                31,
                true,
            )
            .with_row(matrix_row("getQueryLanguages")),            DomainCommand::new(
                "database get-namespace",
                "Database",
                "getNamespace",
                "getNamespace",
                32,
                true,
            )
            .with_row(matrix_row("getNamespace")),            DomainCommand::new(
                "database watch-key",
                "Database",
                "watchKey",
                "watchKey",
                33,
                true,
            )
            .with_row(matrix_row("watchKey")),            DomainCommand::new(
                "database compact-namespace",
                "Database",
                "compactNamespace",
                "compactNamespace",
                34,
                true,
            )
            .with_row(matrix_row("compactNamespace")),            DomainCommand::new(
                "database get-namespace-stats",
                "Database",
                "getNamespaceStats",
                "getNamespaceStats",
                35,
                true,
            )
            .with_row(matrix_row("getNamespaceStats")),            DomainCommand::new(
                "database backup-namespace",
                "Database",
                "backupNamespace",
                "backupNamespace",
                36,
                true,
            )
            .with_row(matrix_row("backupNamespace")),            DomainCommand::new(
                "database restore-namespace",
                "Database",
                "restoreNamespace",
                "restoreNamespace",
                37,
                true,
            )
            .with_row(matrix_row("restoreNamespace")),            DomainCommand::new(
                "database get-key-history",
                "Database",
                "getKeyHistory",
                "getKeyHistory",
                38,
                true,
            )
            .with_row(matrix_row("getKeyHistory")),            DomainCommand::new(
                "database encrypt-key",
                "Database",
                "encryptKey",
                "encryptKey",
                39,
                true,
            )
            .with_row(matrix_row("encryptKey")),            DomainCommand::new(
                "database decrypt-key",
                "Database",
                "decryptKey",
                "decryptKey",
                40,
                true,
            )
            .with_row(matrix_row("decryptKey")),            DomainCommand::new(
                "database subscribe-change-stream",
                "Database",
                "subscribeChangeStream",
                "subscribeChangeStream",
                41,
                true,
            )
            .with_row(matrix_row("subscribeChangeStream")),            DomainCommand::new(
                "database encrypt-namespace",
                "Database",
                "encryptNamespace",
                "encryptNamespace",
                42,
                true,
            )
            .with_row(matrix_row("encryptNamespace")),            DomainCommand::new(
                "database get-namespace-encryption-status",
                "Database",
                "getNamespaceEncryptionStatus",
                "getNamespaceEncryptionStatus",
                43,
                true,
            )
            .with_row(matrix_row("getNamespaceEncryptionStatus")),            DomainCommand::new(
                "database subscribe-audit-log",
                "Database",
                "subscribeAuditLog",
                "subscribeAuditLog",
                44,
                true,
            )
            .with_row(matrix_row("subscribeAuditLog")),            DomainCommand::new(
                "database get-resource-monitor",
                "Database",
                "getResourceMonitor",
                "getResourceMonitor",
                45,
                true,
            )
            .with_row(matrix_row("getResourceMonitor")),            DomainCommand::new(
                "database get-connection-pool-status",
                "Database",
                "getConnectionPoolStatus",
                "getConnectionPoolStatus",
                46,
                true,
            )
            .with_row(matrix_row("getConnectionPoolStatus")),            DomainCommand::new(
                "database query",
                "Database",
                "query",
                "query",
                47,
                true,
            )
            .with_row(matrix_row("query")),            DomainCommand::new(
                "database create-index",
                "Database",
                "createIndex",
                "createIndex",
                48,
                true,
            )
            .with_row(matrix_row("createIndex")),            DomainCommand::new(
                "database list-indexes",
                "Database",
                "listIndexes",
                "listIndexes",
                49,
                true,
            )
            .with_row(matrix_row("listIndexes")),            DomainCommand::new(
                "database add-graph-edge",
                "Database",
                "addGraphEdge",
                "addGraphEdge",
                50,
                true,
            )
            .with_row(matrix_row("addGraphEdge")),            DomainCommand::new(
                "database shortest-path",
                "Database",
                "shortestPath",
                "shortestPath",
                51,
                true,
            )
            .with_row(matrix_row("shortestPath")),            DomainCommand::new(
                "database detect-communities",
                "Database",
                "detectCommunities",
                "detectCommunities",
                52,
                true,
            )
            .with_row(matrix_row("detectCommunities")),            DomainCommand::new(
                "database get-neighbors",
                "Database",
                "getNeighbors",
                "getNeighbors",
                53,
                true,
            )
            .with_row(matrix_row("getNeighbors")),            DomainCommand::new(
                "database insert-vector",
                "Database",
                "insertVector",
                "insertVector",
                54,
                true,
            )
            .with_row(matrix_row("insertVector")),            DomainCommand::new(
                "database search-vector",
                "Database",
                "searchVector",
                "searchVector",
                55,
                true,
            )
            .with_row(matrix_row("searchVector")),            DomainCommand::new(
                "database delete-vector",
                "Database",
                "deleteVector",
                "deleteVector",
                56,
                true,
            )
            .with_row(matrix_row("deleteVector")),            DomainCommand::new(
                "database set-crdt",
                "Database",
                "setCrdt",
                "setCrdt",
                57,
                true,
            )
            .with_row(matrix_row("setCrdt")),            DomainCommand::new(
                "database get-crdt",
                "Database",
                "getCrdt",
                "getCrdt",
                58,
                true,
            )
            .with_row(matrix_row("getCrdt")),            DomainCommand::new(
                "database merge-crdt",
                "Database",
                "mergeCrdt",
                "mergeCrdt",
                59,
                true,
            )
            .with_row(matrix_row("mergeCrdt")),            DomainCommand::new(
                "database start-replication",
                "Database",
                "startReplication",
                "startReplication",
                60,
                true,
            )
            .with_row(matrix_row("startReplication")),            DomainCommand::new(
                "database get-replication-status",
                "Database",
                "getReplicationStatus",
                "getReplicationStatus",
                61,
                true,
            )
            .with_row(matrix_row("getReplicationStatus")),            DomainCommand::new(
                "database stop-replication",
                "Database",
                "stopReplication",
                "stopReplication",
                62,
                true,
            )
            .with_row(matrix_row("stopReplication")),            DomainCommand::new(
                "database append-event",
                "Database",
                "appendEvent",
                "appendEvent",
                63,
                true,
            )
            .with_row(matrix_row("appendEvent")),            DomainCommand::new(
                "database replay-events",
                "Database",
                "replayEvents",
                "replayEvents",
                64,
                true,
            )
            .with_row(matrix_row("replayEvents")),            DomainCommand::new(
                "database create-snapshot",
                "Database",
                "createSnapshot",
                "createSnapshot",
                65,
                true,
            )
            .with_row(matrix_row("createSnapshot")),            DomainCommand::new(
                "database read-from-snapshot",
                "Database",
                "readFromSnapshot",
                "readFromSnapshot",
                66,
                true,
            )
            .with_row(matrix_row("readFromSnapshot")),            DomainCommand::new(
                "database delete-snapshot",
                "Database",
                "deleteSnapshot",
                "deleteSnapshot",
                67,
                true,
            )
            .with_row(matrix_row("deleteSnapshot")),            DomainCommand::new(
                "database configure-sharding",
                "Database",
                "configureSharding",
                "configureSharding",
                68,
                true,
            )
            .with_row(matrix_row("configureSharding")),            DomainCommand::new(
                "database get-shard-info",
                "Database",
                "getShardInfo",
                "getShardInfo",
                69,
                true,
            )
            .with_row(matrix_row("getShardInfo")),            DomainCommand::new(
                "database pull",
                "Database",
                "pull",
                "pull",
                70,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "database cancel",
                "Database",
                "cancel",
                "cancel",
                71,
                true,
            )
            .with_row(matrix_row("cancel")),            DomainCommand::new(
                "database pull",
                "Database",
                "pull",
                "pull",
                72,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "database cancel",
                "Database",
                "cancel",
                "cancel",
                73,
                true,
            )
            .with_row(matrix_row("cancel")),            DomainCommand::new(
                "database pull",
                "Database",
                "pull",
                "pull",
                74,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "database cancel",
                "Database",
                "cancel",
                "cancel",
                75,
                true,
            )
            .with_row(matrix_row("cancel")),        ]
    }

    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/database.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = DatabaseService::new(Arc::clone(&self.backend), class);
        let client: db::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
