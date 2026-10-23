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
    fn get(
        self: Rc<Self>,
        _params: db::GetParams,
        _results: db::GetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("get") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn has(
        self: Rc<Self>,
        _params: db::HasParams,
        _results: db::HasResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("has") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_keys(
        self: Rc<Self>,
        _params: db::ListKeysParams,
        _results: db::ListKeysResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listKeys") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn count(
        self: Rc<Self>,
        _params: db::CountParams,
        _results: db::CountResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("count") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set(
        self: Rc<Self>,
        _params: db::SetParams,
        _results: db::SetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("set") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn delete(
        self: Rc<Self>,
        _params: db::DeleteParams,
        _results: db::DeleteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("delete") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn clear(
        self: Rc<Self>,
        _params: db::ClearParams,
        _results: db::ClearResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("clear") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_batch(
        self: Rc<Self>,
        _params: db::SetBatchParams,
        _results: db::SetBatchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setBatch") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_batch(
        self: Rc<Self>,
        _params: db::GetBatchParams,
        _results: db::GetBatchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getBatch") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn delete_batch(
        self: Rc<Self>,
        _params: db::DeleteBatchParams,
        _results: db::DeleteBatchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("deleteBatch") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_namespaces(
        self: Rc<Self>,
        _params: db::ListNamespacesParams,
        _results: db::ListNamespacesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listNamespaces") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn delete_namespace(
        self: Rc<Self>,
        _params: db::DeleteNamespaceParams,
        _results: db::DeleteNamespaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("deleteNamespace") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn export_namespace(
        self: Rc<Self>,
        _params: db::ExportNamespaceParams,
        _results: db::ExportNamespaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("exportNamespace") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn import_namespace(
        self: Rc<Self>,
        _params: db::ImportNamespaceParams,
        _results: db::ImportNamespaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("importNamespace") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_changes(
        self: Rc<Self>,
        _params: db::SubscribeChangesParams,
        _results: db::SubscribeChangesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeChanges") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn pull_changes(
        self: Rc<Self>,
        _params: db::PullChangesParams,
        _results: db::PullChangesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("pullChanges") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_typed(
        self: Rc<Self>,
        _params: db::GetTypedParams,
        _results: db::GetTypedResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getTyped") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_typed(
        self: Rc<Self>,
        _params: db::SetTypedParams,
        _results: db::SetTypedResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setTyped") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_with_ttl(
        self: Rc<Self>,
        _params: db::SetWithTtlParams,
        _results: db::SetWithTtlResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setWithTtl") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_ttl(
        self: Rc<Self>,
        _params: db::GetTtlParams,
        _results: db::GetTtlResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getTtl") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_batch_with_ttl(
        self: Rc<Self>,
        _params: db::SetBatchWithTtlParams,
        _results: db::SetBatchWithTtlResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setBatchWithTtl") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_metadata(
        self: Rc<Self>,
        _params: db::GetMetadataParams,
        _results: db::GetMetadataResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getMetadata") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_batch_metadata(
        self: Rc<Self>,
        _params: db::GetBatchMetadataParams,
        _results: db::GetBatchMetadataResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getBatchMetadata") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn begin_transaction(
        self: Rc<Self>,
        _params: db::BeginTransactionParams,
        _results: db::BeginTransactionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("beginTransaction") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn commit_transaction(
        self: Rc<Self>,
        _params: db::CommitTransactionParams,
        _results: db::CommitTransactionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("commitTransaction") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn rollback_transaction(
        self: Rc<Self>,
        _params: db::RollbackTransactionParams,
        _results: db::RollbackTransactionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("rollbackTransaction") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_transaction_status(
        self: Rc<Self>,
        _params: db::GetTransactionStatusParams,
        _results: db::GetTransactionStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getTransactionStatus") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_keys_by_time_range(
        self: Rc<Self>,
        _params: db::ListKeysByTimeRangeParams,
        _results: db::ListKeysByTimeRangeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listKeysByTimeRange") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_time_series(
        self: Rc<Self>,
        _params: db::GetTimeSeriesParams,
        _results: db::GetTimeSeriesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getTimeSeries") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn apply_schema(
        self: Rc<Self>,
        _params: db::ApplySchemaParams,
        _results: db::ApplySchemaResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("applySchema") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_schema(
        self: Rc<Self>,
        _params: db::GetSchemaParams,
        _results: db::GetSchemaResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getSchema") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_query_languages(
        self: Rc<Self>,
        _params: db::GetQueryLanguagesParams,
        _results: db::GetQueryLanguagesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getQueryLanguages") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_namespace(
        self: Rc<Self>,
        _params: db::GetNamespaceParams,
        _results: db::GetNamespaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getNamespace") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn watch_key(
        self: Rc<Self>,
        _params: db::WatchKeyParams,
        _results: db::WatchKeyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("watchKey") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn compact_namespace(
        self: Rc<Self>,
        _params: db::CompactNamespaceParams,
        _results: db::CompactNamespaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("compactNamespace") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_namespace_stats(
        self: Rc<Self>,
        _params: db::GetNamespaceStatsParams,
        _results: db::GetNamespaceStatsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getNamespaceStats") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn backup_namespace(
        self: Rc<Self>,
        _params: db::BackupNamespaceParams,
        _results: db::BackupNamespaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("backupNamespace") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn restore_namespace(
        self: Rc<Self>,
        _params: db::RestoreNamespaceParams,
        _results: db::RestoreNamespaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("restoreNamespace") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_key_history(
        self: Rc<Self>,
        _params: db::GetKeyHistoryParams,
        _results: db::GetKeyHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getKeyHistory") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn encrypt_key(
        self: Rc<Self>,
        _params: db::EncryptKeyParams,
        _results: db::EncryptKeyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("encryptKey") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn decrypt_key(
        self: Rc<Self>,
        _params: db::DecryptKeyParams,
        _results: db::DecryptKeyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("decryptKey") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_change_stream(
        self: Rc<Self>,
        _params: db::SubscribeChangeStreamParams,
        _results: db::SubscribeChangeStreamResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeChangeStream") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn encrypt_namespace(
        self: Rc<Self>,
        _params: db::EncryptNamespaceParams,
        _results: db::EncryptNamespaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("encryptNamespace") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_namespace_encryption_status(
        self: Rc<Self>,
        _params: db::GetNamespaceEncryptionStatusParams,
        _results: db::GetNamespaceEncryptionStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getNamespaceEncryptionStatus") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_audit_log(
        self: Rc<Self>,
        _params: db::SubscribeAuditLogParams,
        _results: db::SubscribeAuditLogResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeAuditLog") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_resource_monitor(
        self: Rc<Self>,
        _params: db::GetResourceMonitorParams,
        _results: db::GetResourceMonitorResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getResourceMonitor") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_connection_pool_status(
        self: Rc<Self>,
        _params: db::GetConnectionPoolStatusParams,
        _results: db::GetConnectionPoolStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getConnectionPoolStatus") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn query(
        self: Rc<Self>,
        _params: db::QueryParams,
        _results: db::QueryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("query") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn create_index(
        self: Rc<Self>,
        _params: db::CreateIndexParams,
        _results: db::CreateIndexResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("createIndex") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_indexes(
        self: Rc<Self>,
        _params: db::ListIndexesParams,
        _results: db::ListIndexesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listIndexes") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn add_graph_edge(
        self: Rc<Self>,
        _params: db::AddGraphEdgeParams,
        _results: db::AddGraphEdgeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("addGraphEdge") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn shortest_path(
        self: Rc<Self>,
        _params: db::ShortestPathParams,
        _results: db::ShortestPathResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("shortestPath") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn detect_communities(
        self: Rc<Self>,
        _params: db::DetectCommunitiesParams,
        _results: db::DetectCommunitiesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("detectCommunities") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_neighbors(
        self: Rc<Self>,
        _params: db::GetNeighborsParams,
        _results: db::GetNeighborsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getNeighbors") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn insert_vector(
        self: Rc<Self>,
        _params: db::InsertVectorParams,
        _results: db::InsertVectorResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("insertVector") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn search_vector(
        self: Rc<Self>,
        _params: db::SearchVectorParams,
        _results: db::SearchVectorResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("searchVector") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn delete_vector(
        self: Rc<Self>,
        _params: db::DeleteVectorParams,
        _results: db::DeleteVectorResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("deleteVector") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_crdt(
        self: Rc<Self>,
        _params: db::SetCrdtParams,
        _results: db::SetCrdtResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setCrdt") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_crdt(
        self: Rc<Self>,
        _params: db::GetCrdtParams,
        _results: db::GetCrdtResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getCrdt") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn merge_crdt(
        self: Rc<Self>,
        _params: db::MergeCrdtParams,
        _results: db::MergeCrdtResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("mergeCrdt") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn start_replication(
        self: Rc<Self>,
        _params: db::StartReplicationParams,
        _results: db::StartReplicationResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("startReplication") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_replication_status(
        self: Rc<Self>,
        _params: db::GetReplicationStatusParams,
        _results: db::GetReplicationStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getReplicationStatus") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn stop_replication(
        self: Rc<Self>,
        _params: db::StopReplicationParams,
        _results: db::StopReplicationResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("stopReplication") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn append_event(
        self: Rc<Self>,
        _params: db::AppendEventParams,
        _results: db::AppendEventResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("appendEvent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn replay_events(
        self: Rc<Self>,
        _params: db::ReplayEventsParams,
        _results: db::ReplayEventsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("replayEvents") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn create_snapshot(
        self: Rc<Self>,
        _params: db::CreateSnapshotParams,
        _results: db::CreateSnapshotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("createSnapshot") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn read_from_snapshot(
        self: Rc<Self>,
        _params: db::ReadFromSnapshotParams,
        _results: db::ReadFromSnapshotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("readFromSnapshot") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn delete_snapshot(
        self: Rc<Self>,
        _params: db::DeleteSnapshotParams,
        _results: db::DeleteSnapshotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("deleteSnapshot") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn configure_sharding(
        self: Rc<Self>,
        _params: db::ConfigureShardingParams,
        _results: db::ConfigureShardingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("configureSharding") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_shard_info(
        self: Rc<Self>,
        _params: db::GetShardInfoParams,
        _results: db::GetShardInfoResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getShardInfo") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }
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
