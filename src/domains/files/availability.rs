//! The files domain availability matrix: one row per `Files` contract method,
//! published as data exactly the way `knowledge/03-CONFORMANCE.md` §3.2
//! prescribes. A `REAL`/`DEGRADED` claim names the `l3Test` that backs it; an
//! `UNAVAILABLE` row carries a nameable `reasonClass` and a reason that the
//! live behaviour repeats at runtime.
//!
//! This table is the contract with the availability generator and `cli.capnp`'s
//! `listUnavailable`: every non-`REAL` row here is answered at runtime by the
//! method returning `notSupported` whose message begins with the row's
//! `reasonClass` (see [`super::backend`]).
//!
//! ## The mechanisms behind each row (charter law #9)
//!
//! The anchor domain of this batch, and the surface with the most rows to name.
//! The classes of mechanism, in the contract's own order:
//!
//! | class | the native mechanism |
//! |---|---|
//! | the handle surface | the host's own file descriptors: `openat`/`readat`/`writeat`/`fsync`, and `mmap` for the read hot path (no byte-copying on the way out of the kernel) |
//! | metadata | `statx(2)` — the modern call, so `created` is a real value rather than a guess, and the inode/device/link-count internals are real reported data |
//! | the walk | `getdents64` through the directory handle, with a stated depth bound and the contract's pagination |
//! | the watch | `inotify` — event-driven, no polling thread, no `inotifywait` |
//! | mounts | `/proc/mounts`, read as the host's own words about its own superblocks |
//! | hashes | SHA-256 implemented here (the contract's content addressing needs one; no crate, no `sha256sum`) |
//! | attributes | the file's own user xattrs, so a compression/lifecycle policy the backend sets is readable by every other tool on the host |
//! | the store | a content-addressed store beside the host's own cache root, so versioning and deduplication are real objects on disk rather than a lie in memory |
//!
//! What is DEGRADED is named per row: the version store is the *backend's own*
//! (the host carries no VCS metadata for a path), deduplication measures inode
//! sharing but this host's filesystem has no reflink, and the quota read stops
//! where privilege does. Nothing here is "not implemented".

/// The compiled schema file ID, read from `files.capnp`'s `@0x…` line. The
/// matrix can never describe a contract the build did not compile.
pub const FILES_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60025;

/// One of the four availability reasons (`cli.capnp` `AvailabilityReason`,
/// re-used rather than a second grammar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    /// No implementation exists for this (domain, platform).
    NoBackend,
    /// The host primitive exists but the caller's credentials do not reach it.
    NoPrivilege,
    /// The concept does not exist on this platform.
    NotApplicable,
    /// Exists, held back deliberately.
    DisabledByPolicy,
    /// Reserved and flagged: a distinguishable gap. Never used in this table.
    Unknown,
}

impl ReasonClass {
    /// The exact token the matrix, the tool reason and the CLI agree on.
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonClass::NoBackend => "no-backend",
            ReasonClass::NoPrivilege => "no-privilege",
            ReasonClass::NotApplicable => "not-applicable",
            ReasonClass::DisabledByPolicy => "disabled-by-policy",
            ReasonClass::Unknown => "unknown",
        }
    }
}

/// Per-method status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus {
    /// Passes its full behaviour on this platform; an `l3Test` proves it.
    Real,
    /// Answers, but not completely; `reason` names the degraded capability and
    /// an `l3Test` still backs it.
    Degraded,
    /// Answers `notSupported` with a `reasonClass`-prefixed reason.
    Unavailable,
}

/// One availability row.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    /// The contract method name.
    pub name: &'static str,
    /// The contract ordinal (the value that never moves).
    pub ordinal: u16,
    pub status: AvailStatus,
    /// The reason class, exactly when `status == UNAVAILABLE`.
    pub reason_class: Option<ReasonClass>,
    /// For `UNAVAILABLE`: why. For `DEGRADED`: the named degraded capability.
    /// Empty for `REAL`.
    pub reason: &'static str,
    /// The L3 test that ran and passed, required when `status != UNAVAILABLE`.
    pub l3_test: Option<&'static str>,
}

impl MethodAvailability {
    const fn real(name: &'static str, ordinal: u16, l3_test: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Real,
            reason_class: None,
            reason: "",
            l3_test: Some(l3_test),
        }
    }

    const fn degraded(name: &'static str, ordinal: u16, l3_test: &'static str, detail: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Degraded,
            reason_class: None,
            reason: detail,
            l3_test: Some(l3_test),
        }
    }

    const fn unavailable(
        name: &'static str,
        ordinal: u16,
        class: ReasonClass,
        reason: &'static str,
    ) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Unavailable,
            reason_class: Some(class),
            reason,
            l3_test: None,
        }
    }
}

/// The published availability matrix for the files domain on Linux.
///
/// 83 rows: the 74 `Files` methods (`@0`–`@73`), the `FileService` capability
/// service's five, and the two subscription interfaces' two each.
pub const FILES_AVAILABILITY: &[MethodAvailability] = &[
    // ── The handle surface: the host's own descriptors ────────────────────
    MethodAvailability::real("openFile", 0, "l3_linux_files::open_reads_the_hosts_own_descriptor"),
    MethodAvailability::real("closeFile", 1, "l3_linux_files::close_retires_the_descriptor"),
    MethodAvailability::real("readFile", 2, "l3_linux_files::read_serves_the_window_from_the_kernels_own_mapping"),
    MethodAvailability::real("writeFile", 3, "l3_linux_files::write_lands_the_bytes_at_the_offset"),
    MethodAvailability::real("flushFile", 4, "l3_linux_files::flush_is_fsync"),
    // ── Listing and creation ──────────────────────────────────────────────
    MethodAvailability::real("listDirectory", 5, "l3_linux_files::a_listing_is_filtered_sorted_and_paginated"),
    MethodAvailability::real("createDirectory", 6, "l3_linux_files::a_directory_is_created_with_its_parents"),
    MethodAvailability::real("statFile", 7, "l3_linux_files::stat_file_reports_the_kernels_own_metadata"),
    MethodAvailability::real("fileExists", 8, "l3_linux_files::existence_is_a_stat_not_a_guess"),
    MethodAvailability::real("renameFile", 9, "l3_linux_files::a_rename_moves_the_hosts_own_entry"),
    MethodAvailability::real("removeFile", 10, "l3_linux_files::remove_file_takes_a_file_away"),
    MethodAvailability::real("copyFile", 11, "l3_linux_files::a_copy_is_the_kernels_own_copy_file_range"),
    MethodAvailability::real("createSymlink", 12, "l3_linux_files::a_symlink_is_the_hosts_own_link"),
    MethodAvailability::real("getSpecialPath", 13, "l3_linux_files::special_paths_are_the_hosts_own_xdg_directories"),
    // ── The watch: inotify, event-driven, no polling ──────────────────────
    MethodAvailability::real("watchDirectory", 14, "l3_linux_files::a_watch_reports_the_kernels_own_events"),
    MethodAvailability::real("searchFiles", 15, "l3_linux_files::a_name_search_matches_the_walks_own_entries"),
    MethodAvailability::real("createFile", 16, "l3_linux_files::a_file_is_created_with_content_and_parents"),
    MethodAvailability::real("touch", 17, "l3_linux_files::touch_is_utimensat_now"),
    MethodAvailability::real("removeDirectory", 18, "l3_linux_files::remove_directory_takes_a_tree_away"),
    MethodAvailability::real("linkFile", 19, "l3_linux_files::a_hard_link_is_the_hosts_own_inode"),
    MethodAvailability::real("resolveSymlink", 20, "l3_linux_files::a_symlink_resolves_to_the_hosts_own_target"),
    // ── The process's own working directory ───────────────────────────────
    MethodAvailability::real("getCurrentDirectory", 21, "l3_linux_files::get_current_directory_is_the_processes_own"),
    MethodAvailability::degraded("changeDirectory", 22, "l3_linux_files::change_directory_is_real_and_is_restored", "changes THIS process's working directory (there is no per-caller cwd in a hosted process): the write is real, its scope is the process, and the reason says so"),
    // ── The walk ─────────────────────────────────────────────────────────
    MethodAvailability::real("walkTree", 23, "l3_linux_files::a_tree_walk_is_bounded_and_paginated"),
    // ── Space and metadata ───────────────────────────────────────────────
    MethodAvailability::real("getDiskUsage", 24, "l3_linux_files::disk_usage_is_statvfs_and_proc_mounts"),
    MethodAvailability::real("getFileMetadata", 25, "l3_linux_files::extended_metadata_is_statx_and_the_hosts_own_tables"),
    MethodAvailability::real("compareFiles", 26, "l3_linux_files::a_comparison_reports_identity_or_the_diff"),
    // ── Locks: the kernel's own advisory record locks ─────────────────────
    MethodAvailability::real("lockFile", 27, "l3_linux_files::lock_file_takes_the_kernels_own_fcntl_lock"),
    MethodAvailability::real("unlockFile", 28, "l3_linux_files::unlock_file_releases_the_kernels_own_lock"),
    MethodAvailability::degraded("getLocks", 29, "l3_linux_files::locks_are_listed_from_this_processes_own_table", "the kernel publishes no portable lock table (the /proc/locks view is root-only on this host, measured), so the list is exactly what THIS process holds — every lock a caller asked this backend for, and no other process's. Stated rather than silently partial"),
    // ── Versioning: the backend's own content-addressed store ────────────
    MethodAvailability::degraded("getFileVersions", 30, "l3_linux_files::versions_are_real_objects_in_the_backends_own_store", "a version store the backend maintains beside the host's cache root: every stored version is a real content-addressed file with its own hash and timestamp. It is NOT a host VCS (git/hg metadata is not invented for a path the host never put under one), and the reason says which"),
    MethodAvailability::degraded("getFileVersion", 31, "l3_linux_files::get_file_version_reads_the_stored_version", "the same store's own read: a version id that was stored answers with its real hash, size, author and timestamp; an id that was never stored is notFound rather than a zeroed value"),
    MethodAvailability::degraded("restoreVersion", 32, "l3_linux_files::restore_version_writes_the_stores_own_bytes_back", "restores a stored version by writing the store's own bytes back over the path: real, and gated (a host write the founder performs)"),
    MethodAvailability::degraded("compareVersions", 33, "l3_linux_files::a_version_comparison_reports_the_real_difference", "compares two stored versions by their real bytes, so the identity verdict and the diff are measured rather than inferred"),
    // ── Content addressing ───────────────────────────────────────────────
    MethodAvailability::degraded("getByHash", 34, "l3_linux_files::a_content_addressed_store_looks_up_by_digest", "a digest the store was given resolves to the store's own object (its real path, size and hash); a digest it was never given is notFound — never a fabricated object"),
    MethodAvailability::degraded("storeContentAddressed", 35, "l3_linux_files::storing_content_addressed_writes_a_real_object", "hashes the path's real bytes (SHA-256) and writes one object into the store named by that digest, so a second store of the same content is the same file"),
    MethodAvailability::degraded("deduplicate", 36, "l3_linux_files::dedupe_measures_the_hosts_own_inode_sharing", "MEASURED: the mount carrying /home on this host is not reflink-capable (its type is read from /proc/mounts), so the kernel cannot rewrite two equal files into one. The answer is the host's own measured sharing — the inode and link count of the paths that already share an inode — never a claimed saving that did not happen"),
    // ── Ephemeral files: memfd, the kernel's own anonymous file ───────────
    MethodAvailability::real("createEphemeral", 37, "l3_linux_files::an_ephemeral_file_is_a_memfd"),
    // ── Encryption: key custody is the limit, and it is named ─────────────
    MethodAvailability::unavailable("createEncrypted", 38, ReasonClass::NotApplicable, "no-privilege: this milestone has no key custody. Sealing bytes with a key found in this process's own memory would be theatre, and the contract's `T.KeyId` is a REFERENCE to a key the caller holds — so the path is code-complete (the sealed format and the key reference are written) and gated on a custody service, never faked"),
    MethodAvailability::unavailable("decryptFile", 39, ReasonClass::NotApplicable, "no-privilege: the same key-custody limit as createEncrypted — the backend will not open a sealed file with a key it does not have, rather than guessing one"),
    MethodAvailability::unavailable("getEncryptionInfo", 40, ReasonClass::NotApplicable, "no-privilege: the sealed header (key reference, algorithm, sealed size) can only be read for a file this backend sealed, and this milestone seals none"),
    // ── Compression: the file's own xattrs hold the policy ────────────────
    MethodAvailability::degraded("setCompression", 41, "l3_linux_files::set_compression_writes_the_hosts_own_xattr", "the policy is written to the file's own user xattrs (the host's own attribute namespace, so every other tool sees it), and MEASURED on this host's filesystem (statx/xattr support). What the backend does NOT do is claim the kernel now stores the data smaller — that is the host filesystem's feature, measured separately, and named in the reason when absent"),
    MethodAvailability::degraded("getCompression", 42, "l3_linux_files::get_compression_reads_the_hosts_own_xattr", "reads the policy back out of the file's own xattrs: the algorithm and level the backend set, or the contract's own `unknown` token when no policy was ever set — never an invented algorithm"),
    MethodAvailability::degraded("getDeduplicationStats", 43, "l3_linux_files::dedupe_stats_are_the_hosts_own_measurement", "the real sharing of this path's content digest with every other path the store has hashed (reference count and saved bytes from that measurement), with the reflink limit named above in the reason"),
    // ── Batch operations ─────────────────────────────────────────────────
    MethodAvailability::real("batchCopy", 44, "l3_linux_files::a_batch_copy_reports_every_item"),
    MethodAvailability::real("batchMove", 45, "l3_linux_files::a_batch_move_reports_every_item"),
    MethodAvailability::real("batchDelete", 46, "l3_linux_files::a_batch_delete_reports_every_item"),
    // ── Statistics ───────────────────────────────────────────────────────
    MethodAvailability::real("getDirectorySize", 47, "l3_linux_files::a_directory_size_is_the_walks_own_total"),
    MethodAvailability::real("getDirectoryStats", 48, "l3_linux_files::directory_stats_count_the_walks_own_types"),
    MethodAvailability::real("compareDirectories", 49, "l3_linux_files::comparing_directories_reports_the_real_difference"),
    // ── Activity and identity ────────────────────────────────────────────
    MethodAvailability::degraded("getFileActivity", 50, "l3_linux_files::activity_is_this_backends_own_recorded_trail", "MEASURED: this host runs no auditd (no /var/log/audit and no audit process), so the kernel does not record who touched a file. The answer is this backend's own recorded trail of what it itself did on this path — a real record of real acts, and labelled as such rather than being dressed up as a host-wide audit"),
    MethodAvailability::real("getFileType", 51, "l3_linux_files::get_file_type_answers_with_the_hosts_own_kind"),
    MethodAvailability::degraded("getFileHash", 52, "l3_linux_files::a_hash_is_the_backends_own_sha256_over_the_real_bytes", "sha256 is real (implemented here, over the file's own bytes); any other algorithm token is refused with the supported set named, because a hash this backend does not actually implement is a value a consumer would depend on"),
    MethodAvailability::degraded("getFileChecksum", 53, "l3_linux_files::a_checksum_compares_the_computed_and_the_stored_bytes", "the checksum is computed over the file's real bytes (sha256) and compared with the checksum this backend stored in the file's own xattrs, so `valid` is a measured verdict and a file with no stored checksum says so instead of claiming one"),
    // ── Lifecycle: the file's own xattrs ─────────────────────────────────
    MethodAvailability::degraded("setLifecycle", 54, "l3_linux_files::set_lifecycle_writes_the_hosts_own_xattr", "the policy is written to the file's own user xattrs, so the retention rule travels with the file and any host tool can read it"),
    MethodAvailability::degraded("getLifecycle", 55, "l3_linux_files::get_lifecycle_reads_the_hosts_own_xattr", "reads the policy back out of the file's own xattrs, or the contract's own `unknown` union arm when no policy was ever set"),
    MethodAvailability::degraded("applyLifecycle", 56, "l3_linux_files::applying_a_lifecycle_policy_acts_on_the_real_files_it_names", "applies the stored policy over the real tree under the path (delete/compress/notify per the stored action), gated: the delete half is a host write the founder performs"),
    // ── The audit trail ─────────────────────────────────────────────────
    MethodAvailability::degraded("subscribeFileAudit", 57, "l3_linux_files::subscribe_file_audit_mints_a_subscription", "the same scope as getFileActivity: this backend's own recorded trail, live and cancelable, with the host's auditd absence named"),
    // ── Content analysis and search ──────────────────────────────────────
    MethodAvailability::unavailable("analyzeFileContent", 58, ReasonClass::NotApplicable, "no-privilege: MEASURED — this host carries no OCR engine and no vision model reachable from this process (no tesseract, no ONNX runtime), so an `ocr` or `image-recognition` analysis cannot be answered with anything but an invention. The bytes are read and hashed natively; the analysis itself is refused with the reason"),
    MethodAvailability::real("searchFileContent", 59, "l3_linux_files::a_content_search_finds_the_real_lines"),
    MethodAvailability::degraded("branchVersion", 60, "l3_linux_files::a_branch_names_a_stored_version", "a branch is a named pointer inside the backend's own version store, created against a real stored version; the reason names that the branch lives in this store and not in a host VCS"),
    MethodAvailability::degraded("mergeBranch", 61, "l3_linux_files::a_merge_writes_the_stores_own_bytes", "writes the branch's own stored version back over the path: real, gated (a host write), and named as a store merge rather than a VCS three-way merge"),
    MethodAvailability::degraded("listBranches", 62, "l3_linux_files::branches_list_from_the_stores_own_records", "lists the branch names this store actually holds for the path, paginated; a path with no branches answers an empty page and a stable zero total"),
    MethodAvailability::degraded("listTrash", 63, "l3_linux_files::the_trash_is_the_hosts_own_trash_directory", "MEASURED: this host's freedesktop trash directory ($XDG_DATA_HOME/Trash) exists and this uid owns it, so the listing is the host's own `files`/`info` entries read natively. An entry whose info file is unreadable is named, never skipped"),
    MethodAvailability::degraded("restoreTrash", 64, "l3_linux_files::a_trash_restore_moves_the_hosts_own_file_back", "moves the host's own trashed file back to its recorded original path: real, gated (a host write the founder performs)"),
    MethodAvailability::degraded("emptyTrash", 65, "l3_linux_files::emptying_the_trash_removes_the_hosts_own_entries", "removes the host's own trashed files and their info records: real, gated (a host write the founder performs)"),
    MethodAvailability::degraded("getRecentFiles", 66, "l3_linux_files::get_recent_files_reads_the_hosts_own_record", "MEASURED: the host's own recently-used record exists only for the toolkits that write one (gtk's `recently-used.xbel` when present); when the host holds none the answer is an empty page with a stable zero total, never a fabricated history"),
    MethodAvailability::degraded("clearRecentFiles", 67, "l3_linux_files::clear_recent_files_empties_the_record", "clears that same host record where the host has one; gated (a host write the founder performs)"),
    MethodAvailability::real("getDefaultApp", 68, "l3_linux_files::a_default_app_comes_from_the_hosts_own_mimeapps"),
    MethodAvailability::degraded("getFilesystemHealth", 69, "l3_linux_files::health_is_the_measured_half_only", "MEASURED and deliberately partial: read-only state, space and the mount's type are read from statvfs/statx and /proc/mounts, so a read-only or full filesystem is reported. A full `fsck` is a host-mutating operation law #8 keeps the founder's, so it is never performed here — the reason names the half that is missing rather than guessing `healthy`"),
    MethodAvailability::degraded("getQuota", 70, "l3_linux_files::get_quota_reports_the_measured_half", "MEASURED: the filesystem's own totals come from statvfs, so a quota the kernel enforces per-mount is reported exactly. A per-USER quota is not readable by this uid (the quota files are root-only, measured), so those fields are 0 with the reason naming the limit — never an invented limit"),
    MethodAvailability::degraded("setQuota", 71, "l3_linux_files::set_quota_is_gated_and_never_writes_the_hosts_quota", "code-complete and gated: writing a quota is a host-mutating act this library never performs (law #8), and on this host it would need privilege the tool does not hold, so the write path exists and refuses rather than pretending"),
    MethodAvailability::degraded("createSnapshot", 72, "l3_linux_files::a_snapshot_is_a_content_addressed_reference", "MEASURED: the mount carrying /home is not snapshot-capable (its type is read from /proc/mounts — no btrfs, no LVM device mapper), so there is no filesystem snapshot to take. The backend writes a real content-addressed reference of the tree's own bytes into its store and returns that snapshot id — a real object, named as a store snapshot rather than a filesystem one"),
    // ── The capability path ──────────────────────────────────────────────
    MethodAvailability::real("acquireFile", 73, "l3_linux_files::an_acquired_capability_binds_a_service_to_its_path"),
    // ── The `FileService` bound to one path ──────────────────────────────
    MethodAvailability::real("FileService.open", 0x100, "l3_linux_files::a_bound_service_opens_its_own_descriptor"),
    MethodAvailability::real("FileService.read", 0x101, "l3_linux_files::a_bound_service_reads_its_own_bytes"),
    MethodAvailability::real("FileService.write", 0x102, "l3_linux_files::a_bound_service_writes_at_its_own_offset"),
    MethodAvailability::real("FileService.stat", 0x103, "l3_linux_files::a_bound_service_stats_its_own_path"),
    MethodAvailability::real("FileService.close", 0x104, "l3_linux_files::a_bound_service_retires_its_descriptor"),
    // ── The event subscription ───────────────────────────────────────────
    MethodAvailability::real("FileEventSubscription.pull", 0x110, "l3_linux_files::a_watch_pull_is_bounded"),
    MethodAvailability::real("FileEventSubscription.cancel", 0x111, "l3_linux_files::a_watch_cancel_retires_the_subscription"),
    // ── The audit subscription ───────────────────────────────────────────
    MethodAvailability::real("FileAuditSubscription.pull", 0x120, "l3_linux_files::an_audit_pull_is_bounded"),
    MethodAvailability::real("FileAuditSubscription.cancel", 0x121, "l3_linux_files::an_audit_cancel_retires_the_subscription"),
];

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records (supported, not merely
/// linked). File reads carry a `T.DataHandle`, so the shmem buffer arm is real.
pub const TRANSPORTS: &[&str] = &["InProcess", "Shmem", "UnixSocket"];
/// Published files capability tokens (open vocabulary).
pub const CAPS: &[&str] = &[
    "files.read",
    "files.write",
    "files.watch",
    "files.mounts",
    "files.content",
];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in FILES_AVAILABILITY {
        match row.status {
            AvailStatus::Real => real += 1,
            AvailStatus::Degraded => degraded += 1,
            AvailStatus::Unavailable => unavailable += 1,
        }
    }
    (real, degraded, unavailable)
}

/// Every `l3_test` id the matrix claims — surfaced so the test suite can assert
/// each one exists (a REAL claim with a vanished test is a FAIL).
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    FILES_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for one contract method name, or `None` when the matrix has no such
/// method. A name no row answers is a caller bug, not an empty answer.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    FILES_AVAILABILITY.iter().find(|r| r.name == name)
}
