//! The files domain's rendering, in the shell's aesthetic: box-drawing tables,
//! `◉/○/◌`, human sizes, honest reasons. Every renderer reads the ENGINE'S
//! OWNED types (`FileInfo`, `DirectoryEntry`, `DiskUsage`, `ExtendedMetadata`,
//! `FileEvent`, `FileVersion`, `FileLock`, …) — there is no capnp reader here,
//! so nothing can leak and nothing can disagree with what the engine read off
//! the host's own descriptors.
//!
//! Every width is a display width under the terminal-faithful measure, and
//! every cell is CONTENT plus a STYLE (`Cell`), never a pre-styled string: the
//! table measures the content and applies the colour after the column widths
//! are fixed.

use crate::color::ColorExt;
use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::widgets::{human_bytes, Cell, Sgr, Table};
use crate::OutputFormat;

use smart_lib::domains::files::types::{
    FileInfoResult, Page, Pagination, SortField, SortOrder, TextResult,
};
use smart_lib::domains::files::FilesBackend;

use super::{listing_filter, read_request};
use super::{FilesAction as A, FilesCmd};

/// A 64-bit value that must not be rounded: `T.Timestamp` is nanoseconds since
/// the epoch and a digest is 256 bits of identity, both beyond `f64`'s exact
/// integer range (2^53). They travel as their exact decimal text — an opaque
/// identity is not a quantity, so no arithmetic is lost.
fn exact(value: u64) -> Value {
    Value::String(value.to_string())
}

/// The most rows one human table draws. The host's own records can carry
/// hundreds of entries (`recently-used.xbel` on this host carries 300+), and a
/// table 300 rows tall is not the aesthetic — but hiding the rest would be a
/// lie about how much there is. So the shell draws this many and says, on the
/// line under the table, exactly how many more the host's own read found.
const MAX_ROWS: usize = 24;

/// The rows a human table draws, and how many the host's own read found beyond
/// them (0 when there was nothing to cut).
fn bounded(mut rows: Vec<Value>) -> (Vec<Value>, usize) {
    if rows.len() <= MAX_ROWS {
        return (rows, 0);
    }
    let hidden = rows.len() - MAX_ROWS;
    rows.truncate(MAX_ROWS);
    (rows, hidden)
}

/// The engine's own published reason for one contract method, so a degraded or
/// unavailable row is answered with the matrix's own measured words rather than
/// a second, hand-written excuse that could drift from it.
fn matrix_reason(method: &str) -> Option<String> {
    use smart_lib::domains::files::availability::{method_availability_for, AvailStatus};
    let row = method_availability_for(method)?;
    let word = match row.status {
        AvailStatus::Real => "REAL",
        AvailStatus::Degraded => "DEGRADED",
        AvailStatus::Unavailable => "UNAVAILABLE",
    };
    Some(if row.reason.is_empty() {
        word.to_string()
    } else {
        format!("{word} — {}", row.reason)
    })
}

// ═══ the data half ═══════════════════════════════════════════════════════

/// `files` / `files ls` — the listing: `getdents64` through the engine's own
/// filter, rendered as entry records.
pub fn ls_value(cmd: &FilesCmd, backend: &dyn FilesBackend) -> SmartResult<Value> {
    let (path, all, pattern, sort, order, recursive) = match &cmd.action {
        None | Some(A::Ls { path: None, .. }) => (
            backend.get_current_directory().map_err(SmartError::from)?,
            false,
            None,
            "name".to_string(),
            "asc".to_string(),
            false,
        ),
        Some(A::Ls {
            path,
            all,
            pattern,
            sort,
            order,
            recursive,
        }) => (
            path.clone().unwrap_or_else(|| {
                backend
                    .get_current_directory()
                    .unwrap_or_else(|_| ".".to_string())
            }),
            *all,
            pattern.clone(),
            sort.clone(),
            order.clone(),
            *recursive,
        ),
        _ => (
            backend
                .get_current_directory()
                .map_err(SmartError::from)?,
            false,
            None,
            "name".to_string(),
            "asc".to_string(),
            false,
        ),
    };
    let sort_by = super::sort_field(&sort)?;
    let sort_order = super::sort_order(&order)?;
    ls_path_value(
        backend,
        &path,
        all,
        pattern.as_deref(),
        sort_by,
        sort_order,
        recursive,
    )
}

/// The listing of one directory as records (the selector tree's `entries`).
pub fn ls_path_value(
    backend: &dyn FilesBackend,
    path: &str,
    all: bool,
    pattern: Option<&str>,
    sort_by: SortField,
    sort_order: SortOrder,
    recursive: bool,
) -> SmartResult<Value> {
    let filter = listing_filter(
        pattern.map(|p| vec![p.to_string()]).unwrap_or_default(),
        all,
        sort_by,
        sort_order,
        recursive,
    );
    match backend.list_directory(path, &filter) {
        Ok(page) => Ok(Value::List(page.items.iter().map(entry_value).collect())),
        Err(e) => Err(SmartError::from(e)),
    }
}

/// One directory entry as a record.
fn entry_value(entry: &smart_lib::domains::files::types::DirectoryEntry) -> Value {
    let created = entry
        .metadata
        .iter()
        .find(|(key, _)| key == "created")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let accessed = entry
        .metadata
        .iter()
        .find(|(key, _)| key == "accessed")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    Value::Object(vec![
        ("name".into(), Value::String(entry.name.clone())),
        ("path".into(), Value::String(entry.path.clone())),
        ("kind".into(), Value::String(entry.kind.as_token().to_string())),
        ("size".into(), Value::Number(entry.size as f64)),
        ("modified".into(), exact(entry.modified.as_nanos())),
        ("created".into(), exact(created.parse().unwrap_or(0))),
        ("accessed".into(), exact(accessed.parse().unwrap_or(0))),
        ("hidden".into(), Value::Bool(entry.name.starts_with('.'))),
    ])
}

/// `files find` — a name search over the backend's own glob, narrowed by kind.
pub fn find_value(
    backend: &dyn FilesBackend,
    path: &str,
    name: Option<&str>,
    dirs_only: bool,
    files_only: bool,
) -> SmartResult<Value> {
    use smart_lib::domains::files::types::FileSearchQuery;
    let query = FileSearchQuery {
        path: path.to_string(),
        pattern: name.unwrap_or("*").to_string(),
        recursive: true,
        page: Pagination {
            offset: 0,
            limit: u32::MAX,
        },
        annotations: Vec::new(),
    };
    match backend.search_files(&query) {
        Ok(page) => Ok(Value::List(
            page.items
                .iter()
                .filter(|hit| super::kind_matches(hit.kind, dirs_only, files_only))
                .map(search_value)
                .collect(),
        )),
        Err(e) => Err(SmartError::from(e)),
    }
}

/// One search hit as a record.
fn search_value(hit: &smart_lib::domains::files::types::FileSearchResult) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(hit.name.clone())),
        ("path".into(), Value::String(hit.path.clone())),
        ("kind".into(), Value::String(hit.kind.as_token().to_string())),
        ("size".into(), Value::Number(hit.size as f64)),
        ("modified".into(), exact(hit.modified.as_nanos())),
        ("score".into(), Value::Number(f64::from(hit.score))),
    ])
}

/// `files tree` — the engine's own bounded walk, each row carrying its depth.
pub fn tree_value(backend: &dyn FilesBackend, path: &str, depth: u32) -> SmartResult<Value> {
    let page = backend.walk_tree(
        path,
        depth,
        Pagination {
            offset: 0,
            limit: u32::MAX,
        },
    )?;
    Ok(Value::List(
        page.items
            .iter()
            .map(|entry| {
                let relative = entry
                    .path
                    .strip_prefix(path)
                    .unwrap_or(&entry.path)
                    .trim_start_matches('/');
                let depth = relative.matches('/').count() as u32 + if relative.is_empty() { 0 } else { 1 };
                Value::Object(vec![
                    ("name".into(), Value::String(entry.name.clone())),
                    ("path".into(), Value::String(entry.path.clone())),
                    ("kind".into(), Value::String(entry.kind.as_token().to_string())),
                    ("size".into(), Value::Number(entry.size as f64)),
                    ("depth".into(), Value::Number(f64::from(depth))),
                ])
            })
            .collect(),
    ))
}

/// `files stat` — the kernel's own `statx` answer, read out of the contract's
/// total union (the error arm is the honest answer, never a zeroed struct).
pub fn stat_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    match backend.stat_file(path).map_err(SmartError::from)? {
        FileInfoResult::Info(info) => Ok(info_value(&info)),
        FileInfoResult::Error(e) => Err(SmartError::from(e)),
        FileInfoResult::Unknown => Err(SmartError::failed(format!(
            "statFile answered the contract's unknown union arm for {path}"
        ))),
    }
}

/// One `FileInfo` as a record.
fn info_value(info: &smart_lib::domains::files::types::FileInfo) -> Value {
    let mut fields = vec![
        ("path".into(), Value::String(info.path.clone())),
        ("kind".into(), Value::String(info.kind.as_token().to_string())),
        ("size".into(), Value::Number(info.size as f64)),
        ("created".into(), exact(info.created.as_nanos())),
        ("modified".into(), exact(info.modified.as_nanos())),
        ("accessed".into(), exact(info.accessed.as_nanos())),
        ("mime_type".into(), Value::String(info.mime_type.clone())),
    ];
    if !info.content_hash.is_empty() {
        fields.push(("content_hash".into(), Value::String(hex(&info.content_hash))));
    }
    for (key, value) in &info.metadata {
        fields.push((key.clone(), Value::String(value.clone())));
    }
    Value::Object(fields)
}

/// `files xattrs` — `statx` plus the file's own user xattrs, inode, link
/// count, owner label and mime type.
pub fn xattrs_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    let meta = backend.get_file_metadata(path).map_err(SmartError::from)?;
    // The engine's own metadata pairs travel to the top level, so a selector
    // and the renderer read the same fields (inode, linkCount, ownerUid, …).
    let mut fields = vec![
        ("path".into(), Value::String(meta.path.clone())),
        ("name".into(), Value::String(meta.name.clone())),
        ("kind".into(), Value::String(meta.kind.as_token().to_string())),
        ("size".into(), Value::Number(meta.size as f64)),
        ("owner".into(), Value::String(meta.owner.clone())),
        ("mime_type".into(), Value::String(meta.mime_type.clone())),
        ("target".into(), Value::String(meta.target.clone())),
        ("hidden".into(), Value::Bool(meta.is_hidden)),
        ("read_only".into(), Value::Bool(meta.is_read_only)),
        ("executable".into(), Value::Bool(meta.is_executable)),
        ("created".into(), exact(meta.created.as_nanos())),
        ("modified".into(), exact(meta.modified.as_nanos())),
        ("accessed".into(), exact(meta.accessed.as_nanos())),
    ];
    for (key, value) in &meta.metadata {
        fields.push((key.clone(), Value::String(value.clone())));
    }
    fields.push(("attributes".into(), pairs_value(&meta.metadata)));
    Ok(Value::Object(fields))
}

/// `files cat` — the handle surface's own read path: `openat`, the kernel's
/// `mmap` window served with no intermediate copy, `close`. Rendered HONESTLY:
/// text when the bytes are text, and a named non-text answer (with a hex
/// preview) when they are not — never a lossy decode presented as content.
pub fn cat_value(
    backend: &dyn FilesBackend,
    path: &str,
    offset: u64,
    bytes: Option<u32>,
) -> SmartResult<Value> {
    // The handle surface the engine's own `Files` API names: `openat`, the
    // kernel's `mmap` window served with no intermediate copy, then `close`.
    let handle = backend
        .open_file(path, &read_request())
        .map_err(SmartError::from)?;
    let window = bytes.unwrap_or(u32::MAX);
    let read = backend.read_file(&handle, offset, window);
    let closed = backend.close_file(&handle);
    let data = read.map_err(SmartError::from)?;
    closed.map_err(SmartError::from)?;
    let text = String::from_utf8(data.clone());
    let mut fields = vec![
        ("path".into(), Value::String(path.to_string())),
        ("offset".into(), Value::Number(offset as f64)),
        ("bytes".into(), Value::Number(data.len() as f64)),
    ];
    match text {
        Ok(text) => {
            fields.push(("text".into(), Value::String(text)));
            fields.push(("binary".into(), Value::Bool(false)));
        }
        Err(_) => {
            let preview_len = data.len().min(64);
            fields.push(("binary".into(), Value::Bool(true)));
            fields.push((
                "preview".into(),
                Value::String(hex(&data[..preview_len])),
            ));
            fields.push((
                "reason".into(),
                Value::String(format!(
                    "the window's {} byte(s) are not valid UTF-8 text — the bytes are hashed and \
                     previewed above rather than decoded lossily and passed off as content",
                    data.len()
                )),
            ));
        }
    }
    Ok(Value::Object(fields))
}

/// `files du` — `statvfs` plus the host's own mount row, in human sizes.
pub fn du_value(backend: &dyn FilesBackend, path: String) -> SmartResult<Value> {
    let usage = backend.get_disk_usage(&path).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("path".into(), Value::String(path)),
        ("total".into(), Value::Number(usage.total as f64)),
        ("used".into(), Value::Number(usage.used as f64)),
        ("free".into(), Value::Number(usage.free as f64)),
        ("percent_used".into(), Value::Number(f64::from(usage.percent_used))),
        ("total_human".into(), Value::String(human_bytes(usage.total))),
        ("used_human".into(), Value::String(human_bytes(usage.used))),
        ("free_human".into(), Value::String(human_bytes(usage.free))),
        ("mount_point".into(), Value::String(usage.mount_point.clone())),
        ("mount_id".into(), exact(usage.mount_id.get())),
        ("filesystem_id".into(), exact(usage.filesystem_id.get())),
        ("metadata".into(), pairs_value(&usage.metadata)),
    ]))
}

/// `files mounts` — the mount this path lives on, in the host's own words.
pub fn mounts_value(backend: &dyn FilesBackend, path: String) -> SmartResult<Value> {
    let usage = backend.get_disk_usage(&path).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("path".into(), Value::String(path)),
        ("mount_point".into(), Value::String(usage.mount_point.clone())),
        ("mount_id".into(), exact(usage.mount_id.get())),
        ("filesystem_id".into(), exact(usage.filesystem_id.get())),
        ("metadata".into(), pairs_value(&usage.metadata)),
    ]))
}

/// `files locks` — exactly what THIS process holds: the kernel publishes no
/// portable lock table readable by this uid, so the backend's own table of
/// `fcntl(F_SETLK)` locks it took is the answer, and the reason says so.
pub fn locks_value(backend: &dyn FilesBackend, path: Option<&str>) -> SmartResult<Value> {
    let page = backend
        .get_locks(path.unwrap_or(""), Pagination { offset: 0, limit: u32::MAX })
        .map_err(SmartError::from)?;
    if page.items.is_empty() {
        return Ok(Value::Object(vec![
            ("locks".into(), Value::List(Vec::new())),
            ("total".into(), Value::Number(0.0)),
            (
                "reason".into(),
                Value::String(
                    matrix_reason("getLocks").unwrap_or_else(|| {
                        "this process holds no lock on this path".to_string()
                    }),
                ),
            ),
        ]));
    }
    Ok(Value::Object(vec![
        (
            "locks".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|lock| {
                        Value::Object(vec![
                            ("lock_id".into(), Value::Number(lock.lock_id as f64)),
                            ("path".into(), Value::String(lock.lock_path())),
                            ("owner".into(), Value::String(lock.owner.clone())),
                            ("acquired_at".into(), exact(lock.acquired_at.as_nanos())),
                            ("expires_at".into(), exact(lock.expires_at.as_nanos())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
    ]))
}

/// `files hash` — SHA-256 implemented in the engine, over the file's own bytes.
pub fn hash_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    let result = backend
        .get_file_hash(path, "sha256")
        .map_err(SmartError::from)?;
    match result {
        smart_lib::domains::files::types::FileHashResult::Hash(digest) => {
            Ok(Value::Object(vec![
                ("path".into(), Value::String(path.to_string())),
                ("algorithm".into(), Value::String("sha256".into())),
                ("hash".into(), Value::String(hex(&digest))),
            ]))
        }
        smart_lib::domains::files::types::FileHashResult::Error(e) => Err(SmartError::from(e)),
        smart_lib::domains::files::types::FileHashResult::Unknown => Err(SmartError::failed(
            "getFileHash answered the contract's unknown union arm",
        )),
    }
}

/// `files versions` — the backend's own content-addressed version store: every
/// version a real object, and the store's scope named when it holds none.
pub fn versions_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    let page: Page<smart_lib::domains::files::types::FileVersion> = backend
        .get_file_versions(path, Pagination { offset: 0, limit: u32::MAX })
        .map_err(SmartError::from)?;
    if page.items.is_empty() {
        return Ok(Value::Object(vec![
            ("versions".into(), Value::List(Vec::new())),
            ("total".into(), Value::Number(0.0)),
            (
                "reason".into(),
                Value::String(matrix_reason("getFileVersions").unwrap_or_else(|| {
                    "the store holds no version of this path".to_string()
                })),
            ),
        ]));
    }
    Ok(Value::Object(vec![
        (
            "versions".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|version| {
                        Value::Object(vec![
                            ("version_id".into(), Value::Number(version.version_id as f64)),
                            ("size".into(), Value::Number(version.size as f64)),
                            ("created_at".into(), exact(version.created_at.as_nanos())),
                            ("message".into(), Value::String(version.message.clone())),
                            ("hash".into(), Value::String(hex(&version.content_hash))),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
    ]))
}

/// `files branches` — the branch names the store actually holds.
pub fn branches_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    let page = backend
        .list_branches(path, Pagination { offset: 0, limit: u32::MAX })
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "branches".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|branch| {
                        Value::Object(vec![
                            ("branch_id".into(), Value::String(branch.branch_id.clone())),
                            ("name".into(), Value::String(branch.branch_name.clone())),
                            ("base_version".into(), Value::Number(branch.base_version as f64)),
                            ("created_at".into(), exact(branch.created_at.as_nanos())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("listBranches")
                    .unwrap_or_else(|| "the store's own branch records for this path".to_string()),
            ),
        ),
    ]))
}

/// `files store --content` — the engine's own SHA-256 digest over the path's
/// bytes, and the real object the store writes under that digest.
pub fn store_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    let result = backend
        .store_content_addressed(path)
        .map_err(SmartError::from)?;
    match result {
        smart_lib::domains::files::types::ContentRefResult::Ref(reference) => {
            Ok(Value::Object(vec![
                ("path".into(), Value::String(path.to_string())),
                ("algorithm".into(), Value::String(reference.algorithm.clone())),
                ("hash".into(), Value::String(hex(&reference.hash))),
                ("size_bytes".into(), Value::Number(reference.size_bytes as f64)),
                ("stored".into(), Value::Bool(true)),
                (
                    "reason".into(),
                    Value::String(matrix_reason("storeContentAddressed").unwrap_or_default()),
                ),
            ]))
        }
        smart_lib::domains::files::types::ContentRefResult::Error(e) => Err(SmartError::from(e)),
        other => Err(SmartError::failed(format!(
            "storeContentAddressed answered an unexpected union arm: {other:?}"
        ))),
    }
}

/// `files dedupe` — the host's own inode sharing, measured: this host's
/// filesystem has no reflink (read from `/proc/mounts`), so the answer names
/// what the kernel actually shares rather than claiming a saving.
pub fn dedupe_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    // The engine's own answer: (saved bytes, reference count) as the host
    // measures them — the inode sharing that has actually happened.
    let (saved_bytes, references) = backend.deduplicate(path).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("path".into(), Value::String(path.to_string())),
        ("saved_bytes".into(), Value::Number(saved_bytes as f64)),
        ("saved_human".into(), Value::String(human_bytes(saved_bytes))),
        ("references".into(), Value::Number(f64::from(references))),
        (
            "reason".into(),
            Value::String(
                matrix_reason("deduplicate")
                    .unwrap_or_else(|| "the host's own measured inode sharing".to_string()),
            ),
        ),
    ]))
}

/// `files watch` — a real `inotify` subscription: one watch, bounded pull
/// rounds, the kernel's own events.
pub fn watch_value(
    backend: &dyn FilesBackend,
    path: &str,
    count: u32,
    interval_ms: u64,
) -> SmartResult<Value> {
    let subscription = backend.watch_directory(path).map_err(SmartError::from)?;
    let mut events = Vec::new();
    for _ in 0..count.max(1) {
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        let batch = backend
            .pull_watch(subscription, 32)
            .map_err(SmartError::from)?;
        for event in batch {
            events.push(event_value(&event));
        }
    }
    let _ = backend.cancel_watch(subscription);
    Ok(Value::List(events))
}

/// One `FileEvent` as a record.
fn event_value(event: &smart_lib::domains::files::types::FileEvent) -> Value {
    Value::Object(vec![
        ("kind".into(), Value::String(event.kind.as_token().to_string())),
        ("path".into(), Value::String(event.path.clone())),
        ("old_path".into(), Value::String(event.old_path.clone())),
        ("timestamp".into(), exact(event.timestamp.as_nanos())),
    ])
}

/// `files special` — the host's own XDG directories, through the contract's
/// total `TextResult` union.
pub fn special_value(backend: &dyn FilesBackend, token: &str) -> SmartResult<Value> {
    let kind = super::special_path_token(token);
    let result = backend
        .get_special_path(kind)
        .map_err(SmartError::from)?;
    match result {
        TextResult::Value(path) => Ok(Value::Object(vec![
            ("token".into(), Value::String(token.to_string())),
            ("path".into(), Value::String(path)),
        ])),
        TextResult::Error(e) => Err(SmartError::from(e)),
        TextResult::Unknown => Err(SmartError::failed(
            "getSpecialPath answered the contract's unknown union arm",
        )),
    }
}

/// `files trash` — the host's own freedesktop trash directory.
pub fn trash_value(backend: &dyn FilesBackend) -> SmartResult<Value> {
    let page = backend
        .list_trash(Pagination { offset: 0, limit: u32::MAX })
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "entries".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|entry| {
                        Value::Object(vec![
                            ("entry_id".into(), Value::String(entry.entry_id.clone())),
                            ("original_path".into(), Value::String(entry.original_path.clone())),
                            ("deleted_at".into(), exact(entry.deleted_at.as_nanos())),
                            ("size".into(), Value::Number(entry.size as f64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("listTrash")
                    .unwrap_or_else(|| "the host's own trash directory".to_string()),
            ),
        ),
    ]))
}

/// `files recents` — the host's own recently-used record, where the host has
/// one; an honest empty page with a stable zero total where it does not.
pub fn recents_value(backend: &dyn FilesBackend) -> SmartResult<Value> {
    let page = backend
        .get_recent_files(Pagination { offset: 0, limit: u32::MAX })
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "files".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|recent| {
                        Value::Object(vec![
                            ("path".into(), Value::String(recent.path.clone())),
                            ("mime_type".into(), Value::String(recent.mime_type.clone())),
                            ("timestamp".into(), exact(recent.timestamp.as_nanos())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("getRecentFiles")
                    .unwrap_or_else(|| "the host's own recently-used record".to_string()),
            ),
        ),
    ]))
}

/// `files app` — the host's own default application for a mime type, read out
/// of the contract's total `DefaultAppResult` union.
pub fn app_value(backend: &dyn FilesBackend, mime: &str) -> SmartResult<Value> {
    let result = backend.get_default_app(mime).map_err(SmartError::from)?;
    match result {
        smart_lib::domains::files::types::DefaultAppResult::App(app) => Ok(Value::Object(vec![
            ("mime_type".into(), Value::String(mime.to_string())),
            ("name".into(), Value::String(app.name.clone())),
            ("app_id".into(), exact(app.app_id.get())),
            ("metadata".into(), pairs_value(&app.metadata)),
        ])),
        smart_lib::domains::files::types::DefaultAppResult::Error(e) => Err(SmartError::from(e)),
        smart_lib::domains::files::types::DefaultAppResult::Unknown => Err(SmartError::failed(
            "getDefaultApp answered the contract's unknown union arm",
        )),
    }
}

/// `files grep` — a content search over the files' real lines.
pub fn grep_value(backend: &dyn FilesBackend, path: &str, query: &str) -> SmartResult<Value> {
    use smart_lib::domains::files::types::ContentSearchQuery;
    let search = ContentSearchQuery {
        path: path.to_string(),
        query: query.to_string(),
        recursive: true,
        mime_types: Vec::new(),
        page: Pagination {
            offset: 0,
            limit: u32::MAX,
        },
        annotations: Vec::new(),
    };
    let page = backend
        .search_file_content(&search)
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "hits".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|hit| {
                        Value::Object(vec![
                            ("path".into(), Value::String(hit.path.clone())),
                            ("line".into(), Value::Number(f64::from(hit.line_number))),
                            ("snippet".into(), Value::String(hit.snippet.clone())),
                            ("score".into(), Value::Number(f64::from(hit.score))),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
    ]))
}

/// `files compare` — the engine's own byte comparison, with the structured
/// diff the host's own words carry.
pub fn compare_value(backend: &dyn FilesBackend, left: &str, right: &str) -> SmartResult<Value> {
    let result = backend
        .compare_files(left, right)
        .map_err(SmartError::from)?;
    let diff = String::from_utf8_lossy(&result.diff).to_string();
    Ok(Value::Object(vec![
        ("left".into(), Value::String(left.to_string())),
        ("right".into(), Value::String(right.to_string())),
        ("identical".into(), Value::Bool(result.identical)),
        ("diff".into(), Value::String(diff)),
        ("metadata".into(), pairs_value(&result.metadata)),
    ]))
}

/// `files health` — the measured half only: read-only state, space and the
/// mount's type. A full `fsck` is a host-mutating act law #8 keeps the
/// founder's, and the answer says which half is missing.
pub fn health_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    let result = backend
        .get_filesystem_health(path)
        .map_err(SmartError::from)?;
    match result {
        smart_lib::domains::files::types::FilesystemHealthResult::Health(health) => {
            Ok(Value::Object(vec![
                ("path".into(), Value::String(path.to_string())),
                ("status".into(), Value::String(health.status.clone())),
                ("errors".into(), Value::Number(health.errors as f64)),
                ("warnings".into(), Value::Number(health.warnings as f64)),
                ("details".into(), Value::String(health.details.clone())),
                (
                    "reason".into(),
                    Value::String(
                        matrix_reason("getFilesystemHealth").unwrap_or_default(),
                    ),
                ),
            ]))
        }
        smart_lib::domains::files::types::FilesystemHealthResult::Error(e) => {
            Err(SmartError::from(e))
        }
        smart_lib::domains::files::types::FilesystemHealthResult::Unknown => Err(
            SmartError::failed("getFilesystemHealth answered the contract's unknown union arm"),
        ),
    }
}

/// `files quota` — the filesystem's own totals from `statvfs`, and the
/// per-user half's measured absence named.
pub fn quota_value(backend: &dyn FilesBackend, path: &str) -> SmartResult<Value> {
    let quota = backend.get_quota(path).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("path".into(), Value::String(path.to_string())),
        ("max_size_bytes".into(), Value::Number(quota.max_size_bytes as f64)),
        ("max_files".into(), Value::Number(quota.max_files as f64)),
        (
            "current_size_bytes".into(),
            Value::Number(quota.current_size_bytes as f64),
        ),
        (
            "current_files".into(),
            Value::Number(quota.current_files as f64),
        ),
        (
            "soft_limit_percent".into(),
            Value::Number(f64::from(quota.soft_limit_percent)),
        ),
        (
            "reason".into(),
            Value::String(
                matrix_reason("getQuota").unwrap_or_else(|| "the mount's own statvfs totals".to_string()),
            ),
        ),
    ]))
}

/// `files caps` — the backend's own published capability tokens (law #6).
pub fn caps_value(backend: &dyn FilesBackend) -> Value {
    Value::List(
        backend
            .files_caps()
            .into_iter()
            .map(|token| Value::String(token.to_string()))
            .collect(),
    )
}

/// The value a gated write answers with once it runs (the refusal path is the
/// error the gate returns).
pub fn verb_value(verb: &str, target: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("target".into(), Value::String(target.to_string())),
        ("changed".into(), Value::Bool(true)),
    ])
}

/// The contract's `StringMap` as records, so a pipe can address a pair.
fn pairs_value(pairs: &[(String, String)]) -> Value {
    Value::List(
        pairs
            .iter()
            .map(|(key, value)| {
                Value::Object(vec![
                    ("key".into(), Value::String(key.clone())),
                    ("value".into(), Value::String(value.clone())),
                ])
            })
            .collect(),
    )
}

/// The digest's own hex spelling (SHA-256 over the file's own bytes is the
/// engine's answer, and a digest is an identity, not a quantity).
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ═══ the aesthetic ════════════════════════════════════════════════════════

/// Render one command's value in the requested format.
pub fn render_action(cmd: &FilesCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &FilesCmd, value: &Value) -> String {
    match &cmd.action {
        None | Some(A::Ls { .. }) => render_ls(value),
        Some(A::Find { path, .. }) => render_find(value, path),
        Some(A::Tree { path, depth }) => render_tree(value, path, *depth),
        Some(A::Stat { path }) => render_stat(value, path),
        Some(A::Xattrs { path }) => render_xattrs(value, path),
        Some(A::Cat { path, .. }) => render_cat(value, path),
        Some(A::Du { path }) => render_du(value, path.as_deref()),
        Some(A::Mounts { path }) => render_mounts(value, path.as_deref()),
        Some(A::Locks { path }) => render_locks(value, path.as_deref()),
        Some(A::Hash { path }) => render_hash(value, path),
        Some(A::Versions { path }) => render_versions(value, path),
        Some(A::Branches { path }) => render_branches(value, path),
        Some(A::Store { .. }) => render_store(value),
        Some(A::Dedupe { path }) => render_dedupe(value, path),
        Some(A::Watch { path, .. }) => render_watch(value, path),
        Some(A::Special { token }) => render_special(value, token),
        Some(A::Trash) => render_trash(value),
        Some(A::Recents) => render_recents(value),
        Some(A::App { mime }) => render_app(value, mime),
        Some(A::Grep { path, query }) => render_grep(value, path, query),
        Some(A::Compare { left, right }) => render_compare(value, left, right),
        Some(A::Health { path }) => render_health(value, path),
        Some(A::Quota { path }) => render_quota(value, path),
        Some(A::Caps) => render_caps(value),
        Some(A::Mkdir { .. })
        | Some(A::Touch { .. })
        | Some(A::Rm { .. })
        | Some(A::Mv { .. })
        | Some(A::Cp { .. })
        | Some(A::Write { .. })
        | Some(A::Link { .. })
        | Some(A::Lock { .. })
        | Some(A::Unlock { .. }) => render_verb(value),
    }
}

/// The kind glyph, in the engine's own `FileKind` vocabulary.
fn kind_glyph(kind: &str) -> Cell {
    match kind {
        "directory" => Cell::styled("▣", Sgr::Blue),
        "regular" => Cell::plain("·"),
        "symlink" => Cell::styled("↔", Sgr::Cyan),
        _ => Cell::plain("▢"),
    }
}

fn render_ls(value: &Value) -> String {
    let (entries, hidden) = bounded(match value {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    let mut out = String::new();
    if entries.is_empty() {
        out.push_str(&format!(
            "  {} {} {} — the directory is empty (the host was read and answered with no entries)\n",
            "📁".blue().bold(),
            "Files".blue().bold(),
            "(0)".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let total: u64 = entries
        .iter()
        .map(|e| e.field("size").as_f64().unwrap_or(0.0) as u64)
        .sum();
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "📁".blue().bold(),
        "Files".blue().bold(),
        format!("({})", entries.len()).dimmed(),
        format!("— getdents64, {} total", human_bytes(total)).dimmed()
    ));
    let mut table = Table::new(vec![
        "".into(),
        "NAME".into(),
        "KIND".into(),
        "SIZE".into(),
        "MODIFIED".into(),
    ]);
    for entry in &entries {
        table = table.row(vec![
            kind_glyph(&entry.field_str("kind")),
            entry.field_str("name").into(),
            entry.field_str("kind").into(),
            human_bytes(entry.field("size").as_f64().unwrap_or(0.0) as u64).into(),
            stamp_label(entry.field_str("modified")).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more entr(y|ies) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

fn render_find(value: &Value, path: &str) -> String {
    let (hits, hidden) = bounded(match value {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "🔍".blue().bold(),
        "Find".blue().bold(),
        format!("({})", hits.len()).dimmed(),
        format!("under {} — the backend's own glob over getdents64", path).dimmed()
        ));
    if hits.is_empty() {
        out.push_str(&format!(
            "  └─ {} no entry under {path} matches (an honest empty — the walk ran)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["".into(), "PATH".into(), "KIND".into(), "SIZE".into()]);
    for hit in &hits {
        table = table.row(vec![
            kind_glyph(&hit.field_str("kind")),
            hit.field_str("path").into(),
            hit.field_str("kind").into(),
            human_bytes(hit.field("size").as_f64().unwrap_or(0.0) as u64).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more hit(s) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

fn render_tree(value: &Value, path: &str, depth: u32) -> String {
    let (rows, hidden) = bounded(match value {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "🌳".blue().bold(),
        "Tree".blue().bold(),
        format!("({})", rows.len()).dimmed(),
        format!("— {} walked to depth {}", path, depth).dimmed()
        ));
    if rows.is_empty() {
        out.push_str(&format!(
            "  └─ {} the walk found nothing under {path} (an honest empty)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["".into(), "NAME".into(), "KIND".into(), "SIZE".into()]);
    for row in &rows {
        let depth = row.field("depth").as_f64().unwrap_or(1.0) as usize;
        let indent = "  ".repeat(depth.saturating_sub(1));
        table = table.row(vec![
            kind_glyph(&row.field_str("kind")),
            format!("{indent}{}", row.field_str("name")).into(),
            row.field_str("kind").into(),
            human_bytes(row.field("size").as_f64().unwrap_or(0.0) as u64).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more row(s) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

/// The statx record, one row per measured field.
fn render_stat(value: &Value, path: &str) -> String {
    if value == &Value::Null {
        return format!("  {} stat — no answer for {path}", "✗".red());
    }
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🔎".blue().bold(),
        "Stat".blue().bold(),
        path.bold()
    ));
    let kind = value.field_str("kind");
    out.push_str(&format!(
        "  ├─ Type       {} {}\n",
        kind_glyph(&kind),
        if kind.is_empty() { "— the host did not answer".to_string() } else { kind }
    ));
    out.push_str(&format!(
        "  ├─ Size       {} ({})\n",
        value.field_str("size"),
        human_bytes(value.field("size").as_f64().unwrap_or(0.0) as u64)
    ));
    out.push_str(&format!(
        "  ├─ Created    {}\n",
        stamp_label(value.field_str("created"))
    ));
    out.push_str(&format!(
        "  ├─ Modified   {}\n",
        stamp_label(value.field_str("modified"))
    ));
    out.push_str(&format!(
        "  ├─ Accessed   {}\n",
        stamp_label(value.field_str("accessed"))
    ));
    let mime = value.field_str("mime_type");
    out.push_str(&format!(
        "  ├─ Mime       {}\n",
        if mime.is_empty() { "— the host's mime.types table does not name it".to_string() } else { mime }
    ));
    for (key, _) in [
        ("inode", "Inode"),
        ("linkCount", "Links"),
        ("uid", "Uid"),
        ("gid", "Gid"),
        ("blocks", "Blocks"),
        ("permissions", "Mode"),
        ("changed", "Changed"),
    ] {
        let field = value.field_str(key);
        if field.is_empty() {
            continue;
        }
        // `changed` is the kernel's own ctime in nanoseconds, so it is printed
        // as a wall clock like the other three times rather than as a number.
        let shown = if key == "changed" {
            stamp_label(field)
        } else {
            field
        };
        out.push_str(&format!("  ├─ {:<10} {}\n", key_label(key), shown));
    }
    let hash = value.field_str("content_hash");
    if !hash.is_empty() {
        out.push_str(&format!("  └─ Hash       {}", hash.dimmed()));
    } else {
        out.push_str("  └─ Source     statx(2) — the kernel's own answer");
    }
    out
}

fn key_label(key: &str) -> &'static str {
    match key {
        "inode" => "Inode",
        "linkCount" => "Links",
        "uid" => "Uid",
        "gid" => "Gid",
        "blocks" => "Blocks",
        "permissions" => "Mode",
        "changed" => "Changed",
        _ => "",
    }
}

/// The extended metadata: statx plus the file's own user xattrs.
fn render_xattrs(value: &Value, path: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🏷".blue().bold(),
        "Extended metadata".blue().bold(),
        path.bold()
    ));
    out.push_str(&format!(
        "  ├─ Owner      {} (uid {})\n",
        value.field_str("owner"),
        value.field_str("ownerUid")
    ));
    let permissions = value.field_str("permissions");
    out.push_str(&format!(
        "  ├─ Mode       {} — read-only {} executable {}\n",
        if permissions.is_empty() { "—".to_string() } else { permissions },
        yes_no(value.field_bool("read_only")),
        yes_no(value.field_bool("executable"))
    ));
    let target = value.field_str("target");
    if !target.is_empty() {
        out.push_str(&format!("  ├─ Target     {}\n", target));
    }
    let created = value.field_str("created");
    let modified = value.field_str("modified");
    out.push_str(&format!(
        "  ├─ Created    {} · modified {}\n",
        if nanos_of(&created) == 0 { "—".to_string() } else { stamp_label(created) },
        stamp_label(modified)
    ));
    let attributes = match value.field("attributes") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    if attributes.is_empty() {
        out.push_str(&format!(
            "  └─ Attributes {} the file carries no user.* xattrs on its own namespace\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    out.push_str(&format!("  ├─ Attributes {} ({}):\n", "◉".green(), attributes.len()));
    let last = attributes.len().saturating_sub(1);
    for (index, attribute) in attributes.iter().enumerate() {
        let branch = if index == last { "└─" } else { "├─" };
        let key = attribute.field_str("key");
        // The kernel's own btime travels as nanoseconds, so the shell prints
        // it as a wall clock like every other time (a raw nanos count is the
        // engine's spillover, not a value a human reads).
        let value = if key == "created" {
            stamp_label(attribute.field_str("value"))
        } else {
            attribute.field_str("value")
        };
        out.push_str(&format!("  {}   {} {}\n", branch, key.bold(), value));
    }
    out.push_str("     read natively from the file's own user xattr namespace");
    out.trim_end().to_string()
}

/// The handle-surface read, rendered honestly for text.
fn render_cat(value: &Value, path: &str) -> String {
    let mut out = String::new();
    let bytes = value.field("bytes").as_f64().unwrap_or(0.0) as u64;
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "📄".blue().bold(),
        "Cat".blue().bold(),
        path.bold(),
        format!("— {} byte(s) through the engine's mmap window", bytes).dimmed()
    ));
    if value.field_bool("binary") {
        out.push_str(&format!(
            "  {} {}\n",
            "✗".red(),
            value.field_str("reason")
        ));
        out.push_str(&format!("  └─ hex  {}", value.field_str("preview")));
        return out.trim_end().to_string();
    }
    out.push_str(&value.field_str("text"));
    out.trim_end().to_string()
}

/// `du`: the host's own statvfs answer with human sizes and the mount row.
fn render_du(value: &Value, path: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "💽".blue().bold(),
        "Disk usage".blue().bold(),
        value.field_str("path").bold(),
        "— statvfs plus the host's own /proc/mounts row".dimmed()
    ));
    out.push_str(&format!(
        "  ├─ Total      {:<12} {}\n",
        value.field_str("total_human"),
        format!("({} bytes)", value.field_str("total")).dimmed()
    ));
    out.push_str(&format!(
        "  ├─ Used       {:<12} {}%\n",
        value.field_str("used_human"),
        format!("{:.1}", value.field("percent_used").as_f64().unwrap_or(0.0))
    ));
    out.push_str(&format!("  ├─ Free       {:<12}", value.field_str("free_human")));
    let mount = value.field_str("mount_point");
    if !mount.is_empty() {
        out.push_str(&format!(
            "\n  ├─ Mount      {} (mount id {})\n",
            mount,
            value.field_str("mount_id")
        ));
    }
    let metadata = match value.field("metadata") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    for pair in metadata.iter().filter(|p| !p.field_str("value").is_empty()) {
        out.push_str(&format!(
            "  ├─ {:<10} {}\n",
            pair.field_str("key"),
            pair.field_str("value")
        ));
    }
    let _ = path;
    out.push_str("  └─ Source     the kernel's own statvfs(2) for this path's filesystem");
    out.trim_end().to_string()
}

fn render_mounts(value: &Value, path: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🗄".blue().bold(),
        "Mount".blue().bold(),
        value.field_str("path").bold()
    ));
    let mount = value.field_str("mount_point");
    out.push_str(&format!(
        "  ├─ Mount point {}\n",
        if mount.is_empty() {
            "— /proc/mounts carries no row for this path".to_string()
        } else {
            mount.clone()
        }
    ));
    out.push_str(&format!(
        "  ├─ Mount id    {} · filesystem id {}\n",
        value.field_str("mount_id"),
        value.field_str("filesystem_id")
    ));
    let metadata = match value.field("metadata") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    for pair in &metadata {
        out.push_str(&format!(
            "  ├─ {:<11} {}\n",
            pair.field_str("key"),
            pair.field_str("value")
        ));
    }
    let _ = path;
    out.push_str("  └─ Source     /proc/mounts read as the host's own words (root mount included)");
    out.trim_end().to_string()
}

/// `locks`: this process's own `fcntl` table, with the measured scope named.
fn render_locks(value: &Value, path: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🔒".blue().bold(),
        "Locks".blue().bold(),
        path.map(|p| p.to_string()).unwrap_or_else(|| "every path".to_string()).bold()
    ));
    let (locks, hidden) = bounded(match value.field("locks") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    if locks.is_empty() {
        let reason = value.field_str("reason");
        out.push_str(&format!(
            "  └─ {} this process holds no lock{}\n",
            "○".dimmed(),
            if reason.is_empty() { String::new() } else { format!(" — {reason}") }
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["ID".into(), "PATH".into(), "OWNER".into(), "ACQUIRED".into()]);
    for lock in &locks {
        table = table.row(vec![
            Cell::plain(lock.field_str("lock_id")),
            lock.field_str("path").into(),
            lock.field_str("owner").into(),
            stamp_label(lock.field_str("acquired_at")).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more lock(s) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.push_str(&format!(
        "\n  {} the kernel's own fcntl(F_SETLK) advisory locks — this process's table\n",
        "●".blue().bold()
    ));
    let reason = value.field_str("reason");
    if !reason.is_empty() {
        out.push_str(&format!("     {reason}"));
    }
    out.trim_end().to_string()
}

fn render_hash(value: &Value, path: &str) -> String {
    format!(
        "  {} {} {} {} {}\n",
        "🔐".blue().bold(),
        "SHA-256".blue().bold(),
        path.bold(),
        value.field_str("hash").bold(),
        "— implemented in the engine, over the file's own bytes".dimmed()
    )
    .trim_end()
    .to_string()
}

/// The version store: real objects, with the store's own scope named.
fn render_versions(value: &Value, path: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🕰".blue().bold(),
        "Versions".blue().bold(),
        path.bold()
    ));
    let (versions, hidden) = bounded(match value.field("versions") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    if versions.is_empty() {
        let reason = value.field_str("reason");
        out.push_str(&format!(
            "  └─ {} the store holds no stored version of {}{}\n",
            "○".dimmed(),
            path,
            if reason.is_empty() { String::new() } else { format!(" — {reason}") }
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "ID".into(),
        "SIZE".into(),
        "CREATED".into(),
        "MESSAGE".into(),
        "HASH".into(),
    ]);
    for version in &versions {
        table = table.row(vec![
            Cell::plain(version.field_str("version_id")),
            human_bytes(version.field("size").as_f64().unwrap_or(0.0) as u64).into(),
            stamp_label(version.field_str("created_at")).into(),
            version.field_str("message").into(),
            truncate_digest(version.field_str("hash")).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more version(s) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

fn render_branches(value: &Value, path: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🌿".blue().bold(),
        "Branches".blue().bold(),
        path.bold()
    ));
    let (branches, hidden) = bounded(match value.field("branches") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    if branches.is_empty() {
        out.push_str(&format!(
            "  └─ {} the store holds no branch for {path} (a stable zero total, never a fake)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "BRANCH".into(),
        "BASE".into(),
        "CREATED".into(),
        "ID".into(),
    ]);
    for branch in &branches {
        table = table.row(vec![
            Cell::plain(branch.field_str("name")),
            branch.field_str("base_version").into(),
            stamp_label(branch.field_str("created_at")).into(),
            branch.field_str("branch_id").into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more branch(es) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

fn render_store(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "📦".blue().bold(),
        "Content store".blue().bold(),
        value.field_str("path").bold(),
        "— SHA-256 digest, one real object".dimmed()
    ));
    out.push_str(&format!(
        "  ├─ Digest     {} {}\n",
        value.field_str("hash").bold(),
        format!("({})", value.field_str("algorithm")).dimmed()
    ));
    out.push_str(&format!(
        "  ├─ Size       {}\n",
        human_bytes(value.field("size_bytes").as_f64().unwrap_or(0.0) as u64)
    ));
    let reason = value.field_str("reason");
    out.push_str(&format!(
        "  └─ Scope      {}\n",
        if reason.is_empty() {
            "the backend's own store beside the host's cache root".to_string()
        } else {
            reason
        }
    ));
    out.trim_end().to_string()
}

fn render_dedupe(value: &Value, path: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🧬".blue().bold(),
        "Dedupe".blue().bold(),
        path.bold()
    ));
    out.push_str(&format!(
        "  ├─ Saved      {} over the paths that share this digest\n",
        value.field_str("saved_human")
    ));
    out.push_str(&format!(
        "  ├─ References {} path(s) the host's own inode sharing names\n",
        value.field_str("references")
    ));
    let reason = value.field_str("reason");
    out.push_str(&format!(
        "  └─ Measured   {}\n",
        if reason.is_empty() {
            "the host's own inode sharing".to_string()
        } else {
            reason
        }
    ));
    out.trim_end().to_string()
}

/// `watch`: the kernel's own inotify records, each one a real event.
fn render_watch(value: &Value, path: &str) -> String {
    let (events, hidden) = bounded(match value {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} — the kernel's own inotify records, no polling thread\n",
        "👁".blue().bold(),
        "Watch".blue().bold(),
        path.bold(),
        format!("({} event(s))", events.len()).dimmed()
    ));
    if events.is_empty() {
        out.push_str(&format!(
            "  └─ {} the watch pulled no event in the bounded rounds (an honest empty — the watch is real)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["".into(), "EVENT".into(), "PATH".into(), "WHEN".into()]);
    for event in &events {
        let kind = event.field_str("kind");
        let glyph = match kind.as_str() {
            "create" => Cell::styled("＋", Sgr::Green),
            "delete" => Cell::styled("－", Sgr::Red),
            "modify" => Cell::styled("～", Sgr::Yellow),
            "rename" | "move" => Cell::styled("↔", Sgr::Cyan),
            _ => Cell::plain("▢"),
        };
        let old = event.field_str("old_path");
        let path_cell = if old.is_empty() {
            event.field_str("path")
        } else {
            format!("{} → {}", old, event.field_str("path"))
        };
        table = table.row(vec![
            glyph.with_text(&format!(" {kind}")),
            path_cell.into(),
            stamp_label(event.field_str("timestamp")).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more event(s) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

fn render_special(value: &Value, token: &str) -> String {
    format!(
        "  {} {} {} {}\n",
        "📍".blue().bold(),
        "Special path".blue().bold(),
        token.bold(),
        format!("→ {}", value.field_str("path")).dimmed()
    )
    .trim_end()
    .to_string()
}

fn render_trash(value: &Value) -> String {
    let mut out = String::new();
    let (entries, hidden) = bounded(match value.field("entries") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    out.push_str(&format!(
        "  {} {} {} — the host's own freedesktop trash\n",
        "🗑".blue().bold(),
        "Trash".blue().bold(),
        format!("({})", entries.len()).dimmed()
    ));
    if entries.is_empty() {
        out.push_str(&format!(
            "  └─ {} the trash holds nothing (a stable zero total — the directory was read)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "ENTRY".into(),
        "ORIGINAL PATH".into(),
        "SIZE".into(),
        "DELETED".into(),
    ]);
    for entry in &entries {
        table = table.row(vec![
            Cell::plain(entry.field_str("entry_id")),
            entry.field_str("original_path").into(),
            human_bytes(entry.field("size").as_f64().unwrap_or(0.0) as u64).into(),
            stamp_label(entry.field_str("deleted_at")).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more entr(y|ies) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

fn render_recents(value: &Value) -> String {
    let mut out = String::new();
    let (files, hidden) = bounded(match value.field("files") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    out.push_str(&format!(
        "  {} {} {} — the host's own recently-used record\n",
        "🕘".blue().bold(),
        "Recent files".blue().bold(),
        format!("({})", files.len()).dimmed()
    ));
    if files.is_empty() {
        out.push_str(&format!(
            "  └─ {} the host holds no recently-used record ({} reported, never a fabricated history)\n",
            "○".dimmed(),
            "getRecentFiles"
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "PATH".into(),
        "MIME".into(),
        "RECORDED".into(),
    ]);
    for file in &files {
        table = table.row(vec![
            Cell::plain(file.field_str("path")),
            file.field_str("mime_type").into(),
            stamp_label(file.field_str("timestamp")).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more file(s) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

fn render_app(value: &Value, mime: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} — the host's own mimeapps.list / mimeinfo.cache\n",
        "🚀".blue().bold(),
        "Default app".blue().bold(),
        mime.bold()
    ));
    out.push_str(&format!("  └─ {} {}", "◉".green(), value.field_str("name").bold()));
    let metadata = match value.field("metadata") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    for pair in &metadata {
        let value = pair.field_str("value");
        if !value.is_empty() {
            out.push_str(&format!(
                "\n     {} {}",
                pair.field_str("key").dimmed(),
                value
            ));
        }
    }
    out.trim_end().to_string()
}

fn render_grep(value: &Value, path: &str, query: &str) -> String {
    let mut out = String::new();
    let (hits, hidden) = bounded(match value.field("hits") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    });
    out.push_str(&format!(
        "  {} {} {} {} {} — real lines, read natively\n",
        "🧵".blue().bold(),
        "Grep".blue().bold(),
        path.bold(),
        format!("({})", hits.len()).dimmed(),
        format!("for {query:?}").dimmed()
    ));
    if hits.is_empty() {
        out.push_str(&format!(
            "  └─ {} no line under {path} contains {query:?} (the files were read)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["".into(), "PATH".into(), "LINE".into(), "TEXT".into()]);
    for hit in &hits {
        table = table.row(vec![
            Cell::plain("·"),
            hit.field_str("path").into(),
            hit.field_str("line").into(),
            crate::widgets::truncate_to(&hit.field_str("snippet"), 72).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more hit(s) the host's own read found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

fn render_compare(value: &Value, left: &str, right: &str) -> String {
    let identical = value.field_bool("identical");
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} {}\n",
        "⇄".blue().bold(),
        "Compare".blue().bold(),
        left,
        right,
        if identical { "◉ identical".green().to_string() } else { "◐ different".yellow().to_string() }
    ));
    if !identical {
        let diff = value.field_str("diff");
        if !diff.is_empty() {
            out.push_str(&diff);
        }
    }
    out.trim_end().to_string()
}

fn render_health(value: &Value, path: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🩺".blue().bold(),
        "Filesystem health".blue().bold(),
        path.bold()
    ));
    let status = value.field_str("status");
    let glyph = match status.as_str() {
        "healthy" => Cell::styled("◉", Sgr::Green),
        "degraded" | "checking" => Cell::styled("◐", Sgr::Yellow),
        _ => Cell::plain("○"),
    };
    out.push_str(&format!(
        "  ├─ Status     {} {}\n",
        glyph,
        if status.is_empty() { "— the host did not answer".to_string() } else { status }
    ));
    out.push_str(&format!(
        "  ├─ Errors     {} · warnings {}\n",
        value.field_str("errors"),
        value.field_str("warnings")
    ));
    let details = value.field_str("details");
    if !details.is_empty() {
        out.push_str(&format!("  ├─ Details    {}\n", details));
    }
    let reason = value.field_str("reason");
    out.push_str(&format!(
        "  └─ Measured   {}\n",
        if reason.is_empty() {
            "the read-only half: statvfs state, space and the mount's type".to_string()
        } else {
            reason
        }
    ));
    out.trim_end().to_string()
}

fn render_quota(value: &Value, path: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "📊".blue().bold(),
        "Quota".blue().bold(),
        path.bold()
    ));
    out.push_str(&format!(
        "  ├─ Size       {} of {}\n",
        human_bytes(value.field("current_size_bytes").as_f64().unwrap_or(0.0) as u64),
        human_bytes(value.field("max_size_bytes").as_f64().unwrap_or(0.0) as u64)
    ));
    out.push_str(&format!(
        "  ├─ Files      {} of {}\n",
        value.field_str("current_files"),
        value.field_str("max_files")
    ));
    let reason = value.field_str("reason");
    out.push_str(&format!(
        "  └─ Measured   {}\n",
        if reason.is_empty() {
            "the mount's own statvfs totals".to_string()
        } else {
            reason
        }
    ));
    out.trim_end().to_string()
}

fn render_caps(value: &Value) -> String {
    let Value::List(caps) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — what this backend serves\n",
        "🔑".blue().bold(),
        "Files caps".blue().bold()
    ));
    for (index, cap) in caps.iter().enumerate() {
        let branch = if index + 1 == caps.len() { "└─" } else { "├─" };
        out.push_str(&format!("  {} {} {}\n", branch, "◉".green(), cap.display()));
    }
    out.trim_end().to_string()
}

/// A gated write's answer once the gate accepted it.
fn render_verb(value: &Value) -> String {
    format!(
        "  {} {} {} {}",
        "✎".green(),
        value.field_str("verb").bold(),
        value.field_str("target"),
        "— the engine's own host-mutating path, accepted".dimmed()
    )
    .trim_end()
    .to_string()
}

// ── the shell's own small helpers ─────────────────────────────────────────

/// A nanosecond timestamp as the shell prints it: a local wall-clock reading,
/// or an em dash when the host reported no time (0 is never invented).
pub fn stamp_label(nanos_text: String) -> String {
    let nanos = nanos_of(&nanos_text);
    if nanos == 0 {
        return "—".to_string();
    }
    let secs = (nanos / 1_000_000_000) as i64;
    let millis = (nanos % 1_000_000_000) / 1_000_000;
    let civil = civil_from_secs(secs);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        civil.0, civil.1, civil.2, civil.3, civil.4, civil.5, millis
    )
}

fn nanos_of(text: &str) -> u64 {
    text.parse::<u64>().unwrap_or(0)
}

/// The contract's own civil kernel (`days_from_civil`'s inverse), so the shell
/// never reaches for chrono's epoch math — the same rule the time domain holds.
fn civil_from_secs(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);
    // Howard Hinnant's days_from_civil, inverted: the arithmetic kernels the
    // contract's calendar types are declared with.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m,
        d,
        (seconds_of_day / 3_600) as u32,
        ((seconds_of_day % 3_600) / 60) as u32,
        (seconds_of_day % 60) as u32,
    )
}

fn yes_no(yes: bool) -> Cell {
    if yes {
        Cell::styled("◉", Sgr::Green)
    } else {
        Cell::styled("○", Sgr::Dimmed)
    }
}

/// A digest's own first 16 hex digits, marked as cut (the shell never hides
/// that it truncated).
fn truncate_digest(digest: String) -> String {
    if digest.len() <= 16 {
        return digest;
    }
    format!("{}…", &digest[..16])
}
