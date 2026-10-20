//! The files domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `files.capnp`'s `interface Files` block (`@0`–`@73`), its
//! `FileService` interface (`@0`–`@4`), its `FileEventSubscription`
//! interface (`@0`–`@1`) and its `FileAuditSubscription` interface
//! (`@0`–`@1`). The dispatch is the generated `ServerDispatch`; there is no
//! ordinal, no method table and no envelope of this layer's own anywhere on
//! the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached (law #5).
//!
//! # The capability the contract mints
//!
//! `acquireFile @73` hands back a `FileService` reference bound to ONE path.
//! Holding that reference IS the authority to read and write that path —
//! the contract's own rule, and the reason the returned service is bound:
//! every `FileService` method goes through the engine's `bound_*` family,
//! which refuses a path that is not the one the capability was acquired for.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use crate::core::ids::{FileHandleId, KeyId};
use crate::core::values::{Duration, Timestamp};
use crate::core::ErrorCode;
use crate::domains::files::availability;
use crate::domains::files::backend::{AuditSubscription, FilesBackend, WatchSubscription};
use crate::domains::files::types::{
    AuditAction, BatchCopyRequest, BatchDeleteRequest, BatchMoveRequest, BatchResult, BoundPath,
    ChecksumResult, ComparisonResult, ContentAddressedRef, ContentRefResult, ContentSearchQuery,
    ContentSearchResult, CreateEncryptedRequest, DirectoryEntry, DirectoryFilter, DirectoryStats,
    DiskUsage, EncryptionInfoResult, EphemeralFileConfig, ExtendedMetadata, FileActivity,
    FileAuditEntry, FileAuditFilter, FileBranch, FileCompressionConfig, FileDeduplicationInfo,
    FileEvent, FileHashResult, FileInfo, FileInfoResult, FileLock, FileSearchQuery,
    FileSearchResult, FileVersion, GetByHashRequest, LifecyclePolicy,
    LifecyclePolicyResult, LockFileRequest, Metadata, OpenRequest, Page, Pagination, RecentFile,
    RetentionAction, SpecialPath, TextResult, TrashEntry, VersionResult,
};
use crate::schema::files_capnp::files as fs;
use crate::schema::files_capnp::file_service as fsvc;
use crate::schema::files_capnp::file_event_subscription as evsub;
use crate::schema::files_capnp::file_audit_subscription as ausub;
use crate::schema::types_capnp;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

use super::availability::matrix_reader;

matrix_reader!(availability);

/// The files service: the domain's backend, and the class this connection
/// earned.
pub struct FilesService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for FilesService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> FilesService<B>
where
    B: FilesBackend + 'static,
{
    /// A service over this backend, at this class.
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }

    /// The class's answer for a mutating verb on a connection that did not
    /// earn the right to change this host's files (law #5). The verb is
    /// refused *before* the backend is reached: a change that is refused
    /// afterwards has already happened.
    fn gate(&self, verb: &str) -> Option<capnp::Error> {
        if self.class.is_control() {
            return None;
        }
        Some(errors::decline(
            ErrorCode::PermissionDenied,
            &format!(
                "permissionDenied: the {verb} verb changes this host's files, and this \
                 connection holds the read-only class"
            ),
        ))
    }
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
///
/// `None` when the matrix says the method IS served here — the arm then
/// goes to the backend. The answer comes BEFORE the class's gate: what the
/// host cannot do, no class can do.
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
            &format!("invalidArgument: {name} is not a Files contract method"),
        )),
    }
}
/// Write the engine's `VoidResult` into the contract's `T.VoidResult`.
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

/// The contract's pagination as the domain's.
fn pagination(reader: &types_capnp::pagination::Reader<'_>) -> Pagination {
    Pagination {
        offset: reader.get_offset(),
        limit: reader.get_limit(),
    }
}

/// The contract's duration as the domain's.
fn duration(reader: &types_capnp::duration::Reader<'_>) -> Duration {
    Duration::from_nanos(reader.get_nanoseconds())
}

/// The contract's pagination, read from a getter that may fail.
fn pagination_result(reader: capnp::Result<types_capnp::pagination::Reader<'_>>) -> Pagination {
    match reader {
        Ok(page) => pagination(&page),
        Err(_) => Pagination::default(),
    }
}

/// The contract's duration, read from a getter that may fail.
fn duration_result(reader: capnp::Result<types_capnp::duration::Reader<'_>>) -> Duration {
    match reader {
        Ok(value) => duration(&value),
        Err(_) => Duration::default(),
    }
}

/// Read a `Text` field that may be absent, into an owned string.
fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => super::text_of(text),
        Err(_) => String::new(),
    }
}

/// Write the engine's metadata map into the contract's `T.StringMap`.
fn write_metadata(metadata: &Metadata, builder: &mut types_capnp::string_map::Builder<'_>) {
    let mut list = builder.reborrow().init_entries(metadata.len() as u32);
    for (index, (key, value)) in metadata.iter().enumerate() {
        let mut entry = list.reborrow().get(index as u32);
        entry.set_key(key);
        entry.set_value(value);
    }
}

/// Read the contract's `T.StringMap` into the engine's metadata map.
fn read_metadata(reader: &types_capnp::string_map::Reader<'_>) -> Metadata {
    let mut metadata = Vec::new();
    if let Ok(entries) = reader.get_entries() {
        for entry in entries.iter() {
            metadata.push((text_field(entry.get_key()), text_field(entry.get_value())));
        }
    }
    metadata
}

/// Write the engine's hash bytes into the contract's `T.HashOutput`.
///
/// The contract's `HashOutput.algorithm` is an ORDINAL whose vocabulary no
/// schema in the tree declares, so the engine writes 0 and carries the
/// algorithm's own name in the surrounding metadata wherever the struct has
/// one. The bytes are the engine's own digest, never a placeholder.
fn write_hash(bytes: &[u8], builder: &mut types_capnp::hash_output::Builder<'_>) {
    builder.set_algorithm(0);
    builder.set_bytes(bytes);
}

/// Write the engine's mime token into the contract's `T.MimeType`.
///
/// The engine's token is the full "type/subtype" string; the contract splits
/// it, so the split is the engine's own text, split at its own slash.
fn write_mime(token: &str, builder: &mut types_capnp::mime_type::Builder<'_>) {
    match token.split_once('/') {
        Some((kind, subtype)) => {
            builder.set_type(kind);
            builder.set_subtype(subtype);
        }
        None => {
            builder.set_type(token);
            builder.set_subtype("");
        }
    }
}

/// The contract's `T.MimeType` as the engine's mime token.
fn read_mime(reader: &types_capnp::mime_type::Reader<'_>) -> String {
    let kind = text_field(reader.get_type());
    let subtype = text_field(reader.get_subtype());
    if subtype.is_empty() {
        kind
    } else {
        format!("{kind}/{subtype}")
    }
}

/// The contract's `T.DataHandle`, read from a getter that may fail.
fn read_data_result(handle: capnp::Result<types_capnp::data_handle::Reader<'_>>) -> Vec<u8> {
    match handle {
        Ok(handle) => read_data(&handle),
        Err(_) => Vec::new(),
    }
}

/// Read the contract's `T.DataHandle` inline arm.
fn read_data(handle: &types_capnp::data_handle::Reader<'_>) -> Vec<u8> {
    use types_capnp::data_handle::WhichReader;
    match handle.which() {
        Ok(WhichReader::Inline(bytes)) => bytes.map(|b| b.to_vec()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The engine's file handle as the contract's `FileHandle`.
fn write_handle(
    handle: &crate::domains::files::types::FileHandle,
    builder: &mut crate::schema::files_capnp::file_handle::Builder<'_>,
) {
    builder.reborrow().init_handle().set_id(handle.id.get());
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&handle.metadata, &mut metadata);
}

/// The contract's `FileHandle` as the engine's handle.
fn read_handle(reader: &crate::schema::files_capnp::file_handle::Reader<'_>) -> crate::domains::files::types::FileHandle {
    crate::domains::files::types::FileHandle {
        id: FileHandleId::new(reader.get_handle().map(|h| h.get_id()).unwrap_or(0)),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// The contract's `FileSearchQuery` as the engine's query.
fn read_search_query(
    reader: &crate::schema::files_capnp::file_search_query::Reader<'_>,
) -> FileSearchQuery {
    FileSearchQuery {
        path: text_field(reader.get_path()),
        pattern: text_field(reader.get_pattern()),
        recursive: reader.get_recursive(),
        page: pagination_result(reader.get_page()),
        annotations: reader
            .get_annotations()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Write the engine's file info into the contract's `FileInfo`.
fn write_file_info(
    info: &FileInfo,
    builder: &mut crate::schema::files_capnp::file_info::Builder<'_>,
) {
    builder.set_path(&info.path);
    builder.set_type(info.kind.to_generated());
    builder.set_size(info.size);
    builder.reborrow().init_created().set_nanoseconds(info.created.as_nanos());
    builder
        .reborrow()
        .init_modified()
        .set_nanoseconds(info.modified.as_nanos());
    builder
        .reborrow()
        .init_accessed()
        .set_nanoseconds(info.accessed.as_nanos());
    write_mime(&info.mime_type, &mut builder.reborrow().init_mime_type());
    write_hash(&info.content_hash, &mut builder.reborrow().init_content_hash());
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&info.metadata, &mut metadata);
}

/// Write the engine's extended metadata into the contract's struct.
fn write_extended_metadata(
    info: &ExtendedMetadata,
    builder: &mut crate::schema::files_capnp::extended_metadata::Builder<'_>,
) {
    builder.set_path(&info.path);
    builder.set_name(&info.name);
    builder.set_type(info.kind.to_generated());
    builder.set_size(info.size);
    builder.reborrow().init_created().set_nanoseconds(info.created.as_nanos());
    builder
        .reborrow()
        .init_modified()
        .set_nanoseconds(info.modified.as_nanos());
    builder
        .reborrow()
        .init_accessed()
        .set_nanoseconds(info.accessed.as_nanos());
    builder.set_owner(&info.owner);
    write_mime(&info.mime_type, &mut builder.reborrow().init_mime_type());
    builder.set_target(&info.target);
    builder.set_is_hidden(info.is_hidden);
    builder.set_is_read_only(info.is_read_only);
    builder.set_is_executable(info.is_executable);
    write_hash(
        &info.content_hash,
        &mut builder.reborrow().init_content_hash(),
    );
    builder.set_compressed_size(info.compressed_size);
    builder
        .reborrow()
        .init_encryption_key()
        .set_id(info.encryption_key.get());
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&info.metadata, &mut metadata);
}

/// Write the engine's comparison into the contract's struct.
fn write_comparison(
    comparison: &ComparisonResult,
    builder: &mut crate::schema::files_capnp::comparison_result::Builder<'_>,
) {
    builder.set_identical(comparison.identical);
    builder.reborrow().init_diff().set_inline(&comparison.diff);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&comparison.metadata, &mut metadata);
}

/// Write the engine's lock into the contract's `FileLock`.
fn write_lock(
    lock: &FileLock,
    builder: &mut crate::schema::files_capnp::file_lock::Builder<'_>,
) {
    builder.set_lock_id(lock.lock_id);
    write_handle(&lock.handle, &mut builder.reborrow().init_handle());
    builder.set_lock_type(lock.kind.to_generated());
    builder.set_owner(&lock.owner);
    builder
        .reborrow()
        .init_acquired_at()
        .set_nanoseconds(lock.acquired_at.as_nanos());
    builder
        .reborrow()
        .init_expires_at()
        .set_nanoseconds(lock.expires_at.as_nanos());
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&lock.metadata, &mut metadata);
}

/// The contract's `LockFileRequest` as the engine's request.
fn read_lock_request(
    reader: &crate::schema::files_capnp::lock_file_request::Reader<'_>,
) -> LockFileRequest {
    LockFileRequest {
        path: text_field(reader.get_path()),
        kind: crate::domains::files::types::LockKind::from_generated(
            reader
                .get_lock_type()
                .unwrap_or(crate::schema::files_capnp::FileLockType::Unknown),
        ),
        owner: text_field(reader.get_owner()),
        timeout: duration_result(reader.get_timeout()),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Write the engine's version into the contract's `FileVersion`.
fn write_version(
    version: &FileVersion,
    builder: &mut crate::schema::files_capnp::file_version::Builder<'_>,
) {
    builder.set_version_id(version.version_id);
    write_hash(
        &version.content_hash,
        &mut builder.reborrow().init_content_hash(),
    );
    builder
        .reborrow()
        .init_created_at()
        .set_nanoseconds(version.created_at.as_nanos());
    builder.reborrow().init_author().set_id(version.author.get());
    builder.set_message(&version.message);
    builder.set_size(version.size);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&version.metadata, &mut metadata);
}

/// Write the engine's branch into the contract's `FileBranch`.
fn write_branch(
    branch: &FileBranch,
    builder: &mut crate::schema::files_capnp::file_branch::Builder<'_>,
) {
    builder.set_branch_id(&branch.branch_id);
    builder.set_branch_name(&branch.branch_name);
    builder.set_path(&branch.path);
    builder.set_base_version(branch.base_version);
    builder
        .reborrow()
        .init_created_at()
        .set_nanoseconds(branch.created_at.as_nanos());
    builder.set_author(&branch.author);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&branch.metadata, &mut metadata);
}

/// Write the engine's content reference into the contract's struct.
fn write_content_ref(
    reference: &ContentAddressedRef,
    builder: &mut crate::schema::files_capnp::content_addressed_ref::Builder<'_>,
) {
    write_hash(&reference.hash, &mut builder.reborrow().init_hash());
    builder.set_algorithm(&reference.algorithm);
    builder.set_size_bytes(reference.size_bytes);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&reference.metadata, &mut metadata);
}

/// The contract's `GetByHashRequest` as the engine's request.
fn read_by_hash_request(
    reader: &crate::schema::files_capnp::get_by_hash_request::Reader<'_>,
) -> GetByHashRequest {
    let hash = reader
        .get_hash()
        .map(|h| h.get_bytes().map(|b| b.to_vec()).unwrap_or_default())
        .unwrap_or_default();
    GetByHashRequest {
        hash,
        algorithm: text_field(reader.get_algorithm()),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// The contract's `EphemeralFileConfig` as the engine's config.
fn read_ephemeral_config(
    reader: &crate::schema::files_capnp::ephemeral_file_config::Reader<'_>,
) -> EphemeralFileConfig {
    EphemeralFileConfig {
        delete_after_read: reader.get_delete_after_read(),
        max_age: duration_result(reader.get_max_age()),
        max_reads: reader.get_max_reads(),
    }
}

/// The contract's `CreateEncryptedRequest` as the engine's request.
fn read_encrypted_request(
    reader: &crate::schema::files_capnp::create_encrypted_request::Reader<'_>,
) -> CreateEncryptedRequest {
    CreateEncryptedRequest {
        path: text_field(reader.get_path()),
        content: read_data_result(reader.get_content()),
        key_id: KeyId::new(
            reader
                .get_key_id()
                .map(|k| k.get_id())
                .unwrap_or(0),
        ),
        algorithm: text_field(reader.get_algorithm()),
        parents: reader.get_parents(),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Write the engine's compression config into the contract's struct.
fn write_compression(
    config: &FileCompressionConfig,
    builder: &mut crate::schema::files_capnp::file_compression_config::Builder<'_>,
) {
    builder.set_algorithm(&config.algorithm);
    builder.set_level(config.level);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&config.metadata, &mut metadata);
}

/// The contract's `FileCompressionConfig` as the engine's config.
fn read_compression(
    reader: &crate::schema::files_capnp::file_compression_config::Reader<'_>,
) -> FileCompressionConfig {
    FileCompressionConfig {
        algorithm: text_field(reader.get_algorithm()),
        level: reader.get_level(),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Write the engine's deduplication info into the contract's struct.
fn write_dedup_info(
    info: &FileDeduplicationInfo,
    builder: &mut crate::schema::files_capnp::file_deduplication_info::Builder<'_>,
) {
    write_hash(&info.content_hash, &mut builder.reborrow().init_content_hash());
    builder.set_reference_count(info.reference_count);
    builder.set_saved_bytes(info.saved_bytes);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&info.metadata, &mut metadata);
}

/// Write the engine's batch result into the contract's struct.
fn write_batch_result(
    result: &BatchResult,
    builder: &mut crate::schema::files_capnp::batch_result::Builder<'_>,
) {
    builder.set_completed(result.completed);
    builder.set_failed(result.failed);
    let mut list = builder.reborrow().init_errors(result.errors.len() as u32);
    for (index, error) in result.errors.iter().enumerate() {
        let mut item = list.reborrow().get(index as u32);
        errors::write_t_error(error, &mut item);
    }
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&result.metadata, &mut metadata);
}

/// The contract's `BatchCopyRequest` as the engine's request.
fn read_batch_copy(
    reader: &crate::schema::files_capnp::batch_copy_request::Reader<'_>,
) -> BatchCopyRequest {
    BatchCopyRequest {
        sources: read_text_list(reader.get_sources()),
        destination: text_field(reader.get_destination()),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// The contract's `BatchMoveRequest` as the engine's request.
fn read_batch_move(
    reader: &crate::schema::files_capnp::batch_move_request::Reader<'_>,
) -> BatchMoveRequest {
    BatchMoveRequest {
        sources: read_text_list(reader.get_sources()),
        destination: text_field(reader.get_destination()),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// The contract's `BatchDeleteRequest` as the engine's request.
fn read_batch_delete(
    reader: &crate::schema::files_capnp::batch_delete_request::Reader<'_>,
) -> BatchDeleteRequest {
    BatchDeleteRequest {
        paths: read_text_list(reader.get_paths()),
        recursive: reader.get_recursive(),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Read a list of `Text` into owned strings.
fn read_text_list(list: capnp::Result<capnp::text_list::Reader<'_>>) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(list) = list {
        for item in list.iter() {
            // An item the peer could not encode is an empty string, the
            // same equivalence the domain's own text field uses.
            match item {
                Ok(text) => out.push(super::text_of(text)),
                Err(_) => out.push(String::new()),
            }
        }
    }
    out
}

/// Write the engine's directory statistics into the contract's struct.
fn write_directory_stats(
    stats: &DirectoryStats,
    builder: &mut crate::schema::files_capnp::directory_stats::Builder<'_>,
) {
    builder.set_total_files(stats.total_files);
    builder.set_total_directories(stats.total_directories);
    builder.set_total_size(stats.total_size);
    let mut list = builder.reborrow().init_by_type(stats.by_type.len() as u32);
    for (index, count) in stats.by_type.iter().enumerate() {
        let mut item = list.reborrow().get(index as u32);
        item.set_type(count.kind.to_generated());
        item.set_count(count.count);
        item.set_total_size(count.total_size);
    }
    builder.set_largest_file(&stats.largest_file);
    builder.set_oldest_file(&stats.oldest_file);
    builder.set_newest_file(&stats.newest_file);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&stats.metadata, &mut metadata);
}

/// Write the engine's activity into the contract's `FileActivity`.
fn write_activity(
    activity: &FileActivity,
    builder: &mut crate::schema::files_capnp::file_activity::Builder<'_>,
) {
    builder
        .reborrow()
        .init_timestamp()
        .set_nanoseconds(activity.timestamp.as_nanos());
    builder.set_actor(&activity.actor);
    builder.set_action(&activity.action);
    builder.set_path(&activity.path);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&activity.metadata, &mut metadata);
}

/// Write the engine's lifecycle policy into the contract's struct.
fn write_lifecycle(
    policy: &LifecyclePolicy,
    builder: &mut crate::schema::files_capnp::lifecycle_policy::Builder<'_>,
) {
    builder.set_path(&policy.path);
    builder.set_action(policy.action.to_generated());
    builder
        .reborrow()
        .init_retention_period()
        .set_nanoseconds(policy.retention_period.as_nanos());
    builder
        .reborrow()
        .init_min_age()
        .set_nanoseconds(policy.min_age.as_nanos());
    builder.set_enabled(policy.enabled);
    builder.set_description(&policy.description);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&policy.metadata, &mut metadata);
}

/// The contract's `LifecyclePolicy` as the engine's policy.
fn read_lifecycle(
    reader: &crate::schema::files_capnp::lifecycle_policy::Reader<'_>,
) -> LifecyclePolicy {
    LifecyclePolicy {
        path: text_field(reader.get_path()),
        action: RetentionAction::from_generated(
            reader
                .get_action()
                .unwrap_or(crate::schema::files_capnp::RetentionAction::Unknown),
        ),
        retention_period: duration_result(reader.get_retention_period()),
        min_age: duration_result(reader.get_min_age()),
        enabled: reader.get_enabled(),
        description: text_field(reader.get_description()),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Write the engine's audit entry into the contract's `FileAuditEntry`.
fn write_audit_entry(
    entry: &FileAuditEntry,
    builder: &mut crate::schema::files_capnp::file_audit_entry::Builder<'_>,
) {
    builder
        .reborrow()
        .init_timestamp()
        .set_nanoseconds(entry.timestamp.as_nanos());
    builder.set_actor(&entry.actor);
    builder.set_action(entry.action.to_generated());
    builder.set_path(&entry.path);
    builder.set_result(&entry.result);
    builder.set_details(&entry.details);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&entry.metadata, &mut metadata);
}

/// The contract's `FileAuditFilter` as the engine's filter.
fn read_audit_filter(
    reader: &crate::schema::files_capnp::file_audit_filter::Reader<'_>,
) -> FileAuditFilter {
    let mut actions = Vec::new();
    if let Ok(list) = reader.get_actions() {
        for action in list.iter() {
            actions.push(AuditAction::from_generated(
                action.unwrap_or(crate::schema::files_capnp::AuditAction::Unknown),
            ));
        }
    }
    FileAuditFilter {
        path_pattern: text_field(reader.get_path_pattern()),
        actor: text_field(reader.get_actor()),
        actions,
        since: Timestamp::from_nanos(
            reader
                .get_since()
                .map(|t| t.get_nanoseconds())
                .unwrap_or(0),
        ),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// The contract's `FileAnalysisRequest` as the engine's request.
fn read_analysis_request(
    reader: &crate::schema::files_capnp::file_analysis_request::Reader<'_>,
) -> crate::domains::files::types::FileAnalysisRequest {
    crate::domains::files::types::FileAnalysisRequest {
        path: text_field(reader.get_path()),
        analysis_types: read_text_list(reader.get_analysis_types()),
        language: text_field(reader.get_language()),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Write the engine's analysis result into the contract's struct.
fn write_analysis_result(
    result: &crate::domains::files::types::FileAnalysisResult,
    builder: &mut crate::schema::files_capnp::file_analysis_result::Builder<'_>,
) {
    builder.set_path(&result.path);
    builder.set_analysis_type(&result.analysis_type);
    builder.reborrow().init_result().set_inline(&result.result);
    builder.set_confidence(result.confidence);
    builder
        .reborrow()
        .init_timestamp()
        .set_nanoseconds(result.timestamp.as_nanos());
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&result.metadata, &mut metadata);
}

/// The contract's `ContentSearchQuery` as the engine's query.
fn read_content_query(
    reader: &crate::schema::files_capnp::content_search_query::Reader<'_>,
) -> ContentSearchQuery {
    let mut mime_types = Vec::new();
    if let Ok(list) = reader.get_mime_types() {
        for mime in list.iter() {
            mime_types.push(read_mime(&mime));
        }
    }
    ContentSearchQuery {
        path: text_field(reader.get_path()),
        query: text_field(reader.get_query()),
        recursive: reader.get_recursive(),
        mime_types,
        page: pagination_result(reader.get_page()),
        annotations: reader
            .get_annotations()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Write the engine's content-search result into the contract's struct.
fn write_content_result(
    result: &ContentSearchResult,
    builder: &mut crate::schema::files_capnp::content_search_result::Builder<'_>,
) {
    builder.set_path(&result.path);
    builder.set_snippet(&result.snippet);
    builder.set_score(result.score);
    builder.set_line_number(result.line_number);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&result.metadata, &mut metadata);
}

/// Write the engine's trash entry into the contract's `TrashEntry`.
fn write_trash_entry(
    entry: &TrashEntry,
    builder: &mut crate::schema::files_capnp::trash_entry::Builder<'_>,
) {
    builder.set_entry_id(&entry.entry_id);
    builder.set_original_path(&entry.original_path);
    builder
        .reborrow()
        .init_deleted_at()
        .set_nanoseconds(entry.deleted_at.as_nanos());
    builder.set_size(entry.size);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&entry.metadata, &mut metadata);
}

/// Write the engine's recent file into the contract's `RecentFile`.
fn write_recent_file(
    file: &RecentFile,
    builder: &mut crate::schema::files_capnp::recent_file::Builder<'_>,
) {
    builder.set_path(&file.path);
    write_mime(&file.mime_type, &mut builder.reborrow().init_mime_type());
    builder
        .reborrow()
        .init_timestamp()
        .set_nanoseconds(file.timestamp.as_nanos());
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&file.metadata, &mut metadata);
}

/// Write the engine's filesystem health into the contract's struct.
fn write_health(
    health: &crate::domains::files::types::FilesystemHealth,
    builder: &mut crate::schema::files_capnp::filesystem_health::Builder<'_>,
) {
    builder.set_status(&health.status);
    builder
        .reborrow()
        .init_last_check()
        .set_nanoseconds(health.last_check.as_nanos());
    builder
        .reborrow()
        .init_next_scheduled()
        .set_nanoseconds(health.next_scheduled.as_nanos());
    builder.set_errors(health.errors);
    builder.set_warnings(health.warnings);
    builder.set_details(&health.details);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&health.metadata, &mut metadata);
}

/// Write the engine's quota into the contract's `FilesystemQuota`.
fn write_quota(
    quota: &crate::domains::files::types::FilesystemQuota,
    builder: &mut crate::schema::files_capnp::filesystem_quota::Builder<'_>,
) {
    builder.set_max_size_bytes(quota.max_size_bytes);
    builder.set_max_files(quota.max_files);
    builder.set_current_size_bytes(quota.current_size_bytes);
    builder.set_current_files(quota.current_files);
    builder.set_soft_limit_percent(quota.soft_limit_percent);
}

/// The contract's `FilesystemQuota` as the engine's quota.
fn read_quota(
    reader: &crate::schema::files_capnp::filesystem_quota::Reader<'_>,
) -> crate::domains::files::types::FilesystemQuota {
    crate::domains::files::types::FilesystemQuota {
        max_size_bytes: reader.get_max_size_bytes(),
        max_files: reader.get_max_files(),
        current_size_bytes: reader.get_current_size_bytes(),
        current_files: reader.get_current_files(),
        soft_limit_percent: reader.get_soft_limit_percent(),
    }
}

/// Write the engine's file event into the contract's `FileEvent`.
fn write_file_event(
    event: &FileEvent,
    builder: &mut crate::schema::files_capnp::file_event::Builder<'_>,
) {
    builder.reborrow().init_id().set_id(event.id.get());
    builder
        .reborrow()
        .init_timestamp()
        .set_nanoseconds(event.timestamp.as_nanos());
    builder.set_type(event.kind.to_generated());
    builder.set_path(&event.path);
    builder.set_old_path(&event.old_path);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&event.metadata, &mut metadata);
}

/// The file-event subscription the service hands back for
/// `watchDirectory @14`.
pub struct FileEventSubscriptionService<B> {
    backend: Arc<B>,
    subscription: WatchSubscription,
}

impl<B> Clone for FileEventSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> evsub::Server for FileEventSubscriptionService<B>
where
    B: FilesBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        _params: evsub::PullParams,
        mut results: evsub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let events = backend
                .pull_watch(subscription, 64)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_events(events.len() as u32);
            for (index, event) in events.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_file_event(event, &mut item);
            }
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: evsub::CancelParams,
        mut results: evsub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_watch(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The audit subscription the service hands back for
/// `subscribeFileAudit @57`.
pub struct FileAuditSubscriptionService<B> {
    backend: Arc<B>,
    subscription: AuditSubscription,
}

impl<B> Clone for FileAuditSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> ausub::Server for FileAuditSubscriptionService<B>
where
    B: FilesBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        _params: ausub::PullParams,
        mut results: ausub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let entries = backend
                .pull_audit(subscription, 64)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_entries(entries.len() as u32);
            for (index, entry) in entries.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_audit_entry(entry, &mut item);
            }
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: ausub::CancelParams,
        mut results: ausub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_audit(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The file service the service hands back for `acquireFile @73`.
///
/// The reference IS the authority, and it is bound to ONE path: every method
/// here goes through the engine's `bound_*` family, which refuses a path
/// that is not the one the capability was acquired for.
pub struct BoundFileService<B> {
    backend: Arc<B>,
    bound: BoundPath,
    class: CapabilityClass,
}

impl<B> Clone for BoundFileService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            bound: BoundPath {
                path: self.bound.path.clone(),
                authority: self.bound.authority,
            },
            class: self.class,
        }
    }
}

impl<B> BoundFileService<B>
where
    B: FilesBackend + 'static,
{
    /// The class's answer for a mutating verb on a connection that did not
    /// earn the right to change this host's files (law #5).
    fn gate(&self, verb: &str) -> Option<capnp::Error> {
        if self.class.is_control() {
            return None;
        }
        Some(errors::decline(
            ErrorCode::PermissionDenied,
            &format!(
                "permissionDenied: the {verb} verb changes this host's files, and this \
                 connection holds the read-only class"
            ),
        ))
    }
}

impl<B> fsvc::Server for BoundFileService<B>
where
    B: FilesBackend + 'static,
{
    fn open(
        self: Rc<Self>,
        params: fsvc::OpenParams,
        mut results: fsvc::OpenResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let bound = self.bound.clone();
        async move {
            let params = params.get()?;
            let request = OpenRequest::from_reader(&params.get_request()?);
            let handle = backend
                .bound_open(&bound, &request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_handle(&handle, &mut builder.init_result().init_handle());
            Ok(())
        }
    }

    fn read(
        self: Rc<Self>,
        params: fsvc::ReadParams,
        mut results: fsvc::ReadResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let bound = self.bound.clone();
        async move {
            let params = params.get()?;
            let offset = params.get_offset();
            let size = params.get_size();
            let bytes = backend
                .bound_read(&bound, offset, size)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_data().set_inline(&bytes);
            Ok(())
        }
    }

    fn write(
        self: Rc<Self>,
        params: fsvc::WriteParams,
        mut results: fsvc::WriteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let bound = self.bound.clone();
        async move {
            if let Some(refusal) = self.gate("write") {
                return Err(refusal);
            }
            let params = params.get()?;
            let offset = params.get_offset();
            let bytes = read_data(&params.get_data()?);
            let written = backend
                .bound_write(&bound, offset, &bytes)
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_written(written);
            Ok(())
        }
    }

    fn stat(
        self: Rc<Self>,
        _params: fsvc::StatParams,
        mut results: fsvc::StatResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let bound = self.bound.clone();
        async move {
            let info = backend
                .bound_stat(&bound)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match info {
                FileInfoResult::Info(value) => {
                    write_file_info(&value, &mut arm.init_info());
                }
                FileInfoResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                FileInfoResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn close(
        self: Rc<Self>,
        _params: fsvc::CloseParams,
        mut results: fsvc::CloseResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let bound = self.bound.clone();
        async move {
            let outcome = backend.bound_close(&bound);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The authority token a capability carries, folded into the engine's opaque
/// u64.
///
/// The engine's backend treats the authority as OPAQUE — it carries it and
/// never interprets it — so the fold only has to be stable and collision-
/// free enough to tell two capabilities apart in the backend's own record.
fn authority_of(capability: &crate::schema::capability_capnp::capability::Reader<'_>) -> u64 {
    let bytes = capability
        .get_id()
        .map(|id| id.to_vec())
        .unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl<B> fs::Server for FilesService<B>
where
    B: FilesBackend + 'static,
{
    // ── The reads ────────────────────────────────────────────────────────

    fn list_directory(
        self: Rc<Self>,
        params: fs::ListDirectoryParams,
        mut results: fs::ListDirectoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let filter = DirectoryFilter::from_reader(&params.get_filter()?);
            let entries: Page<DirectoryEntry> = backend
                .list_directory(&path, &filter)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_entries(entries.items.len() as u32);
            for (index, entry) in entries.items.iter().enumerate() {
                let item = list.reborrow().get(index as u32);
                entry.write_to(item);
            }
            builder.set_total(entries.total);
            Ok(())
        }
    }

    fn stat_file(
        self: Rc<Self>,
        params: fs::StatFileParams,
        mut results: fs::StatFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let info = backend
                .stat_file(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match info {
                FileInfoResult::Info(value) => {
                    write_file_info(&value, &mut arm.init_info());
                }
                FileInfoResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                FileInfoResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn get_file_type(
        self: Rc<Self>,
        params: fs::GetFileTypeParams,
        mut results: fs::GetFileTypeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let kind = backend
                .get_file_type(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            // The contract's answer for a type read is the whole `FileInfo`
            // union, so the type is reported through the same shape a `stat`
            // uses — never as a bare enum the caller cannot place.
            let builder = results.get();
            let arm = builder.init_result();
            let mut info = arm.init_info();
            info.set_path(&path);
            info.set_type(kind.to_generated());
            Ok(())
        }
    }

    fn file_exists(
        self: Rc<Self>,
        params: fs::FileExistsParams,
        mut results: fs::FileExistsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let exists = backend
                .file_exists(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_exists(exists);
            Ok(())
        }
    }

    fn get_special_path(
        self: Rc<Self>,
        params: fs::GetSpecialPathParams,
        mut results: fs::GetSpecialPathResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let kind = SpecialPath::from_generated(
                params
                    .get_type()
                    .unwrap_or(crate::schema::files_capnp::SpecialPathType::Other),
            );
            let path = backend
                .get_special_path(kind)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut arm = builder.init_result();
            match path {
                TextResult::Value(value) => {
                    arm.set_value(&value);
                }
                TextResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                TextResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn resolve_symlink(
        self: Rc<Self>,
        params: fs::ResolveSymlinkParams,
        mut results: fs::ResolveSymlinkResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let target = backend
                .resolve_symlink(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut arm = builder.init_result();
            match target {
                TextResult::Value(value) => {
                    arm.set_value(&value);
                }
                TextResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                TextResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn get_current_directory(
        self: Rc<Self>,
        _params: fs::GetCurrentDirectoryParams,
        mut results: fs::GetCurrentDirectoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let path = backend
                .get_current_directory()
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_path(&path);
            Ok(())
        }
    }

    fn search_files(
        self: Rc<Self>,
        params: fs::SearchFilesParams,
        mut results: fs::SearchFilesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let query = read_search_query(&params.get_query()?);
            let found: Page<FileSearchResult> = backend
                .search_files(&query)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_results(found.items.len() as u32);
            for (index, result) in found.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                item.set_path(&result.path);
                item.set_name(&result.name);
                item.set_type(result.kind.to_generated());
                item.set_size(result.size);
                item
                    .reborrow()
                    .init_modified()
                    .set_nanoseconds(result.modified.as_nanos());
                item.set_score(result.score);
                let mut metadata = item.reborrow().init_metadata();
                write_metadata(&result.metadata, &mut metadata);
            }
            builder.set_total(found.total);
            Ok(())
        }
    }

    fn walk_tree(
        self: Rc<Self>,
        params: fs::WalkTreeParams,
        mut results: fs::WalkTreeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let depth = params.get_max_depth();
            let page = pagination(&params.get_page()?);
            let entries: Page<DirectoryEntry> = backend
                .walk_tree(&path, depth, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_entries(entries.items.len() as u32);
            for (index, entry) in entries.items.iter().enumerate() {
                let item = list.reborrow().get(index as u32);
                entry.write_to(item);
            }
            builder.set_total(entries.total);
            Ok(())
        }
    }

    fn get_disk_usage(
        self: Rc<Self>,
        params: fs::GetDiskUsageParams,
        mut results: fs::GetDiskUsageResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let usage: DiskUsage = backend
                .get_disk_usage(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut out = builder.init_usage();
            out.set_total(usage.total);
            out.set_used(usage.used);
            out.set_free(usage.free);
            out.set_percent_used(usage.percent_used);
            out.set_mount_point(&usage.mount_point);
            out.reborrow().init_mount_id().set_id(usage.mount_id.get());
            out.reborrow()
                .init_filesystem_id()
                .set_id(usage.filesystem_id.get());
            let mut metadata = out.reborrow().init_metadata();
            write_metadata(&usage.metadata, &mut metadata);
            Ok(())
        }
    }

    fn get_file_metadata(
        self: Rc<Self>,
        params: fs::GetFileMetadataParams,
        mut results: fs::GetFileMetadataResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let info: ExtendedMetadata = backend
                .get_file_metadata(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_extended_metadata(&info, &mut builder.init_metadata());
            Ok(())
        }
    }

    fn compare_files(
        self: Rc<Self>,
        params: fs::CompareFilesParams,
        mut results: fs::CompareFilesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let first = text_field(params.get_path1());
            let second = text_field(params.get_path2());
            let comparison = backend
                .compare_files(&first, &second)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_comparison(&comparison, &mut builder.init_result());
            Ok(())
        }
    }

    fn compare_directories(
        self: Rc<Self>,
        params: fs::CompareDirectoriesParams,
        mut results: fs::CompareDirectoriesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let first = text_field(params.get_path1());
            let second = text_field(params.get_path2());
            let comparison = backend
                .compare_directories(&first, &second)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_comparison(&comparison, &mut builder.init_result());
            Ok(())
        }
    }

    fn get_locks(
        self: Rc<Self>,
        params: fs::GetLocksParams,
        mut results: fs::GetLocksResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let locks: Page<FileLock> = backend
                .get_locks(&path, Pagination { offset: 0, limit: 0 })
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_locks(locks.items.len() as u32);
            for (index, lock) in locks.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_lock(lock, &mut item);
            }
            Ok(())
        }
    }

    fn get_file_versions(
        self: Rc<Self>,
        params: fs::GetFileVersionsParams,
        mut results: fs::GetFileVersionsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let page = pagination(&params.get_page()?);
            let versions: Page<FileVersion> = backend
                .get_file_versions(&path, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_versions(versions.items.len() as u32);
            for (index, version) in versions.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_version(version, &mut item);
            }
            builder.set_total(versions.total);
            Ok(())
        }
    }

    fn get_file_version(
        self: Rc<Self>,
        params: fs::GetFileVersionParams,
        mut results: fs::GetFileVersionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let version_id = params.get_version_id();
            let version = backend
                .get_file_version(&path, version_id)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match version {
                VersionResult::Version(value) => {
                    write_version(&value, &mut arm.init_version());
                }
                VersionResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                VersionResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn compare_versions(
        self: Rc<Self>,
        params: fs::CompareVersionsParams,
        mut results: fs::CompareVersionsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let first = params.get_version_a();
            let second = params.get_version_b();
            let comparison = backend
                .compare_versions(&path, first, second)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_comparison(&comparison, &mut builder.init_result());
            Ok(())
        }
    }

    fn get_by_hash(
        self: Rc<Self>,
        params: fs::GetByHashParams,
        mut results: fs::GetByHashResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let request = read_by_hash_request(&params.get_request()?);
            let reference = backend
                .get_by_hash(&request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match reference {
                ContentRefResult::Ref(value) => {
                    write_content_ref(&value, &mut arm.init_ref());
                }
                ContentRefResult::Handle(value) => {
                    write_handle(&value, &mut arm.init_handle());
                }
                ContentRefResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                ContentRefResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn store_content_addressed(
        self: Rc<Self>,
        params: fs::StoreContentAddressedParams,
        mut results: fs::StoreContentAddressedResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let reference = backend
                .store_content_addressed(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match reference {
                ContentRefResult::Ref(value) => {
                    write_content_ref(&value, &mut arm.init_ref());
                }
                ContentRefResult::Handle(value) => {
                    write_handle(&value, &mut arm.init_handle());
                }
                ContentRefResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                ContentRefResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn deduplicate(
        self: Rc<Self>,
        params: fs::DeduplicateParams,
        mut results: fs::DeduplicateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let (saved, references) = backend
                .deduplicate(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            builder.set_saved_bytes(saved);
            builder.set_reference_count(references);
            Ok(())
        }
    }

    fn get_deduplication_stats(
        self: Rc<Self>,
        params: fs::GetDeduplicationStatsParams,
        mut results: fs::GetDeduplicationStatsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let info: FileDeduplicationInfo = backend
                .get_deduplication_stats(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_dedup_info(&info, &mut builder.init_stats());
            Ok(())
        }
    }

    fn get_compression(
        self: Rc<Self>,
        params: fs::GetCompressionParams,
        mut results: fs::GetCompressionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let config = backend
                .get_compression(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_compression(&config, &mut builder.init_config());
            Ok(())
        }
    }

    fn get_encryption_info(
        self: Rc<Self>,
        params: fs::GetEncryptionInfoParams,
        mut results: fs::GetEncryptionInfoResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("getEncryptionInfo") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let info = backend
                .get_encryption_info(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match info {
                EncryptionInfoResult::Info(value) => {
                    let mut out = arm.init_info();
                    out.reborrow().init_key_id().set_id(value.key_id.get());
                    out.set_algorithm(&value.algorithm);
                    out.set_encrypted_size(value.encrypted_size);
                    let mut metadata = out.reborrow().init_metadata();
                    write_metadata(&value.metadata, &mut metadata);
                }
                EncryptionInfoResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                EncryptionInfoResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn get_file_hash(
        self: Rc<Self>,
        params: fs::GetFileHashParams,
        mut results: fs::GetFileHashResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let algorithm = text_field(params.get_algorithm());
            let hash = backend
                .get_file_hash(&path, &algorithm)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match hash {
                FileHashResult::Hash(bytes) => {
                    write_hash(&bytes, &mut arm.init_hash());
                }
                FileHashResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                FileHashResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn get_file_checksum(
        self: Rc<Self>,
        params: fs::GetFileChecksumParams,
        mut results: fs::GetFileChecksumResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let checksum: ChecksumResult = backend
                .get_file_checksum(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut out = builder.init_result();
            out.set_valid(checksum.valid);
            write_hash(&checksum.stored, &mut out.reborrow().init_stored());
            write_hash(&checksum.computed, &mut out.reborrow().init_computed());
            let mut metadata = out.reborrow().init_metadata();
            write_metadata(&checksum.metadata, &mut metadata);
            Ok(())
        }
    }

    fn get_lifecycle(
        self: Rc<Self>,
        params: fs::GetLifecycleParams,
        mut results: fs::GetLifecycleResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let policy = backend
                .get_lifecycle(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match policy {
                LifecyclePolicyResult::Policy(value) => {
                    write_lifecycle(&value, &mut arm.init_policy());
                }
                LifecyclePolicyResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                LifecyclePolicyResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn get_directory_size(
        self: Rc<Self>,
        params: fs::GetDirectorySizeParams,
        mut results: fs::GetDirectorySizeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let size = backend
                .get_directory_size(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_size(size);
            Ok(())
        }
    }

    fn get_directory_stats(
        self: Rc<Self>,
        params: fs::GetDirectoryStatsParams,
        mut results: fs::GetDirectoryStatsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let stats: DirectoryStats = backend
                .get_directory_stats(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_directory_stats(&stats, &mut builder.init_stats());
            Ok(())
        }
    }

    fn get_file_activity(
        self: Rc<Self>,
        params: fs::GetFileActivityParams,
        mut results: fs::GetFileActivityResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let page = pagination(&params.get_page()?);
            let activities: Page<FileActivity> = backend
                .get_file_activity(&path, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_activities(activities.items.len() as u32);
            for (index, activity) in activities.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_activity(activity, &mut item);
            }
            builder.set_total(activities.total);
            Ok(())
        }
    }

    fn analyze_file_content(
        self: Rc<Self>,
        params: fs::AnalyzeFileContentParams,
        mut results: fs::AnalyzeFileContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("analyzeFileContent") {
                return Err(refusal);
            }
            let params = params.get()?;
            let request = read_analysis_request(&params.get_request()?);
            let result = backend
                .analyze_file_content(&request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_analysis_result(&result, &mut builder.init_result());
            Ok(())
        }
    }

    fn search_file_content(
        self: Rc<Self>,
        params: fs::SearchFileContentParams,
        mut results: fs::SearchFileContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let query = read_content_query(&params.get_query()?);
            let found: Page<ContentSearchResult> = backend
                .search_file_content(&query)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_results(found.items.len() as u32);
            for (index, result) in found.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_content_result(result, &mut item);
            }
            builder.set_total(found.total);
            Ok(())
        }
    }

    fn branch_version(
        self: Rc<Self>,
        params: fs::BranchVersionParams,
        mut results: fs::BranchVersionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let version_id = params.get_version_id();
            let branch_name = text_field(params.get_branch_name());
            let branch = backend
                .branch_version(&path, version_id, &branch_name)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match branch {
                crate::domains::files::types::BranchResult::Branch(value) => {
                    write_branch(&value, &mut arm.init_branch());
                }
                crate::domains::files::types::BranchResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                crate::domains::files::types::BranchResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn list_branches(
        self: Rc<Self>,
        params: fs::ListBranchesParams,
        mut results: fs::ListBranchesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let page = pagination(&params.get_page()?);
            let branches: Page<FileBranch> = backend
                .list_branches(&path, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_branches(branches.items.len() as u32);
            for (index, branch) in branches.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_branch(branch, &mut item);
            }
            builder.set_total(branches.total);
            Ok(())
        }
    }

    fn list_trash(
        self: Rc<Self>,
        params: fs::ListTrashParams,
        mut results: fs::ListTrashResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = pagination(&params.get_page()?);
            let entries: Page<TrashEntry> = backend
                .list_trash(page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_entries(entries.items.len() as u32);
            for (index, entry) in entries.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_trash_entry(entry, &mut item);
            }
            builder.set_total(entries.total);
            Ok(())
        }
    }

    fn get_recent_files(
        self: Rc<Self>,
        params: fs::GetRecentFilesParams,
        mut results: fs::GetRecentFilesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = pagination(&params.get_page()?);
            let files: Page<RecentFile> = backend
                .get_recent_files(page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_files(files.items.len() as u32);
            for (index, file) in files.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_recent_file(file, &mut item);
            }
            builder.set_total(files.total);
            Ok(())
        }
    }

    fn get_default_app(
        self: Rc<Self>,
        params: fs::GetDefaultAppParams,
        mut results: fs::GetDefaultAppResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let mime = read_mime(&params.get_mime_type()?);
            let app = backend
                .get_default_app(&mime)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match app {
                crate::domains::files::types::DefaultAppResult::App(value) => {
                    let mut out = arm.init_app();
                    out.reborrow().init_app_id().set_id(value.app_id.get());
                    out.set_name(&value.name);
                }
                crate::domains::files::types::DefaultAppResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                crate::domains::files::types::DefaultAppResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn get_filesystem_health(
        self: Rc<Self>,
        params: fs::GetFilesystemHealthParams,
        mut results: fs::GetFilesystemHealthResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let health = backend
                .get_filesystem_health(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match health {
                crate::domains::files::types::FilesystemHealthResult::Health(value) => {
                    write_health(&value, &mut arm.init_health());
                }
                crate::domains::files::types::FilesystemHealthResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                crate::domains::files::types::FilesystemHealthResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    fn get_quota(
        self: Rc<Self>,
        params: fs::GetQuotaParams,
        mut results: fs::GetQuotaResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let quota = backend
                .get_quota(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_quota(&quota, &mut builder.init_quota());
            Ok(())
        }
    }

    fn create_snapshot(
        self: Rc<Self>,
        params: fs::CreateSnapshotParams,
        mut results: fs::CreateSnapshotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let name = text_field(params.get_name());
            let snapshot = backend
                .create_snapshot(&path, &name)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            match snapshot {
                crate::domains::files::types::SnapshotRefResult::Snapshot(id) => {
                    arm.init_snapshot_id().set_id(id.get());
                }
                crate::domains::files::types::SnapshotRefResult::Error(error) => {
                    let mut failure = arm.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                crate::domains::files::types::SnapshotRefResult::Unknown => {
                    arm.init_unknown();
                }
            }
            Ok(())
        }
    }

    // ── The subscriptions ─────────────────────────────────────────────────

    fn watch_directory(
        self: Rc<Self>,
        params: fs::WatchDirectoryParams,
        mut results: fs::WatchDirectoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let subscription = backend
                .watch_directory(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = FileEventSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: evsub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn subscribe_file_audit(
        self: Rc<Self>,
        params: fs::SubscribeFileAuditParams,
        mut results: fs::SubscribeFileAuditResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let filter = read_audit_filter(&params.get_filter()?);
            let subscription = backend
                .subscribe_audit(&filter)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = FileAuditSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: ausub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    // ── The writes ───────────────────────────────────────────────────────

    fn open_file(
        self: Rc<Self>,
        params: fs::OpenFileParams,
        mut results: fs::OpenFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let request = OpenRequest::from_reader(&params.get_request()?);
            let handle = backend
                .open_file(&path, &request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            write_handle(&handle, &mut arm.init_handle());
            Ok(())
        }
    }

    fn close_file(
        self: Rc<Self>,
        params: fs::CloseFileParams,
        mut results: fs::CloseFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let handle = read_handle(&params.get_handle()?);
            let outcome = backend.close_file(&handle);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn read_file(
        self: Rc<Self>,
        params: fs::ReadFileParams,
        mut results: fs::ReadFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let handle = read_handle(&params.get_handle()?);
            let offset = params.get_offset();
            let size = params.get_size();
            let bytes = backend
                .read_file(&handle, offset, size)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_data().set_inline(&bytes);
            Ok(())
        }
    }

    fn write_file(
        self: Rc<Self>,
        params: fs::WriteFileParams,
        mut results: fs::WriteFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("writeFile") {
                return Err(refusal);
            }
            let params = params.get()?;
            let handle = read_handle(&params.get_handle()?);
            let offset = params.get_offset();
            let bytes = read_data(&params.get_data()?);
            let written = backend
                .write_file(&handle, offset, &bytes)
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_written(written);
            Ok(())
        }
    }

    fn flush_file(
        self: Rc<Self>,
        params: fs::FlushFileParams,
        mut results: fs::FlushFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let handle = read_handle(&params.get_handle()?);
            let outcome = backend.flush_file(&handle);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn create_directory(
        self: Rc<Self>,
        params: fs::CreateDirectoryParams,
        mut results: fs::CreateDirectoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("createDirectory") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let parents = params.get_parents();
            let outcome = backend.create_directory(&path, parents);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn remove_directory(
        self: Rc<Self>,
        params: fs::RemoveDirectoryParams,
        mut results: fs::RemoveDirectoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("removeDirectory") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let recursive = params.get_recursive();
            let outcome = backend.remove_directory(&path, recursive);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn rename_file(
        self: Rc<Self>,
        params: fs::RenameFileParams,
        mut results: fs::RenameFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("renameFile") {
                return Err(refusal);
            }
            let params = params.get()?;
            let old = text_field(params.get_old_path());
            let new = text_field(params.get_new_path());
            let outcome = backend.rename_file(&old, &new);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn remove_file(
        self: Rc<Self>,
        params: fs::RemoveFileParams,
        mut results: fs::RemoveFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("removeFile") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let recursive = params.get_recursive();
            let outcome = backend.remove_file(&path, recursive);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn copy_file(
        self: Rc<Self>,
        params: fs::CopyFileParams,
        mut results: fs::CopyFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("copyFile") {
                return Err(refusal);
            }
            let params = params.get()?;
            let source = text_field(params.get_src());
            let destination = text_field(params.get_dst());
            let outcome = backend.copy_file(&source, &destination);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn create_symlink(
        self: Rc<Self>,
        params: fs::CreateSymlinkParams,
        mut results: fs::CreateSymlinkResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("createSymlink") {
                return Err(refusal);
            }
            let params = params.get()?;
            let target = text_field(params.get_target());
            let link = text_field(params.get_link());
            let outcome = backend.create_symlink(&target, &link);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn link_file(
        self: Rc<Self>,
        params: fs::LinkFileParams,
        mut results: fs::LinkFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("linkFile") {
                return Err(refusal);
            }
            let params = params.get()?;
            let target = text_field(params.get_target());
            let link = text_field(params.get_link());
            let hard = params.get_hard();
            let outcome = backend.link_file(&target, &link, hard);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn change_directory(
        self: Rc<Self>,
        params: fs::ChangeDirectoryParams,
        mut results: fs::ChangeDirectoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("changeDirectory") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let outcome = backend.change_directory(&path);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn create_file(
        self: Rc<Self>,
        params: fs::CreateFileParams,
        mut results: fs::CreateFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("createFile") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let content = read_data(&params.get_content()?);
            let parents = params.get_parents();
            let outcome = backend.create_file(&path, &content, parents);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn touch(
        self: Rc<Self>,
        params: fs::TouchParams,
        mut results: fs::TouchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("touch") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let outcome = backend.touch(&path);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn lock_file(
        self: Rc<Self>,
        params: fs::LockFileParams,
        mut results: fs::LockFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let request = read_lock_request(&params.get_request()?);
            let lock = backend
                .lock_file(&request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_lock(&lock, &mut builder.init_lock());
            Ok(())
        }
    }

    fn unlock_file(
        self: Rc<Self>,
        params: fs::UnlockFileParams,
        mut results: fs::UnlockFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("unlockFile") {
                return Err(refusal);
            }
            let params = params.get()?;
            let lock_id = params.get_lock_id();
            let outcome = backend.unlock_file(lock_id);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn restore_version(
        self: Rc<Self>,
        params: fs::RestoreVersionParams,
        mut results: fs::RestoreVersionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("restoreVersion") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let version_id = params.get_version_id();
            let outcome = backend.restore_version(&path, version_id);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn create_ephemeral(
        self: Rc<Self>,
        params: fs::CreateEphemeralParams,
        mut results: fs::CreateEphemeralResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("createEphemeral") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let config = read_ephemeral_config(&params.get_config()?);
            let handle = backend
                .create_ephemeral(&path, &config)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            write_handle(&handle, &mut arm.init_handle());
            Ok(())
        }
    }

    fn create_encrypted(
        self: Rc<Self>,
        params: fs::CreateEncryptedParams,
        mut results: fs::CreateEncryptedResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("createEncrypted") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("createEncrypted") {
                return Err(refusal);
            }
            let params = params.get()?;
            let request = read_encrypted_request(&params.get_request()?);
            let handle = backend
                .create_encrypted(&request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let arm = builder.init_result();
            write_handle(&handle, &mut arm.init_handle());
            Ok(())
        }
    }

    fn decrypt_file(
        self: Rc<Self>,
        params: fs::DecryptFileParams,
        mut results: fs::DecryptFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("decryptFile") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("decryptFile") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let key = KeyId::new(params.get_key_id()?.get_id());
            let outcome = backend.decrypt_file(&path, key);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_compression(
        self: Rc<Self>,
        params: fs::SetCompressionParams,
        mut results: fs::SetCompressionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setCompression") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let config = read_compression(&params.get_config()?);
            let outcome = backend.set_compression(&path, &config);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn batch_copy(
        self: Rc<Self>,
        params: fs::BatchCopyParams,
        mut results: fs::BatchCopyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("batchCopy") {
                return Err(refusal);
            }
            let params = params.get()?;
            let request = read_batch_copy(&params.get_request()?);
            let result: BatchResult = backend
                .batch_copy(&request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_batch_result(&result, &mut builder.init_result());
            Ok(())
        }
    }

    fn batch_move(
        self: Rc<Self>,
        params: fs::BatchMoveParams,
        mut results: fs::BatchMoveResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("batchMove") {
                return Err(refusal);
            }
            let params = params.get()?;
            let request = read_batch_move(&params.get_request()?);
            let result: BatchResult = backend
                .batch_move(&request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_batch_result(&result, &mut builder.init_result());
            Ok(())
        }
    }

    fn batch_delete(
        self: Rc<Self>,
        params: fs::BatchDeleteParams,
        mut results: fs::BatchDeleteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("batchDelete") {
                return Err(refusal);
            }
            let params = params.get()?;
            let request = read_batch_delete(&params.get_request()?);
            let result: BatchResult = backend
                .batch_delete(&request)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_batch_result(&result, &mut builder.init_result());
            Ok(())
        }
    }

    fn set_lifecycle(
        self: Rc<Self>,
        params: fs::SetLifecycleParams,
        mut results: fs::SetLifecycleResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setLifecycle") {
                return Err(refusal);
            }
            let params = params.get()?;
            let policy = read_lifecycle(&params.get_policy()?);
            let outcome = backend.set_lifecycle(&policy.path, &policy);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn apply_lifecycle(
        self: Rc<Self>,
        params: fs::ApplyLifecycleParams,
        mut results: fs::ApplyLifecycleResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("applyLifecycle") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let processed = backend
                .apply_lifecycle(&path)
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_processed(processed);
            Ok(())
        }
    }

    fn merge_branch(
        self: Rc<Self>,
        params: fs::MergeBranchParams,
        mut results: fs::MergeBranchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("mergeBranch") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let branch = text_field(params.get_branch_name());
            let outcome = backend.merge_branch(&path, &branch);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn restore_trash(
        self: Rc<Self>,
        params: fs::RestoreTrashParams,
        mut results: fs::RestoreTrashResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("restoreTrash") {
                return Err(refusal);
            }
            let params = params.get()?;
            let entry = text_field(params.get_entry_id());
            let target = text_field(params.get_target_path());
            let outcome = backend.restore_trash(&entry, &target);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn empty_trash(
        self: Rc<Self>,
        _params: fs::EmptyTrashParams,
        mut results: fs::EmptyTrashResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("emptyTrash") {
                return Err(refusal);
            }
            let outcome = backend.empty_trash();
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn clear_recent_files(
        self: Rc<Self>,
        _params: fs::ClearRecentFilesParams,
        mut results: fs::ClearRecentFilesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("clearRecentFiles") {
                return Err(refusal);
            }
            let outcome = backend.clear_recent_files();
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_quota(
        self: Rc<Self>,
        params: fs::SetQuotaParams,
        mut results: fs::SetQuotaResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setQuota") {
                return Err(refusal);
            }
            let params = params.get()?;
            let path = text_field(params.get_path());
            let quota = read_quota(&params.get_quota()?);
            let outcome = backend.set_quota(&path, &quota);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    // ── The capability ────────────────────────────────────────────────────

    fn acquire_file(
        self: Rc<Self>,
        params: fs::AcquireFileParams,
        mut results: fs::AcquireFileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let path = text_field(params.get_path());
            let authority = authority_of(&params.get_authority()?);
            let bound = backend
                .acquire_file(&path, authority)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = BoundFileService {
                backend: Arc::clone(&backend),
                bound,
                class: self.class,
            };
            let client: fsvc::Client = capnp_rpc::new_client(server);
            results.get().set_service(client);
            Ok(())
        }
    }
}

/// The files domain, as a served domain: the contract interface it serves,
/// the descriptor discovery publishes for it, and the per-connection server.
pub struct FilesDomain<B> {
    backend: Arc<B>,
}

impl<B> FilesDomain<B>
where
    B: FilesBackend + 'static,
{
    /// The files domain over this backend. The backend is shared: the same
    /// host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for FilesDomain<B>
where
    B: FilesBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        fs::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::FILES_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "files"
    }

    fn category(&self) -> &'static str {
        "files"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    /// The service's own state: the files domain cannot answer every method
    /// of its contract on this host (the encryption pair and the audit
    /// subscription, measured), so the service is `degraded` and the missing
    /// methods are named in the registry.
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

    /// The commands this domain publishes through the registry (cli.capnp).
    ///
    /// The paths are the PRODUCT's own words — what `smart files …` types —
    /// and each one names the contract method that answers it. A verb that
    /// needs several methods publishes the one that produces its value and
    /// lists the others under the stable `composes` key.
    ///
    /// EVERY contract method of this domain is published: a method the
    /// engine serves must not be hidden from the surface.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "files ls",
                "Files",
                "listDirectory",
                "one directory's entries",
                0,
                false,
            )
            .with_row(matrix_row("listDirectory")),
            DomainCommand::new(
                "files find",
                "Files",
                "searchFiles",
                "a name search under a path",
                1,
                false,
            )
            .with_row(matrix_row("searchFiles")),
            DomainCommand::new(
                "files tree",
                "Files",
                "walkTree",
                "a bounded tree walk",
                2,
                false,
            )
            .with_row(matrix_row("walkTree")),
            DomainCommand::new(
                "files stat",
                "Files",
                "statFile",
                "one path's own metadata",
                3,
                false,
            )
            .with_row(matrix_row("statFile")),
            DomainCommand::new(
                "files xattrs",
                "Files",
                "getFileMetadata",
                "one path's extended metadata",
                4,
                false,
            )
            .with_row(matrix_row("getFileMetadata")),
            DomainCommand::composing(
                "files cat",
                "Files",
                "readFile",
                "one file's bytes",
                5,
                false,
                "openFile,closeFile",
            )
            .with_row(matrix_row("readFile")),
            DomainCommand::composing(
                "files write",
                "Files",
                "writeFile",
                "write bytes into a file",
                6,
                false,
                "openFile,closeFile",
            )
            .with_row(matrix_row("writeFile")),
            DomainCommand::new(
                "files du",
                "Files",
                "getDiskUsage",
                "the disk usage of the filesystem a path lives on",
                7,
                false,
            )
            .with_row(matrix_row("getDiskUsage")),
            DomainCommand::new(
                "files locks",
                "Files",
                "getLocks",
                "the advisory locks on one path",
                8,
                false,
            )
            .with_row(matrix_row("getLocks")),
            DomainCommand::new(
                "files hash",
                "Files",
                "getFileHash",
                "a file's content hash",
                9,
                false,
            )
            .with_row(matrix_row("getFileHash")),
            DomainCommand::new(
                "files versions",
                "Files",
                "getFileVersions",
                "the versions the store holds for a path",
                10,
                false,
            )
            .with_row(matrix_row("getFileVersions")),
            DomainCommand::new(
                "files branches",
                "Files",
                "listBranches",
                "the branches the store holds for a path",
                11,
                false,
            )
            .with_row(matrix_row("listBranches")),
            DomainCommand::new(
                "files store",
                "Files",
                "storeContentAddressed",
                "the content-addressed reference of a path",
                12,
                false,
            )
            .with_row(matrix_row("storeContentAddressed")),
            DomainCommand::new(
                "files dedupe",
                "Files",
                "deduplicate",
                "the measured inode sharing for a path",
                13,
                false,
            )
            .with_row(matrix_row("deduplicate")),
            DomainCommand::composing(
                "files watch",
                "Files",
                "watchDirectory",
                "a live event stream for one directory",
                14,
                false,
                "pull,cancel",
            )
            .with_row(matrix_row("watchDirectory")),
            DomainCommand::new(
                "files special",
                "Files",
                "getSpecialPath",
                "an XDG special path, resolved by the host",
                15,
                false,
            )
            .with_row(matrix_row("getSpecialPath")),
            DomainCommand::new(
                "files trash",
                "Files",
                "listTrash",
                "the host's own trash",
                16,
                true,
            )
            .with_row(matrix_row("listTrash")),
            DomainCommand::new(
                "files recents",
                "Files",
                "getRecentFiles",
                "the host's own recently-used record",
                17,
                true,
            )
            .with_row(matrix_row("getRecentFiles")),
            DomainCommand::new(
                "files app",
                "Files",
                "getDefaultApp",
                "the default application for a mime type",
                18,
                false,
            )
            .with_row(matrix_row("getDefaultApp")),
            DomainCommand::new(
                "files grep",
                "Files",
                "searchFileContent",
                "a content search over real lines",
                19,
                false,
            )
            .with_row(matrix_row("searchFileContent")),
            DomainCommand::new(
                "files compare",
                "Files",
                "compareFiles",
                "compare two files byte-for-byte",
                20,
                false,
            )
            .with_row(matrix_row("compareFiles")),
            DomainCommand::new(
                "files health",
                "Files",
                "getFilesystemHealth",
                "the measured health of one filesystem",
                21,
                false,
            )
            .with_row(matrix_row("getFilesystemHealth")),
            DomainCommand::new(
                "files quota",
                "Files",
                "getQuota",
                "the measured quota of one filesystem",
                22,
                false,
            )
            .with_row(matrix_row("getQuota")),
            DomainCommand::new(
                "files mkdir",
                "Files",
                "createDirectory",
                "create a directory",
                23,
                false,
            )
            .with_row(matrix_row("createDirectory")),
            DomainCommand::new(
                "files touch",
                "Files",
                "touch",
                "touch a path's timestamps",
                24,
                false,
            )
            .with_row(matrix_row("touch")),
            DomainCommand::new(
                "files rm",
                "Files",
                "removeFile",
                "remove a file or a tree",
                25,
                false,
            )
            .with_row(matrix_row("removeFile")),
            DomainCommand::new(
                "files mv",
                "Files",
                "renameFile",
                "rename or move a path",
                26,
                false,
            )
            .with_row(matrix_row("renameFile")),
            DomainCommand::new(
                "files cp",
                "Files",
                "copyFile",
                "copy a path",
                27,
                false,
            )
            .with_row(matrix_row("copyFile")),
            DomainCommand::composing(
                "files link",
                "Files",
                "createSymlink",
                "create a symbolic or hard link",
                28,
                false,
                "linkFile",
            )
            .with_row(matrix_row("createSymlink")),
            DomainCommand::new(
                "files lock",
                "Files",
                "lockFile",
                "take an advisory lock on a path",
                29,
                false,
            )
            .with_row(matrix_row("lockFile")),
            DomainCommand::new(
                "files unlock",
                "Files",
                "unlockFile",
                "release an advisory lock",
                30,
                false,
            )
            .with_row(matrix_row("unlockFile")),
            // ── The methods the product's shell has no verb for ──────────
            DomainCommand::new(
                "files open",
                "Files",
                "openFile",
                "open a path and take a handle",
                31,
                false,
            )
            .with_row(matrix_row("openFile")),
            DomainCommand::new(
                "files close",
                "Files",
                "closeFile",
                "close a handle",
                32,
                false,
            )
            .with_row(matrix_row("closeFile")),
            DomainCommand::new(
                "files flush",
                "Files",
                "flushFile",
                "flush a handle's writes",
                33,
                false,
            )
            .with_row(matrix_row("flushFile")),
            DomainCommand::new(
                "files exists",
                "Files",
                "fileExists",
                "whether a path exists",
                34,
                false,
            )
            .with_row(matrix_row("fileExists")),
            DomainCommand::new(
                "files create",
                "Files",
                "createFile",
                "create a file with content",
                35,
                false,
            )
            .with_row(matrix_row("createFile")),
            DomainCommand::new(
                "files rmdir",
                "Files",
                "removeDirectory",
                "remove a directory",
                36,
                false,
            )
            .with_row(matrix_row("removeDirectory")),
            DomainCommand::new(
                "files hardlink",
                "Files",
                "linkFile",
                "create a hard link",
                37,
                false,
            )
            .with_row(matrix_row("linkFile")),
            DomainCommand::new(
                "files realpath",
                "Files",
                "resolveSymlink",
                "resolve a path's symbolic links",
                38,
                false,
            )
            .with_row(matrix_row("resolveSymlink")),
            DomainCommand::new(
                "files pwd",
                "Files",
                "getCurrentDirectory",
                "the current working directory",
                39,
                true,
            )
            .with_row(matrix_row("getCurrentDirectory")),
            DomainCommand::new(
                "files cd",
                "Files",
                "changeDirectory",
                "change the current working directory",
                40,
                false,
            )
            .with_row(matrix_row("changeDirectory")),
            DomainCommand::new(
                "files version",
                "Files",
                "getFileVersion",
                "one version of a path",
                41,
                false,
            )
            .with_row(matrix_row("getFileVersion")),
            DomainCommand::new(
                "files version restore",
                "Files",
                "restoreVersion",
                "restore one version of a path",
                42,
                false,
            )
            .with_row(matrix_row("restoreVersion")),
            DomainCommand::new(
                "files version compare",
                "Files",
                "compareVersions",
                "compare two versions of a path",
                43,
                false,
            )
            .with_row(matrix_row("compareVersions")),
            DomainCommand::new(
                "files by-hash",
                "Files",
                "getByHash",
                "the content reference of a hash",
                44,
                false,
            )
            .with_row(matrix_row("getByHash")),
            DomainCommand::new(
                "files ephemeral",
                "Files",
                "createEphemeral",
                "create an ephemeral file",
                45,
                false,
            )
            .with_row(matrix_row("createEphemeral")),
            DomainCommand::new(
                "files encrypted",
                "Files",
                "createEncrypted",
                "create an encrypted file",
                46,
                false,
            )
            .with_row(matrix_row("createEncrypted")),
            DomainCommand::new(
                "files decrypt",
                "Files",
                "decryptFile",
                "decrypt a file",
                47,
                false,
            )
            .with_row(matrix_row("decryptFile")),
            DomainCommand::new(
                "files encryption-info",
                "Files",
                "getEncryptionInfo",
                "a file's encryption info",
                48,
                false,
            )
            .with_row(matrix_row("getEncryptionInfo")),
            DomainCommand::new(
                "files compression set",
                "Files",
                "setCompression",
                "set a path's compression",
                49,
                false,
            )
            .with_row(matrix_row("setCompression")),
            DomainCommand::new(
                "files compression",
                "Files",
                "getCompression",
                "a path's compression",
                50,
                false,
            )
            .with_row(matrix_row("getCompression")),
            DomainCommand::new(
                "files dedupe-stats",
                "Files",
                "getDeduplicationStats",
                "a path's deduplication statistics",
                51,
                false,
            )
            .with_row(matrix_row("getDeduplicationStats")),
            DomainCommand::new(
                "files batch copy",
                "Files",
                "batchCopy",
                "copy many paths at once",
                52,
                false,
            )
            .with_row(matrix_row("batchCopy")),
            DomainCommand::new(
                "files batch move",
                "Files",
                "batchMove",
                "move many paths at once",
                53,
                false,
            )
            .with_row(matrix_row("batchMove")),
            DomainCommand::new(
                "files batch delete",
                "Files",
                "batchDelete",
                "delete many paths at once",
                54,
                false,
            )
            .with_row(matrix_row("batchDelete")),
            DomainCommand::new(
                "files size",
                "Files",
                "getDirectorySize",
                "the size of a directory tree",
                55,
                false,
            )
            .with_row(matrix_row("getDirectorySize")),
            DomainCommand::new(
                "files stats",
                "Files",
                "getDirectoryStats",
                "the statistics of a directory tree",
                56,
                false,
            )
            .with_row(matrix_row("getDirectoryStats")),
            DomainCommand::new(
                "files compare-dirs",
                "Files",
                "compareDirectories",
                "compare two directory trees",
                57,
                false,
            )
            .with_row(matrix_row("compareDirectories")),
            DomainCommand::new(
                "files activity",
                "Files",
                "getFileActivity",
                "the recorded activity on a path",
                58,
                false,
            )
            .with_row(matrix_row("getFileActivity")),
            DomainCommand::new(
                "files type",
                "Files",
                "getFileType",
                "a path's type",
                59,
                false,
            )
            .with_row(matrix_row("getFileType")),
            DomainCommand::new(
                "files checksum",
                "Files",
                "getFileChecksum",
                "a path's stored checksum",
                60,
                false,
            )
            .with_row(matrix_row("getFileChecksum")),
            DomainCommand::new(
                "files lifecycle set",
                "Files",
                "setLifecycle",
                "set a path's lifecycle policy",
                61,
                false,
            )
            .with_row(matrix_row("setLifecycle")),
            DomainCommand::new(
                "files lifecycle",
                "Files",
                "getLifecycle",
                "a path's lifecycle policy",
                62,
                false,
            )
            .with_row(matrix_row("getLifecycle")),
            DomainCommand::new(
                "files lifecycle apply",
                "Files",
                "applyLifecycle",
                "apply a path's lifecycle policy",
                63,
                false,
            )
            .with_row(matrix_row("applyLifecycle")),
            DomainCommand::composing(
                "files audit",
                "Files",
                "subscribeFileAudit",
                "the audit trail, delivered live",
                64,
                false,
                "pull,cancel",
            )
            .with_row(matrix_row("subscribeFileAudit")),
            DomainCommand::new(
                "files analyze",
                "Files",
                "analyzeFileContent",
                "analyse a file's content",
                65,
                false,
            )
            .with_row(matrix_row("analyzeFileContent")),
            DomainCommand::new(
                "files branch",
                "Files",
                "branchVersion",
                "branch one version of a path",
                66,
                false,
            )
            .with_row(matrix_row("branchVersion")),
            DomainCommand::new(
                "files branch merge",
                "Files",
                "mergeBranch",
                "merge a branch back into a path",
                67,
                false,
            )
            .with_row(matrix_row("mergeBranch")),
            DomainCommand::new(
                "files trash restore",
                "Files",
                "restoreTrash",
                "restore one entry from the trash",
                68,
                false,
            )
            .with_row(matrix_row("restoreTrash")),
            DomainCommand::new(
                "files trash empty",
                "Files",
                "emptyTrash",
                "empty the trash",
                69,
                false,
            )
            .with_row(matrix_row("emptyTrash")),
            DomainCommand::new(
                "files recents clear",
                "Files",
                "clearRecentFiles",
                "clear the recently-used record",
                70,
                false,
            )
            .with_row(matrix_row("clearRecentFiles")),
            DomainCommand::new(
                "files quota set",
                "Files",
                "setQuota",
                "set a filesystem's quota",
                71,
                false,
            )
            .with_row(matrix_row("setQuota")),
            DomainCommand::new(
                "files snapshot",
                "Files",
                "createSnapshot",
                "create a snapshot of a path",
                72,
                false,
            )
            .with_row(matrix_row("createSnapshot")),
            DomainCommand::new(
                "files acquire",
                "Files",
                "acquireFile",
                "acquire the capability over one path",
                73,
                false,
            )
            .with_row(matrix_row("acquireFile")),
            // ── The bound file service ────────────────────────────────────
            DomainCommand::new(
                "files service open",
                "FileService",
                "open",
                "open the bound path",
                74,
                false,
            )
            .with_row(matrix_row("FileService.open")),
            DomainCommand::new(
                "files service read",
                "FileService",
                "read",
                "read the bound path",
                75,
                false,
            )
            .with_row(matrix_row("FileService.read")),
            DomainCommand::new(
                "files service write",
                "FileService",
                "write",
                "write the bound path",
                76,
                false,
            )
            .with_row(matrix_row("FileService.write")),
            DomainCommand::new(
                "files service stat",
                "FileService",
                "stat",
                "stat the bound path",
                77,
                true,
            )
            .with_row(matrix_row("FileService.stat")),
            DomainCommand::new(
                "files service close",
                "FileService",
                "close",
                "close the bound path's descriptor",
                78,
                true,
            )
            .with_row(matrix_row("FileService.close")),
            // ── The subscriptions ─────────────────────────────────────────
            DomainCommand::new(
                "files watch pull",
                "FileEventSubscription",
                "pull",
                "take the file events that have arrived",
                79,
                true,
            )
            .with_row(matrix_row("FileEventSubscription.pull")),
            DomainCommand::new(
                "files watch cancel",
                "FileEventSubscription",
                "cancel",
                "retire a file-event subscription",
                80,
                false,
            )
            .with_row(matrix_row("FileEventSubscription.cancel")),
            DomainCommand::new(
                "files audit pull",
                "FileAuditSubscription",
                "pull",
                "take the audit entries that have arrived",
                81,
                true,
            )
            .with_row(matrix_row("FileAuditSubscription.pull")),
            DomainCommand::new(
                "files audit cancel",
                "FileAuditSubscription",
                "cancel",
                "retire an audit subscription",
                82,
                false,
            )
            .with_row(matrix_row("FileAuditSubscription.cancel")),
        ]
    }

    /// The files schema's node tree: what a client that never compiled
    /// `files.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(env!("OUT_DIR"), "/schemas/files.schema.bin")))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = FilesService::new(Arc::clone(&self.backend), class);
        let client: fs::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
