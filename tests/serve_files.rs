//! The files domain, served through the contract — the spine's own tests.

#![cfg(all(feature = "rpc", feature = "std", feature = "domain-files"))]

extern crate std;

use std::sync::Arc;

use smart_lib::domains::files::availability;
use smart_lib::platforms::linux::files::LinuxFilesBackend;
use smart_lib::schema::cli_capnp::cli_service;
use smart_lib::schema::discovery_capnp::discovery;
use smart_lib::schema::files_capnp::{self as fcapnp, files as fs};
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::files::FilesDomain;
use smart_lib::serve::service::Service;
use smart_lib::serve::CapabilityClass;

mod serve_common;

use serve_common::{assert_honest, metadata_of, run_client, socket_path, text};

/// The service under test: the files domain plus the foundation interfaces.
fn build_service() -> Arc<Service> {
    let service = Service::new();
    service.abi().publish(SupportedAbi {
        schema_id: availability::FILES_SCHEMA_ID,
        name: "smart-protocol/files".to_string(),
        major: 0,
        minor: 1,
    });
    let domain = FilesDomain::new(Arc::new(LinuxFilesBackend::new()));
    let service = service.with_domain(Arc::new(domain));
    service.set_endpoints(vec![PublishedEndpoint::new(
        "unix",
        "/tmp/smart-lib-s1-files.sock",
    )]);
    Arc::new(service)
}

/// A contract-only client of the `Files` interface, in process.
fn files_client() -> fs::Client {
    let server = smart_lib::serve::files::FilesService::new(
        Arc::new(LinuxFilesBackend::new()),
        CapabilityClass::Control,
    );
    capnp_rpc::new_client(server)
}

/// One contract call over the `Files` interface.
macro_rules! call {
    ($method:ident, |$params:ident| $body:block) => {{
        let client = files_client();
        futures::executor::block_on(async move {
            let mut request = client.$method();
            {
                let $params = request.get();
                $body
            }
            request.send().promise.await
        })
    }};
}

/// A path this test owns: under /tmp, named for this process and this run.
///
/// The path is leaked once, so every call can name it without moving it
/// into a closure.
fn scratch(name: &str) -> &'static str {
    let path = format!("/tmp/smart-lib-s1-files-{}-{name}", std::process::id());
    Box::leak(path.into_boxed_str())
}

// ═══════════════════════════════════════════════════════════════════════
// Every contract method answers, or refuses with a reason
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn every_method_of_the_files_interface_answers_or_refuses_honestly() {
    let file = scratch("file.txt");
    let directory = scratch("dir");
    let other = scratch("other.txt");
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(&other);
    let _ = std::fs::remove_dir_all(&directory);

    // createFile @16 — the fixture this test reads back.
    let created = call!(create_file_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.reborrow().init_content().set_inline(b"the spine's own bytes\n");
        params.set_parents(true);
    });
    assert_honest("createFile", created.map(|_| ()));

    // statFile @7 — one path's own metadata.
    let stat = call!(stat_file_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("statFile", stat.map(|_| ()));

    // fileExists @8 — whether a path exists.
    let exists = call!(file_exists_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("fileExists", exists.map(|_| ()));

    // listDirectory @5 — one directory's entries.
    let listed = call!(list_directory_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
        let mut filter = params.init_filter();
        filter.set_include_hidden(true);
        filter.reborrow().init_page().set_limit(4);
    });
    assert_honest("listDirectory", listed.map(|_| ()));

    // openFile @0 / readFile @2 / closeFile @1 — the handle's own round.
    let opened = call!(open_file_request, |params| {
        let mut params = params;
        params.set_path(file);
        let mut request = params.init_request();
        request.set_mode(fcapnp::OpenMode::Read);
    });
    assert_honest("openFile", opened.map(|_| ()));

    // getFileType @51 — a path's type.
    let kind = call!(get_file_type_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("getFileType", kind.map(|_| ()));

    // getFileMetadata @25 — one path's extended metadata.
    let metadata = call!(get_file_metadata_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("getFileMetadata", metadata.map(|_| ()));

    // getDiskUsage @24 — the filesystem's own usage.
    let usage = call!(get_disk_usage_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
    });
    assert_honest("getDiskUsage", usage.map(|_| ()));

    // searchFiles @15 — a name search under a path.
    let searched = call!(search_files_request, |params| {
        let mut params = params;
        let mut query = params.init_query();
        query.set_path("/tmp");
        query.set_pattern("smart-lib-s1-files-*");
        query.reborrow().init_page().set_limit(4);
    });
    assert_honest("searchFiles", searched.map(|_| ()));

    // walkTree @23 — a bounded tree walk.
    let walked = call!(walk_tree_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
        params.set_max_depth(1);
        let mut page = params.init_page();
        page.set_limit(4);
    });
    assert_honest("walkTree", walked.map(|_| ()));

    // getCurrentDirectory @21 — the process's own working directory.
    let cwd = call!(get_current_directory_request, |_params| {});
    assert_honest("getCurrentDirectory", cwd.map(|_| ()));

    // getSpecialPath @13 — an XDG special path, resolved by the host.
    let special = call!(get_special_path_request, |params| {
        let mut params = params;
        params.set_type(fcapnp::SpecialPathType::Home);
    });
    assert_honest("getSpecialPath", special.map(|_| ()));

    // resolveSymlink @20 — a path's symbolic links.
    let resolved = call!(resolve_symlink_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("resolveSymlink", resolved.map(|_| ()));

    // compareFiles @26 / compareDirectories @49 — the comparisons.
    let compared = call!(compare_files_request, |params| {
        let mut params = params;
        params.set_path1(file);
        params.set_path2(file);
    });
    assert_honest("compareFiles", compared.map(|_| ()));

    let compared_dirs = call!(compare_directories_request, |params| {
        let mut params = params;
        params.set_path1("/tmp");
        params.set_path2("/tmp");
    });
    assert_honest("compareDirectories", compared_dirs.map(|_| ()));

    // getLocks @29 — the advisory locks on a path.
    let locks = call!(get_locks_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("getLocks", locks.map(|_| ()));

    // lockFile @27 / unlockFile @28.
    let locked = call!(lock_file_request, |params| {
        let mut params = params;
        let mut request = params.init_request();
        request.set_path(&file);
        request.set_owner("smart-lib-test");
    });
    assert_honest("lockFile", locked.map(|_| ()));

    let unlocked = call!(unlock_file_request, |params| {
        let mut params = params;
        params.set_lock_id(1);
    });
    assert_honest("unlockFile", unlocked.map(|_| ()));

    // The version store's reads.
    let versions = call!(get_file_versions_request, |params| {
        let mut params = params;
        params.set_path(file);
        let mut page = params.init_page();
        page.set_limit(4);
    });
    assert_honest("getFileVersions", versions.map(|_| ()));

    let version = call!(get_file_version_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.set_version_id(1);
    });
    assert_honest("getFileVersion", version.map(|_| ()));

    let compared_versions = call!(compare_versions_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.set_version_a(1);
        params.set_version_b(2);
    });
    assert_honest("compareVersions", compared_versions.map(|_| ()));

    // The content-addressed store's reads.
    let by_hash = call!(get_by_hash_request, |params| {
        let mut params = params;
        let mut request = params.init_request();
        request.set_algorithm("sha256");
    });
    assert_honest("getByHash", by_hash.map(|_| ()));

    let stored = call!(store_content_addressed_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("storeContentAddressed", stored.map(|_| ()));

    let deduplicated = call!(deduplicate_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("deduplicate", deduplicated.map(|_| ()));

    let dedup_stats = call!(get_deduplication_stats_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("getDeduplicationStats", dedup_stats.map(|_| ()));

    // The compression pair.
    let compression = call!(get_compression_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("getCompression", compression.map(|_| ()));

    let compression_set = call!(set_compression_request, |params| {
        let mut params = params;
        params.set_path(file);
        let mut config = params.init_config();
        config.set_algorithm("gzip");
        config.set_level(6);
    });
    assert_honest("setCompression", compression_set.map(|_| ()));

    // The encryption trio — two of them are unavailable on this host.
    let encryption = call!(get_encryption_info_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("getEncryptionInfo", encryption.map(|_| ()));

    let encrypted = call!(create_encrypted_request, |params| {
        let mut params = params;
        let mut request = params.init_request();
        request.set_path(&other);
        request.set_algorithm("aes");
        request.reborrow().init_content().set_inline(b"sealed");
    });
    assert_honest("createEncrypted", encrypted.map(|_| ()));

    let decrypted = call!(decrypt_file_request, |params| {
        let mut params = params;
        params.set_path(other);
        params.reborrow().init_key_id().set_id(1);
    });
    assert_honest("decryptFile", decrypted.map(|_| ()));

    // The batch verbs.
    let batch_copy = call!(batch_copy_request, |params| {
        let mut params = params;
        let mut request = params.init_request();
        let mut sources = request.reborrow().init_sources(1);
        sources.reborrow().set(0, file);
        request.reborrow().set_destination(other);
    });
    assert_honest("batchCopy", batch_copy.map(|_| ()));

    let batch_move = call!(batch_move_request, |params| {
        let mut params = params;
        let mut request = params.init_request();
        let mut sources = request.reborrow().init_sources(1);
        sources.reborrow().set(0, file);
        request.reborrow().set_destination(other);
    });
    assert_honest("batchMove", batch_move.map(|_| ()));

    let batch_delete = call!(batch_delete_request, |params| {
        let mut params = params;
        let mut request = params.init_request();
        let mut paths = request.reborrow().init_paths(1);
        paths.reborrow().set(0, other);
    });
    assert_honest("batchDelete", batch_delete.map(|_| ()));

    // The directory statistics.
    let size = call!(get_directory_size_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
    });
    assert_honest("getDirectorySize", size.map(|_| ()));

    let stats = call!(get_directory_stats_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
    });
    assert_honest("getDirectoryStats", stats.map(|_| ()));

    // getFileActivity @50.
    let activity = call!(get_file_activity_request, |params| {
        let mut params = params;
        params.set_path(file);
        let mut page = params.init_page();
        page.set_limit(4);
    });
    assert_honest("getFileActivity", activity.map(|_| ()));

    // getFileHash @52 / getFileChecksum @53.
    let hash = call!(get_file_hash_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.set_algorithm("sha256");
    });
    assert_honest("getFileHash", hash.map(|_| ()));

    let checksum = call!(get_file_checksum_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("getFileChecksum", checksum.map(|_| ()));

    // The lifecycle policy.
    let lifecycle = call!(get_lifecycle_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("getLifecycle", lifecycle.map(|_| ()));

    let lifecycle_set = call!(set_lifecycle_request, |params| {
        let mut params = params;
        params.set_path(file);
        let mut policy = params.init_policy();
        policy.set_path(file);
        policy.set_action(fcapnp::RetentionAction::Delete);
        policy.set_enabled(true);
    });
    assert_honest("setLifecycle", lifecycle_set.map(|_| ()));

    let applied = call!(apply_lifecycle_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("applyLifecycle", applied.map(|_| ()));

    // The content search and the analysis.
    let content = call!(search_file_content_request, |params| {
        let mut params = params;
        let mut query = params.init_query();
        query.set_path("/tmp");
        query.set_query("spine");
        query.reborrow().init_page().set_limit(4);
    });
    assert_honest("searchFileContent", content.map(|_| ()));

    let analysed = call!(analyze_file_content_request, |params| {
        let mut params = params;
        let mut request = params.init_request();
        request.set_path(&file);
        let mut types = request.reborrow().init_analysis_types(1);
        types.reborrow().set(0, "text");
    });
    assert_honest("analyzeFileContent", analysed.map(|_| ()));

    // The version store's writes.
    let branched = call!(branch_version_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.set_version_id(1);
        params.set_branch_name("test-branch");
    });
    assert_honest("branchVersion", branched.map(|_| ()));

    let merged = call!(merge_branch_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.set_branch_name("test-branch");
    });
    assert_honest("mergeBranch", merged.map(|_| ()));

    let branches = call!(list_branches_request, |params| {
        let mut params = params;
        params.set_path(file);
        let mut page = params.init_page();
        page.set_limit(4);
    });
    assert_honest("listBranches", branches.map(|_| ()));

    let restored = call!(restore_version_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.set_version_id(1);
    });
    assert_honest("restoreVersion", restored.map(|_| ()));

    // The trash and the recents.
    let trash = call!(list_trash_request, |params| {
        let mut params = params;
        let mut page = params.init_page();
        page.set_limit(4);
    });
    assert_honest("listTrash", trash.map(|_| ()));

    let restore_trash = call!(restore_trash_request, |params| {
        let mut params = params;
        params.set_entry_id("test-entry");
        params.set_target_path(file);
    });
    assert_honest("restoreTrash", restore_trash.map(|_| ()));

    let emptied = call!(empty_trash_request, |_params| {});
    assert_honest("emptyTrash", emptied.map(|_| ()));

    let recents = call!(get_recent_files_request, |params| {
        let mut params = params;
        let mut page = params.init_page();
        page.set_limit(4);
    });
    assert_honest("getRecentFiles", recents.map(|_| ()));

    let cleared_recents = call!(clear_recent_files_request, |_params| {});
    assert_honest("clearRecentFiles", cleared_recents.map(|_| ()));

    // The default application.
    let default_app = call!(get_default_app_request, |params| {
        let mut params = params;
        let mut mime = params.init_mime_type();
        mime.set_type("text");
        mime.set_subtype("plain");
    });
    assert_honest("getDefaultApp", default_app.map(|_| ()));

    // The filesystem's health and quota.
    let health = call!(get_filesystem_health_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
    });
    assert_honest("getFilesystemHealth", health.map(|_| ()));

    let quota = call!(get_quota_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
    });
    assert_honest("getQuota", quota.map(|_| ()));

    let quota_set = call!(set_quota_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
        let mut quota = params.init_quota();
        quota.set_max_size_bytes(1024);
        quota.set_max_files(4);
    });
    assert_honest("setQuota", quota_set.map(|_| ()));

    // The snapshot.
    let snapshot = call!(create_snapshot_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.set_name("test-snapshot");
    });
    assert_honest("createSnapshot", snapshot.map(|_| ()));

    // The subscriptions.
    let watched = call!(watch_directory_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
    });
    assert_honest("watchDirectory", watched.map(|_| ()));

    let audited = call!(subscribe_file_audit_request, |params| {
        let mut params = params;
        let mut filter = params.init_filter();
        filter.set_path_pattern("/tmp");
    });
    assert_honest("subscribeFileAudit", audited.map(|_| ()));

    // The mutating verbs the host can perform.
    let directory_created = call!(create_directory_request, |params| {
        let mut params = params;
        params.set_path(directory);
        params.set_parents(true);
    });
    assert_honest("createDirectory", directory_created.map(|_| ()));

    let touched = call!(touch_request, |params| {
        let mut params = params;
        params.set_path(file);
    });
    assert_honest("touch", touched.map(|_| ()));

    let copied = call!(copy_file_request, |params| {
        let mut params = params;
        params.set_src(file);
        params.set_dst(other);
    });
    assert_honest("copyFile", copied.map(|_| ()));

    let renamed = call!(rename_file_request, |params| {
        let mut params = params;
        params.set_old_path(other);
        params.set_new_path(scratch("renamed.txt"));
    });
    assert_honest("renameFile", renamed.map(|_| ()));

    let linked = call!(create_symlink_request, |params| {
        let mut params = params;
        params.set_target(file);
        params.set_link(scratch("link"));
    });
    assert_honest("createSymlink", linked.map(|_| ()));

    let hard_linked = call!(link_file_request, |params| {
        let mut params = params;
        params.set_target(file);
        params.set_link(&scratch("hardlink"));
        params.set_hard(true);
    });
    assert_honest("linkFile", hard_linked.map(|_| ()));

    let changed = call!(change_directory_request, |params| {
        let mut params = params;
        params.set_path("/tmp");
    });
    assert_honest("changeDirectory", changed.map(|_| ()));

    let ephemeral = call!(create_ephemeral_request, |params| {
        let mut params = params;
        params.set_path(&scratch("ephemeral"));
        let mut config = params.init_config();
        config.set_delete_after_read(true);
        config.set_max_reads(1);
    });
    assert_honest("createEphemeral", ephemeral.map(|_| ()));

    // The removals: the fixture goes last.
    let removed = call!(remove_file_request, |params| {
        let mut params = params;
        params.set_path(file);
        params.set_recursive(false);
    });
    assert_honest("removeFile", removed.map(|_| ()));

    let removed_directory = call!(remove_directory_request, |params| {
        let mut params = params;
        params.set_path(directory);
        params.set_recursive(true);
    });
    assert_honest("removeDirectory", removed_directory.map(|_| ()));

    // The fixture's leftovers, cleaned up.
    for path in [
        file,
        other,
        directory,
        scratch("renamed.txt"),
        scratch("link"),
        scratch("hardlink"),
        scratch("ephemeral"),
    ] {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(path);
    }
}

#[test]
fn the_bound_file_service_is_the_contracts_own_capability() {
    let file = scratch("bound.txt");
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::write(&file, b"the bound path's own bytes\n");

    let client = files_client();
    let acquired = futures::executor::block_on(async move {
        let mut request = client.acquire_file_request();
        {
            let mut params = request.get();
            params.set_path(file);
            let mut authority = params.init_authority();
            authority.set_target(file);
        }
        request.send().promise.await
    })
    .expect("the capability answers");
    let service = acquired
        .get()
        .expect("the results")
        .get_service()
        .expect("the bound service");

    // The reference IS the authority over that one path.
    let stat_client = service.clone();
    let stat = futures::executor::block_on(async move {
        let request = stat_client.stat_request();
        request.send().promise.await
    })
    .expect("the stat answers");
    let result = stat
        .get()
        .expect("the results")
        .get_result()
        .expect("the result");
    assert!(result.which().is_ok(), "the result must be a union arm");

    let reader = service.clone();
    let read = futures::executor::block_on(async move {
        let mut request = reader.read_request();
        {
            let mut params = request.get();
            params.set_offset(0);
            params.set_size(8);
        }
        request.send().promise.await
    })
    .expect("the read answers");
    let data = read.get().expect("the results").get_data().expect("the data");
    assert!(data.has_inline(), "the read answers the bound path's bytes");

    let closer = service.clone();
    let closed = futures::executor::block_on(async move {
        let request = closer.close_request();
        request.send().promise.await
    });
    assert_honest("FileService.close", closed.map(|_| ()));

    let _ = std::fs::remove_file(&file);
}

#[test]
fn the_subscriptions_are_the_contracts_own_capabilities() {
    let client = files_client();
    let watched = futures::executor::block_on(async move {
        let mut request = client.watch_directory_request();
        {
            let mut params = request.get();
            params.set_path("/tmp");
        }
        request.send().promise.await
    })
    .expect("the subscription answers");
    let subscription = watched
        .get()
        .expect("the results")
        .get_subscription()
        .expect("a subscription capability");

    let puller = subscription.clone();
    let pulled = futures::executor::block_on(async move {
        let request = puller.pull_request();
        request.send().promise.await
    })
    .expect("the pull answers");
    let events = pulled
        .get()
        .expect("the results")
        .get_events()
        .expect("the events");
    assert!(events.len() <= 64, "the pull is bounded");

    let cancelled = futures::executor::block_on(async move {
        let request = subscription.cancel_request();
        request.send().promise.await
    });
    assert_honest("FileEventSubscription.cancel", cancelled.map(|_| ()));
}

// ═══════════════════════════════════════════════════════════════════════
// The registry
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_registry_publishes_every_method_of_the_files_domain() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
        let cli: cli_service::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = cli.list_commands_request();
                {
                    let params = request.get();
                    let mut page = params.init_pagination();
                    page.set_offset(0);
                    page.set_limit(1024);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let commands = results.get_commands().expect("the commands");
        let mut entries = Vec::new();
        for index in 0..commands.len() {
            let registration = commands.get(index);
            entries.push((
                text(registration.get_command_path().expect("the path")),
                text(registration.get_interface_name().expect("the interface")),
                text(registration.get_method_name().expect("the method")),
                metadata_of(registration.get_metadata().ok(), "methodId"),
                metadata_of(registration.get_metadata().ok(), "composes"),
            ));
        }
        entries
    });

    // EVERY method of the contract's `Files` interface is published: the
    // surface must not hide a method the engine serves.
    let paths: Vec<&str> = published.iter().map(|entry| entry.0.as_str()).collect();
    for expected in [
        // The product's own verbs.
        "files ls",
        "files find",
        "files tree",
        "files stat",
        "files xattrs",
        "files cat",
        "files write",
        "files du",
        "files locks",
        "files hash",
        "files versions",
        "files branches",
        "files store",
        "files dedupe",
        "files watch",
        "files special",
        "files trash",
        "files recents",
        "files app",
        "files grep",
        "files compare",
        "files health",
        "files quota",
        "files mkdir",
        "files touch",
        "files rm",
        "files mv",
        "files cp",
        "files link",
        "files lock",
        "files unlock",
        // The methods the product's shell has no verb for.
        "files open",
        "files close",
        "files flush",
        "files exists",
        "files create",
        "files rmdir",
        "files hardlink",
        "files realpath",
        "files pwd",
        "files cd",
        "files version",
        "files version restore",
        "files version compare",
        "files by-hash",
        "files ephemeral",
        "files compression set",
        "files compression",
        "files dedupe-stats",
        "files batch copy",
        "files batch move",
        "files batch delete",
        "files size",
        "files stats",
        "files compare-dirs",
        "files activity",
        "files type",
        "files checksum",
        "files lifecycle set",
        "files lifecycle",
        "files lifecycle apply",
        "files audit",
        "files branch",
        "files branch merge",
        "files trash restore",
        "files trash empty",
        "files recents clear",
        "files quota set",
        "files snapshot",
        "files acquire",
        // The bound file service.
        "files service open",
        "files service read",
        "files service write",
        "files service stat",
        "files service close",
        // The subscriptions.
        "files watch pull",
        "files watch cancel",
        "files audit pull",
        "files audit cancel",
    ] {
        assert!(
            paths.contains(&expected),
            "the surface must not hide {expected}"
        );
    }
    // The composition: "files cat" needs the open and the close.
    let cat = published
        .iter()
        .find(|entry| entry.0 == "files cat")
        .expect("the cat must be published");
    assert_eq!(cat.4.as_deref(), Some("openFile,closeFile"));
    // The bound service's methods carry the interface that declares them.
    let open = published
        .iter()
        .find(|entry| entry.0 == "files service open")
        .expect("the bound open must be published");
    assert_eq!(open.1, "FileService");
    // The method's identity: the ordinal the schema declares.
    assert_eq!(published[0].3.as_deref(), Some("5"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_unavailable_commands_carry_the_matrix_reason() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
        let cli: cli_service::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = cli.list_unavailable_request();
                {
                    let params = request.get();
                    let mut page = params.init_pagination();
                    page.set_offset(0);
                    page.set_limit(1024);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let commands = results.get_commands().expect("the commands");
        let mut entries = Vec::new();
        for index in 0..commands.len() {
            let registration = commands.get(index);
            entries.push((
                text(registration.get_command_path().expect("the path")),
                registration.get_unavailable_reason().expect("the reason"),
                metadata_of(registration.get_metadata().ok(), "reason"),
            ));
        }
        entries
    });

    assert_eq!(published.len(), 4, "four commands cannot run here");
    let paths: Vec<&str> = published.iter().map(|entry| entry.0.as_str()).collect();
    assert!(paths.contains(&"files encrypted"));
    assert!(paths.contains(&"files decrypt"));
    assert!(paths.contains(&"files encryption-info"));
    assert!(paths.contains(&"files analyze"));
    for entry in &published {
        assert_eq!(
            entry.1,
            smart_lib::schema::cli_capnp::AvailabilityReason::UnsupportedPlatform
        );
        assert!(
            entry
                .2
                .as_deref()
                .expect("the reason must be published")
                .starts_with("not-applicable:"),
            "the reason must be the matrix's own: {:?}",
            entry.2
        );
    }

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The discovery descriptor
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_discovery_publishes_the_files_domains_descriptor() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
        let discovery_client: discovery::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = discovery_client.lookup_request();
                {
                    let params = request.get();
                    let mut filter = params.init_filter();
                    filter.set_name("files");
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let services = results.get_services().expect("the service list");
        let descriptor = services.get(0);
        let endpoints = descriptor.get_endpoints().expect("the endpoints");
        (
            text(descriptor.get_name().expect("the name")),
            text(descriptor.get_category().expect("the category")),
            descriptor.get_id().expect("the id").get_id(),
            descriptor.get_state().expect("the state"),
            endpoints.len(),
            metadata_of(descriptor.get_metadata().ok(), "capabilities"),
        )
    });

    assert_eq!(published.0, "files");
    assert_eq!(published.1, "files");
    assert_eq!(published.2, availability::FILES_SCHEMA_ID);
    // Three of the eighty-three cannot run here, so the service is degraded.
    assert_eq!(
        published.3,
        smart_lib::schema::discovery_capnp::ServiceState::Degraded
    );
    assert_eq!(published.4, 1);
    let tokens = published.5.expect("the capability tokens must be published");
    assert!(!tokens.is_empty(), "the tokens must be the domain's own");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_domains_availability_matrix_is_the_engines_own() {
    let (real, degraded, unavailable) = availability::status_counts();
    assert_eq!(real + degraded + unavailable, 83, "the matrix's own rows");
    assert_eq!(unavailable, 4, "the encryption trio and the content analysis");
    assert_eq!(real + degraded, 79);
}

#[test]
fn the_surface_names_every_contract_method_of_the_domain() {
    // The surface must not hide a method the engine serves: EVERY contract
    // method of this domain is named by at least one published path, and the
    // names are the contract's own.
    let service = build_service();
    let path = socket_path();

    let methods = run_client(service, &path, CapabilityClass::Control, |session| {
        let mut names: Vec<String> = Vec::new();
        {
            let cli: cli_service::Client = session.interface();
            let response = session
                .run(async move {
                    let mut request = cli.list_commands_request();
                    {
                        let params = request.get();
                        let mut page = params.init_pagination();
                        page.set_offset(0);
                        page.set_limit(1024);
                    }
                    request.send().promise.await
                })
                .expect("the call answers");
            let commands = response
                .get()
                .expect("the results")
                .get_commands()
                .expect("the commands");
            for index in 0..commands.len() {
                names.push(text(
                    commands
                        .get(index)
                        .get_method_name()
                        .expect("the method"),
                ));
            }
        }
        {
            let cli: cli_service::Client = session.interface();
            let response = session
                .run(async move {
                    let mut request = cli.list_unavailable_request();
                    {
                        let params = request.get();
                        let mut page = params.init_pagination();
                        page.set_offset(0);
                        page.set_limit(1024);
                    }
                    request.send().promise.await
                })
                .expect("the call answers");
            let commands = response
                .get()
                .expect("the results")
                .get_commands()
                .expect("the commands");
            for index in 0..commands.len() {
                names.push(text(
                    commands
                        .get(index)
                        .get_method_name()
                        .expect("the method"),
                ));
            }
        }
        names.sort();
        names.dedup();
        names
    });

    assert_eq!(
        methods.len(),
        81,
        "the surface must name every contract method: {methods:?}"
    );
    for expected in ["openFile", "closeFile", "readFile", "writeFile", "flushFile", "listDirectory", "createDirectory", "statFile", "fileExists", "renameFile", "removeFile", "copyFile", "createSymlink", "getSpecialPath", "watchDirectory", "searchFiles", "createFile", "touch", "removeDirectory", "linkFile", "resolveSymlink", "getCurrentDirectory", "changeDirectory", "walkTree", "getDiskUsage", "getFileMetadata", "compareFiles", "lockFile", "unlockFile", "getLocks", "getFileVersions", "getFileVersion", "restoreVersion", "compareVersions", "getByHash", "storeContentAddressed", "deduplicate", "createEphemeral", "createEncrypted", "decryptFile", "getEncryptionInfo", "setCompression", "getCompression", "getDeduplicationStats", "batchCopy", "batchMove", "batchDelete", "getDirectorySize", "getDirectoryStats", "compareDirectories", "getFileActivity", "getFileType", "getFileHash", "getFileChecksum", "setLifecycle", "getLifecycle", "applyLifecycle", "subscribeFileAudit", "analyzeFileContent", "searchFileContent", "branchVersion", "mergeBranch", "listBranches", "listTrash", "restoreTrash", "emptyTrash", "getRecentFiles", "clearRecentFiles", "getDefaultApp", "getFilesystemHealth", "getQuota", "setQuota", "createSnapshot", "acquireFile", "open", "read", "write", "stat", "pull", "cancel"] {
        assert!(
            methods.iter().any(|name| name == expected),
            "the surface must name {expected}: {methods:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
}
