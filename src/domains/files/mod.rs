//! The **files** domain: the anchor of batch 4 — the host's own files and
//! directories, over one object-safe [`FilesBackend`] trait with the frozen
//! `files.capnp` surface (`openFile @0` through `acquireFile @73`, the
//! `FileService` capability service, and the two subscription interfaces —
//! 83 contract rows in all, `files.capnp` having absorbed `filesystem.capnp`).
//!
//! What this domain is: the contract's complete file surface, served from the
//! host's own mechanisms rather than from a `cp -r` subprocess (the archive
//! body's condemned past) or from a `Statfs` guess.
//!
//! ## The mechanisms on this backend (law #9)
//!
//! | contract surface | native mechanism |
//! |---|---|
//! | `openFile`/`closeFile`/`readFile`/`writeFile`/`flushFile` | the host's own descriptors: `openat`, `mmap` + `pread`-window for the read hot path, `pwrite` at the offset, `fsync` |
//! | `statFile`/`getFileType`/`getFileMetadata` | `statx(2)`, so `created` is a real value and inode/device/link-count are reported data |
//! | `listDirectory`/`walkTree`/`getDirectoryStats` | `getdents64` through the directory handle, with a stated depth bound |
//! | `watchDirectory` | `inotify` — event-driven, no polling thread |
//! | `getDiskUsage` | `statvfs` plus the mount's own row from `/proc/mounts` |
//! | `getFileHash`/`getFileChecksum` | SHA-256 implemented over the file's own bytes |
//! | `setCompression`/`setLifecycle`/`getFileChecksum` | the file's own user xattrs, the host's own attribute namespace |
//! | versioning / content addressing / dedupe | a content-addressed store beside the host's own cache root |
//! | `getDefaultApp` | the host's own XDG `mimeapps.list`/`mimeinfo.cache` |
//! | `listTrash`/`getRecentFiles` | the host's own trash directory and recent-files record |
//! | `acquireFile` | a capability mint, and a `FileService` bound to that path alone |
//!
//! What is DEGRADED, and why: the version store and the branches inside it are
//! the *backend's own* (the host carries no VCS metadata for a path it never put
//! under one, and the reason says so per row); deduplication measures the
//! host's own inode sharing and this host's filesystem has no reflink; the
//! quota read stops exactly where this uid's privilege does; the activity and
//! audit trails are this backend's own records because the host runs no
//! auditd. `analyzeFileContent` and the three encryption methods are
//! `notApplicable` with a measured reason — no OCR engine and no key custody
//! are reachable on this host, and a guessed key or an invented analysis is the
//! fabrication this library exists to prevent.
//!
//! Law #8: every host write on this domain (create/remove/copy/rename/write,
//! restore, empty, merge, apply, setQuota) is code-complete, gate-ready and
//! never performed by a build or a test. The test suites are read-only against
//! the host and mutating against the backend's own store under a temporary
//! directory.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Files;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, FILES_AVAILABILITY,
    FILES_SCHEMA_ID, MethodAvailability, ReasonClass,
};
pub use backend::{AuditSubscription, FilesBackend, WatchSubscription};
pub use types::{
    AuditAction, BatchCopyRequest, BatchDeleteRequest, BatchMoveRequest, BatchResult, BoundPath,
    BranchResult, ChecksumResult, ComparisonResult, ContentAddressedRef, ContentRefResult,
    ContentSearchQuery, ContentSearchResult, CreateEncryptedRequest, DefaultAppInfo, DefaultAppResult,
    DirectoryEntry, DirectoryFilter, DirectoryStats, DiskUsage, EncryptedFileInfo,
    EncryptionInfoResult, EphemeralFileConfig, ExtendedMetadata, FileActivity, FileAnalysisRequest,
    FileAnalysisResult, FileAuditEntry, FileAuditFilter, FileBranch, FileCompressionConfig,
    FileDeduplicationInfo, FileEvent, FileEventKind, FileHandle, FileHashResult, FileInfo,
    FileInfoResult, FileKind, FileLock, FileSearchQuery, FileSearchResult, FileTypeCount,
    FileVersion, FilesystemHealth, FilesystemHealthResult, FilesystemQuota, GetByHashRequest,
    LifecyclePolicy, LifecyclePolicyResult, LockFileRequest, LockKind, Metadata, OpenFlag,
    OpenMode, OpenRequest, Page, Pagination, RecentFile, RetentionAction, SnapshotRefResult,
    SortField, SortOrder, SpecialPath, TextResult, TrashEntry, VersionResult,
};
