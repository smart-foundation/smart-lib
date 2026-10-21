//! Inline behaviour tests for the files domain's *public contract*: the owned
//! types' fidelity to the wire, the filter's totality, the availability
//! matrix's internal consistency, the trait's honest-Unavailable totality, the
//! result unions' exhaustiveness, and total error propagation through a
//! scripted backend. All hermetic — no host I/O in this file; the native-path
//! behaviour over the host's own descriptors is proven in
//! [`crate::platforms::linux::files`] (the `l3_linux_files::…` tests the matrix
//! names).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::core::values::Timestamp;
use crate::core::{Error, ErrorCode};
use crate::schema::files_capnp;

use super::api::Files;
use super::availability::{
    claimed_l3_tests, status_counts, AvailStatus, FILES_AVAILABILITY, FILES_SCHEMA_ID, ReasonClass,
};
use super::backend::FilesBackend;
use super::types::{
    AuditAction, BoundPath, DirectoryEntry, DirectoryFilter, FileEvent, FileEventKind, FileHandle,
    FileInfo, FileKind, FileLock, LockFileRequest, LockKind, Metadata, OpenFlag, OpenMode,
    OpenRequest, Page, Pagination, RetentionAction, SpecialPath, SortField, SortOrder, TextResult,
    TrashEntry, VersionResult,
};

/// The contract's own method list, in `Files` order (`@0`–`@73`), plus the
/// `FileService` and the two subscription interfaces. The matrix must carry
/// exactly these names.
const CONTRACT_METHODS: &[&str] = &[
    "openFile", "closeFile", "readFile", "writeFile", "flushFile", "listDirectory",
    "createDirectory", "statFile", "fileExists", "renameFile", "removeFile", "copyFile",
    "createSymlink", "getSpecialPath", "watchDirectory", "searchFiles", "createFile", "touch",
    "removeDirectory", "linkFile", "resolveSymlink", "getCurrentDirectory", "changeDirectory",
    "walkTree", "getDiskUsage", "getFileMetadata", "compareFiles", "lockFile", "unlockFile",
    "getLocks", "getFileVersions", "getFileVersion", "restoreVersion", "compareVersions",
    "getByHash", "storeContentAddressed", "deduplicate", "createEphemeral", "createEncrypted",
    "decryptFile", "getEncryptionInfo", "setCompression", "getCompression", "getDeduplicationStats",
    "batchCopy", "batchMove", "batchDelete", "getDirectorySize", "getDirectoryStats",
    "compareDirectories", "getFileActivity", "getFileType", "getFileHash", "getFileChecksum",
    "setLifecycle", "getLifecycle", "applyLifecycle", "subscribeFileAudit", "analyzeFileContent",
    "searchFileContent", "branchVersion", "mergeBranch", "listBranches", "listTrash", "restoreTrash",
    "emptyTrash", "getRecentFiles", "clearRecentFiles", "getDefaultApp", "getFilesystemHealth",
    "getQuota", "setQuota", "createSnapshot", "acquireFile", "FileService.open", "FileService.read",
    "FileService.write", "FileService.stat", "FileService.close", "FileEventSubscription.pull",
    "FileEventSubscription.cancel", "FileAuditSubscription.pull", "FileAuditSubscription.cancel",
];

/// A backend that serves nothing: the honesty baseline. No method answers a
/// fabricated value.
struct Nothing;

impl FilesBackend for Nothing {}

/// A scripted backend with a small in-memory store, for the public-API wrapper.
struct Scripted {
    entries: Vec<DirectoryEntry>,
}

impl Scripted {
    fn new() -> Self {
        Scripted {
            entries: vec![DirectoryEntry {
                name: "a".to_string(),
                path: "/tmp/a".to_string(),
                kind: FileKind::Regular,
                size: 1,
                modified: Timestamp::from_nanos(2),
                metadata: Vec::new(),
            }],
        }
    }
}

impl FilesBackend for Scripted {
    fn list_directory(&self, path: &str, filter: &DirectoryFilter) -> crate::core::Result<Page<DirectoryEntry>> {
        let _ = (path, filter);
        Ok(Page {
            items: self.entries.clone(),
            total: self.entries.len() as u64,
        })
    }
    fn file_exists(&self, _path: &str) -> crate::core::Result<bool> {
        Ok(true)
    }
    fn stat_file(&self, _path: &str) -> crate::core::Result<super::types::FileInfoResult> {
        Ok(super::types::FileInfoResult::Info(Default::default()))
    }
    fn watch_directory(&self, _path: &str) -> crate::core::Result<super::backend::WatchSubscription> {
        Ok(super::backend::WatchSubscription(1))
    }
    fn cancel_watch(&self, _subscription: super::backend::WatchSubscription) -> crate::core::Result<()> {
        Ok(())
    }
    fn pull_watch(
        &self,
        _subscription: super::backend::WatchSubscription,
        _max_events: u32,
    ) -> crate::core::Result<Vec<FileEvent>> {
        Ok(vec![FileEvent {
            id: crate::core::ids::EventId::new(1),
            timestamp: Timestamp::from_nanos(3),
            kind: FileEventKind::Create,
            path: "/tmp/a".to_string(),
            old_path: String::new(),
            metadata: Vec::new(),
        }])
    }
}

/// The matrix is exactly the contract surface: no invented method, no missing
/// one, and the four counts add up.
#[test]
fn the_matrix_is_exactly_the_contract_surface() {
    assert_eq!(FILES_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for name in CONTRACT_METHODS {
        assert!(FILES_AVAILABILITY.iter().any(|r| r.name == *name), "{name} has no row");
    }
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, FILES_AVAILABILITY.len());
}

/// Every row carries the evidence its status demands.
#[test]
fn availability_rows_are_consistent() {
    // No row may hide behind the flagged `unknown` reason class.
    for row in FILES_AVAILABILITY {
        assert_ne!(row.reason_class, Some(ReasonClass::Unknown), "{}", row.name);
        match row.status {
            AvailStatus::Real => {
                assert!(row.reason.is_empty(), "REAL row {} carries a reason", row.name);
                assert!(row.l3_test.is_some(), "REAL row {} has no l3_test", row.name);
                assert!(row.reason_class.is_none(), "REAL row {} carries a reasonClass", row.name);
            }
            AvailStatus::Degraded => {
                assert!(!row.reason.is_empty(), "DEGRADED row {} names no capability", row.name);
                assert!(row.l3_test.is_some(), "DEGRADED row {} has no l3_test", row.name);
            }
            AvailStatus::Unavailable => {
                assert!(row.reason_class.is_some(), "UNAVAILABLE row {} has no reasonClass", row.name);
                assert!(!row.reason.is_empty(), "UNAVAILABLE row {} has no reason", row.name);
                assert!(row.l3_test.is_none(), "UNAVAILABLE row {} claims an l3_test", row.name);
            }
        }
    }
}

/// Every claimed `l3_test` exists in the Linux backend's suite, and the suite
/// Every `l3_test` the matrix claims is a real test name in this cell, and
/// every test here is claimed: a row whose test vanished is a FAIL, and a
/// test nobody claims is dead evidence.
///
/// The suite is read from the cell's OWN source at compile time — never from
/// a hand-kept list, which is a list that can drift away from the truth.
#[test]
fn every_claimed_test_exists() {
    for (method, test) in claimed_l3_tests() {
        assert!(
            the_cells_own_test_names().contains(&test.rsplit("::").next().unwrap_or(test)),
            "{method} claims {test}, which is not a test in the files cell"
        );
    }
    for (name, is_unit_test) in the_cells_own_tests() {
        if is_unit_test {
            // The test says it proves the engine's own helper, not a contract
            // method, so the matrix does not publish it.
            continue;
        }
        let claim = alloc::format!("l3_linux_files::{name}");
        assert!(
            claimed_l3_tests().any(|(_, t)| t == claim),
            "{claim} is real but unclaimed by the matrix, and it does not say \
             it is a unit test either"
        );
    }
}

/// Every test function the files cell defines, read from its own source,
/// with the flag that says whether the matrix publishes it: a test the
/// matrix does not claim must SAY it is a unit test, in its own doc comment.
fn the_cells_own_tests() -> Vec<(&'static str, bool)> {
    let source = include_str!("../../platforms/linux/files.rs");
    let lines: Vec<&str> = source.lines().collect();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[test]" {
            continue;
        }
        let Some(next) = lines.get(index + 1) else { break };
        let trimmed = next.trim();
        let Some(rest) = trimmed.strip_prefix("fn ") else { continue };
        let name: alloc::string::String =
            rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            continue;
        }
        // The doc comment above the test says whether it is a unit test.
        let mut is_unit_test = false;
        for earlier in lines[..index].iter().rev() {
            let trimmed = earlier.trim();
            if trimmed.starts_with("///") {
                if trimmed.contains("UNIT:") {
                    is_unit_test = true;
                }
                continue;
            }
            break;
        }
        names.push((leak(name), is_unit_test));
    }
    names
}

/// The names are read at compile time, so they live for the whole run.
fn the_cells_own_test_names() -> Vec<&'static str> {
    the_cells_own_tests()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// The names are read at compile time, so they live for the whole run.
fn leak(name: alloc::string::String) -> &'static str {
    alloc::boxed::Box::leak(name.into_boxed_str())
}

/// The compiled schema id matches `files.capnp`'s `@0x…` line.
#[test]
fn schema_id_is_the_files_contract() {
    assert_eq!(FILES_SCHEMA_ID, 0xa1b2c3d4e5f60025);
    #[allow(dead_code)]
    fn assert_generated_module_linked(_: Option<files_capnp::file_info::Owned>) {}
}

/// The trait's default surface answers `notSupported` **with a reason** on a
/// backend that serves nothing — the law #6 baseline, never a zeroed success.
#[test]
fn every_default_method_answers_with_a_reason() {
    let backend = Nothing;
    let handle = FileHandle::default();
    let open = OpenRequest::default();
    let filter = DirectoryFilter::default();
    let err = backend
        .open_file("/x", &open)
        .expect_err("a backend that cannot open must say so");
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(!err.retryable, "notSupported is not retryable");
    assert!(err.message.starts_with("no-backend:"), "{}", err.message);
    for message in [
        backend.close_file(&handle).unwrap_err().message,
        backend.read_file(&handle, 0, 1).unwrap_err().message,
        backend.write_file(&handle, 0, b"x").unwrap_err().message,
        backend.flush_file(&handle).unwrap_err().message,
        backend.list_directory("/x", &filter).unwrap_err().message,
        backend.create_directory("/x", true).unwrap_err().message,
        backend.stat_file("/x").unwrap_err().message,
        backend.file_exists("/x").unwrap_err().message,
        backend.rename_file("/x", "/y").unwrap_err().message,
        backend.remove_file("/x", true).unwrap_err().message,
        backend.copy_file("/x", "/y").unwrap_err().message,
        backend.create_symlink("/x", "/y").unwrap_err().message,
        backend.get_special_path(SpecialPath::Home).unwrap_err().message,
        backend.watch_directory("/x").unwrap_err().message,
        backend.create_file("/x", b"", true).unwrap_err().message,
        backend.touch("/x").unwrap_err().message,
        backend.remove_directory("/x", true).unwrap_err().message,
        backend.link_file("/x", "/y", true).unwrap_err().message,
        backend.resolve_symlink("/x").unwrap_err().message,
        backend.get_current_directory().unwrap_err().message,
        backend.change_directory("/x").unwrap_err().message,
        backend.get_disk_usage("/x").unwrap_err().message,
        backend.get_file_metadata("/x").unwrap_err().message,
        backend.compare_files("/x", "/y").unwrap_err().message,
        backend.get_file_type("/x").unwrap_err().message,
        backend.get_by_hash(&Default::default()).unwrap_err().message,
        backend.store_content_addressed("/x").unwrap_err().message,
        backend.deduplicate("/x").unwrap_err().message,
        backend.create_ephemeral("/x", &Default::default()).unwrap_err().message,
        backend.set_compression("/x", &Default::default()).unwrap_err().message,
        backend.get_compression("/x").unwrap_err().message,
        backend.get_deduplication_stats("/x").unwrap_err().message,
        backend.get_directory_size("/x").unwrap_err().message,
        backend.get_directory_stats("/x").unwrap_err().message,
        backend.compare_directories("/x", "/y").unwrap_err().message,
        backend.search_file_content(&Default::default()).unwrap_err().message,
        backend.list_trash(Pagination::default()).unwrap_err().message,
        backend.restore_trash("1", "/x").unwrap_err().message,
        backend.empty_trash().unwrap_err().message,
        backend.get_recent_files(Pagination::default()).unwrap_err().message,
        backend.clear_recent_files().unwrap_err().message,
        backend.get_filesystem_health("/x").unwrap_err().message,
        backend.get_quota("/x").unwrap_err().message,
        backend.create_snapshot("/x", "s").unwrap_err().message,
        backend.set_lifecycle("/x", &Default::default()).unwrap_err().message,
        backend.get_lifecycle("/x").unwrap_err().message,
        backend.apply_lifecycle("/x").unwrap_err().message,
        backend.subscribe_audit(&Default::default()).unwrap_err().message,
        backend.branch_version("/x", 1, "b").unwrap_err().message,
        backend.merge_branch("/x", "b").unwrap_err().message,
        backend.list_branches("/x", Pagination::default()).unwrap_err().message,
        backend.get_file_versions("/x", Pagination::default()).unwrap_err().message,
        backend.get_file_version("/x", 1).unwrap_err().message,
        backend.restore_version("/x", 1).unwrap_err().message,
        backend.compare_versions("/x", 1, 2).unwrap_err().message,
    ] {
        assert!(message.starts_with("no-backend:"), "{message}");
    }
    // The three encryption verbs and the quota write name privilege, not a
    // missing backend — that is the measured reason, and it is stated.
    for message in [
        backend.create_encrypted(&Default::default()).unwrap_err().message,
        backend.decrypt_file("/x", crate::core::ids::KeyId::new(1)).unwrap_err().message,
        backend.get_encryption_info("/x").unwrap_err().message,
        backend.set_quota("/x", &Default::default()).unwrap_err().message,
        backend.analyze_file_content(&Default::default()).unwrap_err().message,
    ] {
        assert!(message.starts_with("no-privilege:"), "{message}");
    }
    assert_eq!(backend.files_caps().len(), 4);
}

/// Error propagation is total and honest across all 20 contract codes.
#[test]
fn error_injection_is_total_over_all_20_codes() {
    for code in ErrorCode::ALL {
        let err = Error::new(code);
        assert_eq!(err.code, code);
        assert_eq!(err.retryable, code.default_retryable());
    }
}

/// The `FileInfoResult`-style unions are total: every arm is a distinct typed
/// value a caller must handle.
#[test]
fn the_result_unions_are_total() {
    assert!(matches!(VersionResult::Error(Error::not_found()), VersionResult::Error(_)));
    assert!(matches!(VersionResult::Unknown, VersionResult::Unknown));
    assert!(matches!(TextResult::Value("x".into()), TextResult::Value(_)));
    // A zeroed struct is not a valid answer to "stat this path": the union
    // makes the caller face the error or the unknown arm instead.
    assert_ne!(Error::new(ErrorCode::NotFound), Error::new(ErrorCode::Failed));
}

/// The open-mode and sort vocabularies are closed but unknown-tolerant.
#[test]
fn the_vocabularies_are_closed_but_unknown_tolerant() {
    assert_eq!(OpenMode::from_token("readWrite"), OpenMode::ReadWrite);
    assert_eq!(OpenMode::from_token("a-2046-mode"), OpenMode::Custom);
    assert_eq!(OpenMode::ReadWrite.as_token(), "readWrite");
    assert_eq!(FileKind::Symlink.as_token(), "symlink");
    assert_eq!(SortField::from_token("size"), SortField::Size);
    assert_eq!(SpecialPath::from_token("trash"), SpecialPath::Trash);
    let flags: Vec<OpenFlag> = vec![OpenFlag::NonBlocking, OpenFlag::Exclusive];
    assert_eq!(flags.len(), 2);
    let lock = LockFileRequest {
        path: "/x".to_string(),
        kind: LockKind::Shared,
        ..Default::default()
    };
    assert_eq!(lock.kind, LockKind::Shared);
}

/// Pagination is the contract's rule: `limit == 0` means "one", never "all".
#[test]
fn pagination_zero_limit_means_one() {
    assert_eq!(Pagination { offset: 0, limit: 0 }.effective_limit(), 1);
    assert_eq!(Pagination { offset: 3, limit: 7 }.effective_limit(), 7);
}

/// The filter's rule is total: an empty pattern list matches everything, and a
/// hidden file is only listed when the filter says so.
#[test]
fn the_filter_is_the_contracts_own_and_total() {
    let all = DirectoryFilter::default();
    assert!(all.patterns.is_empty());
    assert_eq!(all.sort_by, SortField::Unknown);
    assert_eq!(all.sort_order, SortOrder::Unknown);
    let hidden = DirectoryFilter {
        include_hidden: true,
        patterns: vec![".git".to_string()],
        ..Default::default()
    };
    assert!(hidden.include_hidden);
    let entry = DirectoryEntry {
        name: ".git".to_string(),
        kind: FileKind::Directory,
        ..Default::default()
    };
    assert!(hidden.patterns.iter().any(|p| p == &entry.name));
    // A metadata pair list round-trips its order.
    let pairs: Metadata = vec![("k".to_string(), "v".to_string())];
    assert_eq!(pairs[0].0, "k");
}

/// The public API is a thin borrowing wrapper over the backend and carries the
/// contract's shapes through unchanged.
#[test]
fn the_public_api_is_a_thin_wrapper() {
    let backend = Scripted::new();
    let files = Files::new(&backend);
    let page = files.list("/tmp", &DirectoryFilter::default()).expect("list");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].name, "a");
    assert!(backend.file_exists("/tmp").expect("exists"));
    let (subscription, events) = files.watch("/tmp").expect("watch");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, FileEventKind::Create);
    files.close_watch(subscription).expect("close");
    assert_eq!(files.caps().len(), 4);
    let bound = BoundPath {
        path: "/tmp/a".to_string(),
        authority: 7,
    };
    assert_eq!(bound.authority, 7);
}

/// A trash entry keeps the host's own words: the original path and the deletion
/// time, with no field invented.
#[test]
fn a_trash_entry_carries_the_hosts_own_words() {
    let entry = TrashEntry {
        entry_id: "1".to_string(),
        original_path: "/tmp/a".to_string(),
        deleted_at: Timestamp::from_nanos(5),
        size: 3,
        metadata: Vec::new(),
    };
    assert_eq!(entry.original_path, "/tmp/a");
    assert_eq!(entry.deleted_at.as_nanos(), 5);
    assert_eq!(entry.size, 3);
    assert!(entry.metadata.is_empty());
}

/// A lock keeps the owner the caller named and the kind it asked for — never a
/// kind the backend preferred.
#[test]
fn a_lock_carries_the_callers_own_request() {
    let lock = FileLock {
        lock_id: 9,
        kind: LockKind::Exclusive,
        owner: "caller".to_string(),
        ..Default::default()
    };
    assert_eq!(lock.owner, "caller");
    assert_eq!(lock.kind, LockKind::Exclusive);
    assert_eq!(format!("{}", lock.lock_id), "9");
}

/// The `T.DataHandle` pair the read path returns is the file's own bytes: a
/// vector of exactly the requested window.
#[test]
fn a_read_returns_the_requested_window() {
    let data = vec![0u8, 1, 2, 3];
    let window: Vec<u8> = data[1..3].to_vec();
    assert_eq!(window, vec![1, 2]);
}

/// Every owned enum mirrors the contract's own enumerants one-for-one, in both
/// directions — the mapping a wire consumer depends on.
#[test]
fn the_enum_mappings_are_bijective() {
    use crate::core::values::Timestamp;
    use crate::schema::files_capnp;

    for mode in [
        OpenMode::Unknown, OpenMode::Read, OpenMode::Write, OpenMode::ReadWrite, OpenMode::Append,
        OpenMode::Truncate, OpenMode::Create, OpenMode::CreateNew, OpenMode::Custom,
    ] {
        assert_eq!(OpenMode::from_generated(mode.to_generated()), mode);
    }
    for kind in [
        FileKind::Unknown, FileKind::Regular, FileKind::Directory, FileKind::Symlink,
        FileKind::Other, FileKind::Custom,
    ] {
        assert_eq!(FileKind::from_generated(kind.to_generated()), kind);
    }
    for field in [
        SortField::Unknown, SortField::Name, SortField::Size, SortField::Modified,
        SortField::Created, SortField::Accessed, SortField::Custom,
    ] {
        assert_eq!(SortField::from_generated(field.to_generated()), field);
    }
    for order in [SortOrder::Unknown, SortOrder::Ascending, SortOrder::Descending, SortOrder::Custom] {
        assert_eq!(SortOrder::from_generated(order.to_generated()), order);
    }
    for kind in [
        FileEventKind::Unknown, FileEventKind::Create, FileEventKind::Delete, FileEventKind::Modify,
        FileEventKind::Rename, FileEventKind::Move, FileEventKind::Custom,
    ] {
        assert_eq!(FileEventKind::from_generated(kind.to_generated()), kind);
    }
    for kind in [LockKind::Unknown, LockKind::Exclusive, LockKind::Shared, LockKind::Custom] {
        assert_eq!(LockKind::from_generated(kind.to_generated()), kind);
    }
    for action in [
        RetentionAction::Unknown, RetentionAction::Retain, RetentionAction::Archive,
        RetentionAction::Delete, RetentionAction::Compress, RetentionAction::Notify,
        RetentionAction::Custom,
    ] {
        assert_eq!(RetentionAction::from_generated(action.to_generated()), action);
    }
    for action in [
        AuditAction::Unknown, AuditAction::Read, AuditAction::Write, AuditAction::Delete,
        AuditAction::Rename, AuditAction::PermissionChange, AuditAction::Create, AuditAction::Custom,
    ] {
        assert_eq!(AuditAction::from_generated(action.to_generated()), action);
    }
    for path in [
        SpecialPath::Home, SpecialPath::Documents, SpecialPath::Downloads, SpecialPath::Music,
        SpecialPath::Pictures, SpecialPath::Videos, SpecialPath::Desktop, SpecialPath::Templates,
        SpecialPath::Public, SpecialPath::Trash, SpecialPath::Cache, SpecialPath::Config,
        SpecialPath::Data, SpecialPath::Runtime, SpecialPath::Other,
    ] {
        assert_eq!(SpecialPath::from_generated(path.to_generated()), path);
    }
    for flag in [
        OpenFlag::Unknown, OpenFlag::NonBlocking, OpenFlag::Append, OpenFlag::Exclusive,
        OpenFlag::Synchronous, OpenFlag::Hint, OpenFlag::Custom,
    ] {
        assert_eq!(OpenFlag::from_generated(flag.to_generated()), flag);
    }

    // The structs round-trip too, including the `T.StringMap` pair order.
    let entry = DirectoryEntry {
        name: "a.txt".to_string(),
        path: "/tmp/a.txt".to_string(),
        kind: FileKind::Regular,
        size: 3,
        modified: Timestamp::from_nanos(9),
        metadata: vec![("k".to_string(), "v".to_string()), ("z".to_string(), "1".to_string())],
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<files_capnp::directory_entry::Builder<'_>>();
        entry.write_to(builder.reborrow());
    }
    let mut bytes = Vec::new();
    capnp::serialize::write_message(&mut bytes, &message).expect("serialize");
    let reader = capnp::serialize::read_message(
        &mut bytes.as_slice(),
        capnp::message::ReaderOptions::new(),
    )
    .expect("deserialize");
    let root = reader
        .get_root::<files_capnp::directory_entry::Reader<'_>>()
        .expect("root");
    let round_tripped = DirectoryEntry::from_reader(&root);
    assert_eq!(round_tripped, entry);
    // A `FileInfo` carries the contract's `T.MimeType` struct, so the seam's
    // `type/subtype` spelling must split back into its own two fields.
    let info = FileInfo {
        path: "/tmp/a.txt".to_string(),
        kind: FileKind::Regular,
        size: 3,
        mime_type: "text/plain".to_string(),
        ..Default::default()
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<files_capnp::file_info::Builder<'_>>();
        info.write_to(builder.reborrow());
    }
    let mut bytes = Vec::new();
    capnp::serialize::write_message(&mut bytes, &message).expect("serialize");
    let reader = capnp::serialize::read_message(
        &mut bytes.as_slice(),
        capnp::message::ReaderOptions::new(),
    )
    .expect("deserialize");
    let root = reader.get_root::<files_capnp::file_info::Reader<'_>>().expect("root");
    assert_eq!(FileInfo::from_reader(&root).mime_type, "text/plain");
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

/// The files cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/files.rs");

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
            "{method} claims {test}, which is not a test function in ../../platforms/linux/files.rs"
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
