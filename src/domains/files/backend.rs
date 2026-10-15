//! The `FilesBackend` trait: one object-safe method per `Files` contract
//! method (`openFile @0` through `acquireFile @73`), plus the `FileService`
//! capability service and the two subscription interfaces, each returning
//! [`crate::core::Result`].
//!
//! ## Capability story (law #5)
//!
//! There is exactly one permission-shaped gate in the whole contract:
//! `acquireFile @73` takes a `Capability` whose target is a path and hands back
//! a `FileService` bound to that path and to nothing else. Holding that
//! reference IS the authority to operate on the path — no per-method token, no
//! permission name, no ACL. Every other method borrows `&self` and returns owned
//! values, so the trait is object-safe and dispatchable as `dyn FilesBackend`.
//!
//! ## The honest default surface
//!
//! Each method has a default body answering `notSupported` **with a
//! `reasonClass`-prefixed reason** — the law #6 baseline a platform backend
//! overrides exactly where it serves something. A default that answered
//! `Ok(Default)` instead would be the fabrication this library exists to
//! prevent: a caller that opened a file on a host with no filesystem would
//! otherwise get an empty handle and a successful read of nothing.

use alloc::string::String;
use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{
    BatchCopyRequest, BatchDeleteRequest, BatchMoveRequest, ChecksumResult, ComparisonResult,
    ContentRefResult, ContentSearchQuery, ContentSearchResult, CreateEncryptedRequest,
    DirectoryEntry, DirectoryFilter, DirectoryStats, EncryptionInfoResult, EphemeralFileConfig,
    ExtendedMetadata, FileAnalysisRequest, FileAnalysisResult, FileAuditEntry, FileAuditFilter,
    FileCompressionConfig, FileDeduplicationInfo, FileEvent, FileHashResult, FileHandle,
    FileInfoResult, FileKind, FileLock, FileSearchQuery, FileSearchResult, FileVersion, GetByHashRequest,
    LifecyclePolicy, LifecyclePolicyResult, LockFileRequest, OpenRequest,
    Page, Pagination, SpecialPath, TextResult, TrashEntry, BoundPath, BranchResult, FileBranch,
    RecentFile, DefaultAppResult, FilesystemHealthResult, FilesystemQuota, SnapshotRefResult,
    VersionResult,
};

/// A subscription over the events of a watched directory. Holding the returned
/// id IS the authority to pull — there is no subscription id to present.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WatchSubscription(pub u64);

/// A subscription over the audit trail. Same authority rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AuditSubscription(pub u64);

/// A protocol-compliant files backend: one method per contract method, with an
/// honest `notSupported`-with-reason default for everything a host cannot serve.
pub trait FilesBackend {
    // ── The handle surface ────────────────────────────────────────────────
    /// `openFile @0`: open a path and hand back a handle over the host's own
    /// descriptor.
    fn open_file(&self, _path: &str, _request: &OpenRequest) -> Result<FileHandle> {
        Err(Error::not_supported(
            "no-backend: opening a file handle is not served on this backend",
        ))
    }

    /// `closeFile @1`: retire a handle.
    fn close_file(&self, _handle: &FileHandle) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: closing a file handle is not served on this backend",
        ))
    }

    /// `readFile @2`: read a window, returned as `T.DataHandle` bytes.
    fn read_file(&self, _handle: &FileHandle, _offset: u64, _size: u32) -> Result<Vec<u8>> {
        Err(Error::not_supported(
            "no-backend: the file read path is not served on this backend",
        ))
    }

    /// `writeFile @3`: write a window at an offset, returning the byte count.
    fn write_file(&self, _handle: &FileHandle, _offset: u64, _data: &[u8]) -> Result<u32> {
        Err(Error::not_supported(
            "no-backend: the file write path is not served on this backend",
        ))
    }

    /// `flushFile @4`: flush a handle's bytes to the host's own storage.
    fn flush_file(&self, _handle: &FileHandle) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the file flush path is not served on this backend",
        ))
    }

    // ── Listing, creation, and the basic mutations ───────────────────────
    /// `listDirectory @5`.
    fn list_directory(&self, _path: &str, _filter: &DirectoryFilter) -> Result<Page<DirectoryEntry>> {
        Err(Error::not_supported(
            "no-backend: the directory listing path is not served on this backend",
        ))
    }

    /// `createDirectory @6`.
    fn create_directory(&self, _path: &str, _parents: bool) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: directory creation is not served on this backend",
        ))
    }

    /// `statFile @7`.
    fn stat_file(&self, _path: &str) -> Result<FileInfoResult> {
        Err(Error::not_supported(
            "no-backend: the stat path is not served on this backend",
        ))
    }

    /// `fileExists @8`.
    fn file_exists(&self, _path: &str) -> Result<bool> {
        Err(Error::not_supported(
            "no-backend: the existence path is not served on this backend",
        ))
    }

    /// `renameFile @9`.
    fn rename_file(&self, _old_path: &str, _new_path: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the rename path is not served on this backend",
        ))
    }

    /// `removeFile @10`: remove a file, or a whole tree when `recursive`.
    fn remove_file(&self, _path: &str, _recursive: bool) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the remove path is not served on this backend",
        ))
    }

    /// `copyFile @11`.
    fn copy_file(&self, _src: &str, _dst: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the copy path is not served on this backend",
        ))
    }

    /// `createSymlink @12`.
    fn create_symlink(&self, _target: &str, _link: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the symlink path is not served on this backend",
        ))
    }

    /// `getSpecialPath @13`.
    fn get_special_path(&self, _path: SpecialPath) -> Result<TextResult> {
        Err(Error::not_supported(
            "no-backend: the special-path lookup is not served on this backend",
        ))
    }

    /// `watchDirectory @14`: hold the returned id to pull the kernel's events.
    fn watch_directory(&self, _path: &str) -> Result<WatchSubscription> {
        Err(Error::not_supported(
            "no-backend: the directory watch path is not served on this backend",
        ))
    }

    /// `pull` on a `FileEventSubscription`.
    fn pull_watch(&self, _subscription: WatchSubscription, _max_events: u32) -> Result<Vec<FileEvent>> {
        Err(Error::not_supported(
            "no-backend: the directory watch path is not served on this backend",
        ))
    }

    /// `cancel` on a `FileEventSubscription`.
    fn cancel_watch(&self, _subscription: WatchSubscription) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the directory watch path is not served on this backend",
        ))
    }

    /// `searchFiles @15`.
    fn search_files(&self, _query: &FileSearchQuery) -> Result<Page<FileSearchResult>> {
        Err(Error::not_supported(
            "no-backend: the file search path is not served on this backend",
        ))
    }

    /// `createFile @16`.
    fn create_file(&self, _path: &str, _content: &[u8], _parents: bool) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: file creation is not served on this backend",
        ))
    }

    /// `touch @17`.
    fn touch(&self, _path: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the touch path is not served on this backend",
        ))
    }

    /// `removeDirectory @18`.
    fn remove_directory(&self, _path: &str, _recursive: bool) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: directory removal is not served on this backend",
        ))
    }

    /// `linkFile @19` (hard when `hard`, symbolic otherwise).
    fn link_file(&self, _target: &str, _link: &str, _hard: bool) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the link path is not served on this backend",
        ))
    }

    /// `resolveSymlink @20`.
    fn resolve_symlink(&self, _path: &str) -> Result<TextResult> {
        Err(Error::not_supported(
            "no-backend: the symlink resolution path is not served on this backend",
        ))
    }

    /// `getCurrentDirectory @21`.
    fn get_current_directory(&self) -> Result<String> {
        Err(Error::not_supported(
            "no-backend: the working-directory read is not served on this backend",
        ))
    }

    /// `changeDirectory @22`.
    fn change_directory(&self, _path: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the working-directory write is not served on this backend",
        ))
    }

    /// `walkTree @23`.
    fn walk_tree(&self, _path: &str, _max_depth: u32, _page: Pagination) -> Result<Page<DirectoryEntry>> {
        Err(Error::not_supported(
            "no-backend: the tree walk is not served on this backend",
        ))
    }

    /// `getDiskUsage @24`.
    fn get_disk_usage(&self, _path: &str) -> Result<super::types::DiskUsage> {
        Err(Error::not_supported(
            "no-backend: the disk-usage read is not served on this backend",
        ))
    }

    /// `getFileMetadata @25`.
    fn get_file_metadata(&self, _path: &str) -> Result<ExtendedMetadata> {
        Err(Error::not_supported(
            "no-backend: the extended-metadata read is not served on this backend",
        ))
    }

    /// `compareFiles @26`.
    fn compare_files(&self, _path1: &str, _path2: &str) -> Result<ComparisonResult> {
        Err(Error::not_supported(
            "no-backend: the file comparison is not served on this backend",
        ))
    }

    /// `lockFile @27`.
    fn lock_file(&self, _request: &LockFileRequest) -> Result<FileLock> {
        Err(Error::not_supported(
            "no-backend: the file locking path is not served on this backend",
        ))
    }

    /// `unlockFile @28`.
    fn unlock_file(&self, _lock_id: u64) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the file unlocking path is not served on this backend",
        ))
    }

    /// `getLocks @29`.
    fn get_locks(&self, _path: &str, _page: Pagination) -> Result<Page<FileLock>> {
        Err(Error::not_supported(
            "no-backend: the lock listing is not served on this backend",
        ))
    }

    // ── Versioning ───────────────────────────────────────────────────────
    /// `getFileVersions @30`.
    fn get_file_versions(&self, _path: &str, _page: Pagination) -> Result<Page<FileVersion>> {
        Err(Error::not_supported(
            "no-backend: the version store is not served on this backend",
        ))
    }

    /// `getFileVersion @31`.
    fn get_file_version(&self, _path: &str, _version_id: u64) -> Result<VersionResult> {
        Err(Error::not_supported(
            "no-backend: the version store is not served on this backend",
        ))
    }

    /// `restoreVersion @32`.
    fn restore_version(&self, _path: &str, _version_id: u64) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the version store is not served on this backend",
        ))
    }

    /// `compareVersions @33`.
    fn compare_versions(&self, _path: &str, _version_a: u64, _version_b: u64) -> Result<ComparisonResult> {
        Err(Error::not_supported(
            "no-backend: the version comparison is not served on this backend",
        ))
    }

    /// `branchVersion @60`.
    fn branch_version(&self, _path: &str, _version_id: u64, _branch_name: &str) -> Result<BranchResult> {
        Err(Error::not_supported(
            "no-backend: the version store is not served on this backend",
        ))
    }

    /// `mergeBranch @61`.
    fn merge_branch(&self, _path: &str, _branch_name: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the version store is not served on this backend",
        ))
    }

    /// `listBranches @62`.
    fn list_branches(&self, _path: &str, _page: Pagination) -> Result<Page<FileBranch>> {
        Err(Error::not_supported(
            "no-backend: the version store is not served on this backend",
        ))
    }

    // ── Content addressing ───────────────────────────────────────────────
    /// `getByHash @34`.
    fn get_by_hash(&self, _request: &GetByHashRequest) -> Result<ContentRefResult> {
        Err(Error::not_supported(
            "no-backend: the content-addressed store is not served on this backend",
        ))
    }

    /// `storeContentAddressed @35`.
    fn store_content_addressed(&self, _path: &str) -> Result<ContentRefResult> {
        Err(Error::not_supported(
            "no-backend: the content-addressed store is not served on this backend",
        ))
    }

    /// `deduplicate @36`.
    fn deduplicate(&self, _path: &str) -> Result<(u64, u32)> {
        Err(Error::not_supported(
            "no-backend: the deduplication path is not served on this backend",
        ))
    }

    /// `getDeduplicationStats @43`.
    fn get_deduplication_stats(&self, _path: &str) -> Result<FileDeduplicationInfo> {
        Err(Error::not_supported(
            "no-backend: the deduplication path is not served on this backend",
        ))
    }

    // ── Ephemeral and encrypted files ────────────────────────────────────
    /// `createEphemeral @37`.
    fn create_ephemeral(&self, _path: &str, _config: &EphemeralFileConfig) -> Result<FileHandle> {
        Err(Error::not_supported(
            "no-backend: the ephemeral-file path is not served on this backend",
        ))
    }

    /// `createEncrypted @38`.
    fn create_encrypted(&self, _request: &CreateEncryptedRequest) -> Result<FileHandle> {
        Err(Error::not_supported(
            "no-privilege: no key custody is reachable on this backend",
        ))
    }

    /// `decryptFile @39`.
    fn decrypt_file(&self, _path: &str, _key_id: crate::core::ids::KeyId) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: no key custody is reachable on this backend",
        ))
    }

    /// `getEncryptionInfo @40`.
    fn get_encryption_info(&self, _path: &str) -> Result<EncryptionInfoResult> {
        Err(Error::not_supported(
            "no-privilege: no key custody is reachable on this backend",
        ))
    }

    // ── Compression ──────────────────────────────────────────────────────
    /// `setCompression @41`.
    fn set_compression(&self, _path: &str, _config: &FileCompressionConfig) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the compression-policy path is not served on this backend",
        ))
    }

    /// `getCompression @42`.
    fn get_compression(&self, _path: &str) -> Result<FileCompressionConfig> {
        Err(Error::not_supported(
            "no-backend: the compression-policy path is not served on this backend",
        ))
    }

    // ── Batch operations ─────────────────────────────────────────────────
    /// `batchCopy @44`.
    fn batch_copy(&self, _request: &BatchCopyRequest) -> Result<super::types::BatchResult> {
        Err(Error::not_supported(
            "no-backend: the batch copy path is not served on this backend",
        ))
    }

    /// `batchMove @45`.
    fn batch_move(&self, _request: &BatchMoveRequest) -> Result<super::types::BatchResult> {
        Err(Error::not_supported(
            "no-backend: the batch move path is not served on this backend",
        ))
    }

    /// `batchDelete @46`.
    fn batch_delete(&self, _request: &BatchDeleteRequest) -> Result<super::types::BatchResult> {
        Err(Error::not_supported(
            "no-backend: the batch delete path is not served on this backend",
        ))
    }

    // ── Statistics ───────────────────────────────────────────────────────
    /// `getDirectorySize @47`.
    fn get_directory_size(&self, _path: &str) -> Result<u64> {
        Err(Error::not_supported(
            "no-backend: the directory-size read is not served on this backend",
        ))
    }

    /// `getDirectoryStats @48`.
    fn get_directory_stats(&self, _path: &str) -> Result<DirectoryStats> {
        Err(Error::not_supported(
            "no-backend: the directory-stats read is not served on this backend",
        ))
    }

    /// `compareDirectories @49`.
    fn compare_directories(&self, _path1: &str, _path2: &str) -> Result<ComparisonResult> {
        Err(Error::not_supported(
            "no-backend: the directory comparison is not served on this backend",
        ))
    }

    // ── Activity, identity, hashing ──────────────────────────────────────
    /// `getFileActivity @50`.
    fn get_file_activity(&self, _path: &str, _page: Pagination) -> Result<Page<super::types::FileActivity>> {
        Err(Error::not_supported(
            "no-backend: the activity trail is not served on this backend",
        ))
    }

    /// `getFileType @51`.
    fn get_file_type(&self, _path: &str) -> Result<FileKind> {
        Err(Error::not_supported(
            "no-backend: the type read is not served on this backend",
        ))
    }

    /// `getFileHash @52` (an open algorithm token).
    fn get_file_hash(&self, _path: &str, _algorithm: &str) -> Result<FileHashResult> {
        Err(Error::not_supported(
            "no-backend: the hashing path is not served on this backend",
        ))
    }

    /// `getFileChecksum @53`.
    fn get_file_checksum(&self, _path: &str) -> Result<ChecksumResult> {
        Err(Error::not_supported(
            "no-backend: the checksum path is not served on this backend",
        ))
    }

    // ── Lifecycle ────────────────────────────────────────────────────────
    /// `setLifecycle @54`.
    fn set_lifecycle(&self, _path: &str, _policy: &LifecyclePolicy) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the lifecycle-policy path is not served on this backend",
        ))
    }

    /// `getLifecycle @55`.
    fn get_lifecycle(&self, _path: &str) -> Result<LifecyclePolicyResult> {
        Err(Error::not_supported(
            "no-backend: the lifecycle-policy path is not served on this backend",
        ))
    }

    /// `applyLifecycle @56`.
    fn apply_lifecycle(&self, _path: &str) -> Result<u32> {
        Err(Error::not_supported(
            "no-backend: the lifecycle-policy path is not served on this backend",
        ))
    }

    // ── The audit trail ─────────────────────────────────────────────────
    /// `subscribeFileAudit @57`: hold the returned id to pull the trail.
    fn subscribe_audit(&self, _filter: &FileAuditFilter) -> Result<AuditSubscription> {
        Err(Error::not_supported(
            "no-backend: the audit trail is not served on this backend",
        ))
    }

    /// `pull` on a `FileAuditSubscription`.
    fn pull_audit(&self, _subscription: AuditSubscription, _max_entries: u32) -> Result<Vec<FileAuditEntry>> {
        Err(Error::not_supported(
            "no-backend: the audit trail is not served on this backend",
        ))
    }

    /// `cancel` on a `FileAuditSubscription`.
    fn cancel_audit(&self, _subscription: AuditSubscription) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the audit trail is not served on this backend",
        ))
    }

    // ── Content analysis and search ──────────────────────────────────────
    /// `analyzeFileContent @58` (open analysis tokens).
    fn analyze_file_content(&self, _request: &FileAnalysisRequest) -> Result<FileAnalysisResult> {
        Err(Error::not_supported(
            "no-privilege: no OCR or vision engine is reachable on this backend",
        ))
    }

    /// `searchFileContent @59`.
    fn search_file_content(&self, _query: &ContentSearchQuery) -> Result<Page<ContentSearchResult>> {
        Err(Error::not_supported(
            "no-backend: the content search path is not served on this backend",
        ))
    }

    // ── Trash, recents, default apps ─────────────────────────────────────
    /// `listTrash @63`.
    fn list_trash(&self, _page: Pagination) -> Result<Page<TrashEntry>> {
        Err(Error::not_supported(
            "no-backend: the trash read is not served on this backend",
        ))
    }

    /// `restoreTrash @64`.
    fn restore_trash(&self, _entry_id: &str, _target_path: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the trash path is not served on this backend",
        ))
    }

    /// `emptyTrash @65`.
    fn empty_trash(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the trash path is not served on this backend",
        ))
    }

    /// `getRecentFiles @66`.
    fn get_recent_files(&self, _page: Pagination) -> Result<Page<RecentFile>> {
        Err(Error::not_supported(
            "no-backend: the recent-files read is not served on this backend",
        ))
    }

    /// `clearRecentFiles @67`.
    fn clear_recent_files(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the recent-files path is not served on this backend",
        ))
    }

    /// `getDefaultApp @68`.
    fn get_default_app(&self, _mime_type: &str) -> Result<DefaultAppResult> {
        Err(Error::not_supported(
            "no-backend: the default-app read is not served on this backend",
        ))
    }

    // ── Health, quota, snapshots ────────────────────────────────────────
    /// `getFilesystemHealth @69`.
    fn get_filesystem_health(&self, _path: &str) -> Result<FilesystemHealthResult> {
        Err(Error::not_supported(
            "no-backend: the filesystem-health read is not served on this backend",
        ))
    }

    /// `getQuota @70`.
    fn get_quota(&self, _path: &str) -> Result<FilesystemQuota> {
        Err(Error::not_supported(
            "no-backend: the quota read is not served on this backend",
        ))
    }

    /// `setQuota @71`.
    fn set_quota(&self, _path: &str, _quota: &FilesystemQuota) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: writing a quota is a host-mutating act this backend never performs",
        ))
    }

    /// `createSnapshot @72`.
    fn create_snapshot(&self, _path: &str, _name: &str) -> Result<SnapshotRefResult> {
        Err(Error::not_supported(
            "no-backend: the snapshot path is not served on this backend",
        ))
    }

    // ── The capability path ─────────────────────────────────────────────
    /// `acquireFile @73`: present a capability whose target is a path and
    /// receive a service bound to that path and to nothing else.
    fn acquire_file(&self, _path: &str, _authority: u64) -> Result<BoundPath> {
        Err(Error::not_supported(
            "no-backend: the capability acquisition path is not served on this backend",
        ))
    }

    /// `FileService.open @0` on an already-bound path.
    fn bound_open(&self, _bound: &BoundPath, _request: &OpenRequest) -> Result<FileHandle> {
        Err(Error::not_supported(
            "no-backend: the capability service is not served on this backend",
        ))
    }

    /// `FileService.read @1`.
    fn bound_read(&self, _bound: &BoundPath, _offset: u64, _size: u32) -> Result<Vec<u8>> {
        Err(Error::not_supported(
            "no-backend: the capability service is not served on this backend",
        ))
    }

    /// `FileService.write @2`.
    fn bound_write(&self, _bound: &BoundPath, _offset: u64, _data: &[u8]) -> Result<u32> {
        Err(Error::not_supported(
            "no-backend: the capability service is not served on this backend",
        ))
    }

    /// `FileService.stat @3`.
    fn bound_stat(&self, _bound: &BoundPath) -> Result<FileInfoResult> {
        Err(Error::not_supported(
            "no-backend: the capability service is not served on this backend",
        ))
    }

    /// `FileService.close @4`.
    fn bound_close(&self, _bound: &BoundPath) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the capability service is not served on this backend",
        ))
    }

    /// Publish this backend's honest files capability tokens (law #6).
    fn files_caps(&self) -> Vec<&'static str> {
        alloc::vec!["files.read", "files.write", "files.watch", "files.mounts"]
    }
}
