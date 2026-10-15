//! `files` — the domain command module on the NEW engine.
//!
//! It renders from the engine's OWNED types (`FileInfo`, `DirectoryEntry`,
//! `Page<T>`, `DiskUsage`, `ExtendedMetadata`, `FileEvent`, `FileVersion`, …)
//! and calls the engine's native mechanisms directly — `statx` metadata,
//! `getdents64` walks, the `mmap` read hot path, `copy_file_range` copies,
//! `inotify` watches, `statvfs` disk usage, SHA-256 over the file's own bytes,
//! the file's own user xattrs and the content-addressed store beside the host's
//! cache root. There is no `cp -r` subprocess anywhere (the archive body's
//! condemned past) and no capnp reader in this module.
//!
//! ## The read surface, and the write surface
//!
//! Every read below is live: the listing is `getdents64`, the stat is `statx`,
//! the cat is the handle surface's `mmap` window, the watch is the kernel's own
//! inotify events, `du` is `statvfs` plus the mount row the host publishes.
//!
//! Every host-mutating verb (`mkdir`, `touch`, `rm`, `mv`, `cp`, `write`,
//! `link`, `lock`, `unlock`) is gated exactly as `network`'s and `clipboard`'s
//! are: without the accepted write the command refuses and names what it would
//! have changed, and with it the engine's own code-complete path runs (charter
//! law #8 — the live bring-back proof is founder-run).
//!
//! ## What this host answers with a measured reason
//!
//! The version store, the branches inside it, the dedupe answer and the
//! snapshots are the BACKEND'S OWN content-addressed store: real objects on
//! disk beside the host's cache root, and named as such per row (the host
//! carries no VCS metadata for a path it never put under one; this host's
//! filesystem has no reflink and no snapshot-capable mount — both measured
//! from `/proc/mounts`). `getLocks` is exactly what THIS process holds, because
//! the kernel publishes no portable lock table readable by this uid. Quota
//! reads stop where this uid's privilege does and say so. Never a fake empty,
//! never a fabricated value.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::files::types::{
    DirectoryFilter, FileKind, OpenMode, OpenRequest, Pagination, SortField, SortOrder, SpecialPath,
};
use smart_lib::domains::files::FilesBackend;

/// The engine seam this shell speaks to — the per-domain backend trait object.
pub type Backend = dyn FilesBackend;

/// Every page read this command module issues asks for everything (the shell
/// slices with the pipe engine); the backend still bounds each pull.
fn all() -> Pagination {
    Pagination {
        offset: 0,
        limit: u32::MAX,
    }
}

/// `smart files` — the host's own files and directories, read natively.
#[derive(Parser, Debug)]
pub struct FilesCmd {
    #[command(subcommand)]
    pub action: Option<FilesAction>,
}

#[derive(Subcommand, Debug)]
pub enum FilesAction {
    /// The directory listing (the default view): `getdents64`, filtered,
    /// sorted, in the shell's table.
    Ls {
        /// The directory to list (defaults to this process's own cwd).
        path: Option<String>,
        /// Include dot-files.
        #[arg(long)]
        all: bool,
        /// A plain-name glob (`*.rs`).
        #[arg(long)]
        pattern: Option<String>,
        /// Sort by name/size/modified/created/accessed.
        #[arg(long, default_value = "name")]
        sort: String,
        /// Sort ascending or descending.
        #[arg(long, default_value = "asc")]
        order: String,
        /// Descend into subdirectories.
        #[arg(long)]
        recursive: bool,
    },
    /// A name search under a path (`searchFiles` — the backend's own glob).
    Find {
        /// The directory to search.
        path: String,
        /// The plain-name glob to match (`*.rs`).
        #[arg(long)]
        name: Option<String>,
        /// Only directories.
        #[arg(long)]
        dirs: bool,
        /// Only regular files.
        #[arg(long)]
        files: bool,
    },
    /// A bounded tree walk (`walkTree`), depth-stated.
    Tree {
        /// The root of the walk.
        path: String,
        /// How deep the walk descends.
        #[arg(long, default_value = "3")]
        depth: u32,
    },
    /// One path's `statx` metadata.
    Stat {
        /// The path to stat.
        path: String,
    },
    /// The extended metadata: statx plus the file's own user xattrs, inode,
    /// link count, owner and mime type.
    Xattrs {
        /// The path to describe.
        path: String,
    },
    /// Read a file through the handle surface — the kernel's own `mmap`
    /// window — rendered honestly: text when the bytes are text.
    Cat {
        /// The file to read.
        path: String,
        /// The window's offset in bytes.
        #[arg(long, default_value = "0")]
        offset: u64,
        /// How many bytes to read (default: the whole file).
        #[arg(long)]
        bytes: Option<u32>,
    },
    /// Disk usage (`statvfs`) with human sizes, plus the mount this host
    /// publishes for it.
    Du {
        /// The path whose filesystem is measured (defaults to the cwd).
        path: Option<String>,
    },
    /// The mount this path lives on, in the host's own words (`/proc/mounts`).
    Mounts {
        /// The path to resolve a mount for (defaults to the cwd).
        path: Option<String>,
    },
    /// The `fcntl` advisory record locks THIS process holds (`getLocks`).
    Locks {
        /// The path to list locks for (default: every lock this process holds).
        path: Option<String>,
    },
    /// SHA-256 over the file's own bytes (`getFileHash`).
    Hash {
        /// The file to hash.
        path: String,
    },
    /// The version store's own versions of a path (`getFileVersions`).
    Versions {
        /// The path whose stored versions are listed.
        path: String,
    },
    /// The branches the store holds for a path (`listBranches`).
    Branches {
        /// The path whose branches are listed.
        path: String,
    },
    /// Store a path's bytes content-addressed (`storeContentAddressed`) — a
    /// real object in the store, named by its own digest.
    Store {
        /// The path whose bytes are stored (`--content` names it).
        #[arg(long = "content", value_name = "PATH")]
        content: Option<String>,
        /// The same path, positionally.
        path: Option<String>,
    },
    /// Measure the host's own inode sharing for a path (`deduplicate`).
    Dedupe {
        /// The path to measure.
        path: String,
    },
    /// A real `inotify` watch: the kernel's own events, pulled in bounded
    /// rounds until the caller's count is met.
    Watch {
        /// The directory to watch.
        path: String,
        /// How many pull rounds to take before stopping.
        #[arg(long, default_value = "5")]
        count: u32,
        /// The pause between rounds, in milliseconds.
        #[arg(long, default_value = "500")]
        interval: u64,
    },
    /// An XDG special path, resolved by the host (`getSpecialPath`).
    Special {
        /// The token: home, documents, downloads, music, pictures, videos,
        /// desktop, templates, public, trash, cache, config, data, runtime.
        token: String,
    },
    /// The host's own freedesktop trash (`listTrash`).
    Trash,
    /// The host's own recently-used record (`getRecentFiles`).
    Recents,
    /// The host's own default application for a mime type (`getDefaultApp`).
    App {
        /// The mime type, e.g. `text/html`.
        mime: String,
    },
    /// A content search over real lines (`searchFileContent`).
    Grep {
        /// The file or directory to search.
        path: String,
        /// The plain substring to find.
        query: String,
    },
    /// Compare two files byte-for-byte (`compareFiles`).
    Compare {
        /// The first path.
        left: String,
        /// The second path.
        right: String,
    },
    /// The measured half of filesystem health (`getFilesystemHealth`).
    Health {
        /// The path whose filesystem is measured.
        path: String,
    },
    /// The measured half of the quota (`getQuota`).
    Quota {
        /// The path whose quota is read.
        path: String,
    },
    /// This backend's honest capability tokens.
    Caps,
    // ── the host-mutating surface: every one gated ─────────────────────────
    /// Create a directory (host-mutating — gated).
    Mkdir {
        /// The directory to create.
        path: String,
        /// Create missing parents too.
        #[arg(long)]
        parents: bool,
    },
    /// Touch a path's timestamps (host-mutating — gated).
    Touch {
        /// The path to touch.
        path: String,
    },
    /// Remove a file or a tree (host-mutating — gated).
    Rm {
        /// The path to remove.
        path: String,
        /// Remove a directory tree.
        #[arg(long)]
        recursive: bool,
    },
    /// Rename/move a path (host-mutating — gated).
    Mv {
        /// The source path.
        src: String,
        /// The destination path.
        dst: String,
    },
    /// Copy a file through the kernel's own `copy_file_range`
    /// (host-mutating — gated).
    Cp {
        /// The source path.
        src: String,
        /// The destination path.
        dst: String,
    },
    /// Write bytes into a file (host-mutating — gated).
    Write {
        /// The path to write.
        path: String,
        /// The text to write.
        text: String,
        /// Create missing parents too.
        #[arg(long)]
        parents: bool,
    },
    /// Create a symbolic or hard link (host-mutating — gated).
    Link {
        /// The link's target.
        target: String,
        /// The link path.
        link: String,
        /// A hard link rather than a symbolic one.
        #[arg(long)]
        hard: bool,
    },
    /// Take the kernel's own advisory record lock on a path
    /// (host-touching — gated: the lock is taken on a descriptor the backend
    /// opens, which may create the file).
    Lock {
        /// The path to lock.
        path: String,
        /// `shared` or `exclusive` (the default).
        #[arg(long, default_value = "exclusive")]
        kind: String,
        /// The owner label the kernel's lock table records.
        #[arg(long, default_value = "smart")]
        owner: String,
    },
    /// Release a lock this process holds (gated).
    Unlock {
        /// The lock id to release.
        lock_id: u64,
    },
}

/// Whether the caller accepted the host write (the REPL prompts; one-shot mode
/// passes the `--force` flag through) — the C1 gate, re-used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// The path a command's optional argument names, or this process's own cwd
/// (the engine's own read — never a `pwd` shellout).
fn path_or_cwd(path: Option<String>, backend: &Backend) -> SmartResult<String> {
    match path {
        Some(p) => Ok(p),
        None => backend.get_current_directory().map_err(SmartError::from),
    }
}

/// Parse the `--sort` token through the engine's own vocabulary.
fn sort_field(token: &str) -> SmartResult<SortField> {
    match SortField::from_token(token) {
        SortField::Unknown => Err(SmartError::invalid_argument(format!(
            "a sort field is name/size/modified/created/accessed, got {token:?}"
        ))),
        field => Ok(field),
    }
}

/// Parse the `--order` token through the engine's own vocabulary.
fn sort_order(token: &str) -> SmartResult<SortOrder> {
    match token {
        "asc" | "ascending" | "up" => Ok(SortOrder::Ascending),
        "desc" | "descending" | "down" => Ok(SortOrder::Descending),
        other => Err(SmartError::invalid_argument(format!(
            "an order is asc or desc, got {other:?}"
        ))),
    }
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &FilesCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(FilesAction::Ls { .. }) => render::ls_value(cmd, backend),
        Some(FilesAction::Find {
            path,
            name,
            dirs,
            files,
        }) => render::find_value(backend, path, name.as_deref(), *dirs, *files),
        Some(FilesAction::Tree { path, depth }) => render::tree_value(backend, path, *depth),
        Some(FilesAction::Stat { path }) => render::stat_value(backend, path),
        Some(FilesAction::Xattrs { path }) => render::xattrs_value(backend, path),
        Some(FilesAction::Cat { path, offset, bytes }) => {
            render::cat_value(backend, path, *offset, *bytes)
        }
        Some(FilesAction::Du { path }) => render::du_value(backend, path_or_cwd(path.clone(), backend)?),
        Some(FilesAction::Mounts { path }) => {
            render::mounts_value(backend, path_or_cwd(path.clone(), backend)?)
        }
        Some(FilesAction::Locks { path }) => render::locks_value(backend, path.as_deref()),
        Some(FilesAction::Hash { path }) => render::hash_value(backend, path),
        Some(FilesAction::Versions { path }) => render::versions_value(backend, path),
        Some(FilesAction::Branches { path }) => render::branches_value(backend, path),
        Some(FilesAction::Store { content, path }) => {
            let target = content
                .clone()
                .or_else(|| path.clone())
                .ok_or_else(|| SmartError::invalid_argument("store names a path: `files store --content <PATH>`"))?;
            render::store_value(backend, &target)
        }
        Some(FilesAction::Dedupe { path }) => render::dedupe_value(backend, path),
        Some(FilesAction::Watch {
            path,
            count,
            interval,
        }) => render::watch_value(backend, path, *count, *interval),
        Some(FilesAction::Special { token }) => render::special_value(backend, token),
        Some(FilesAction::Trash) => render::trash_value(backend),
        Some(FilesAction::Recents) => render::recents_value(backend),
        Some(FilesAction::App { mime }) => render::app_value(backend, mime),
        Some(FilesAction::Grep { path, query }) => render::grep_value(backend, path, query),
        Some(FilesAction::Compare { left, right }) => render::compare_value(backend, left, right),
        Some(FilesAction::Health { path }) => render::health_value(backend, path),
        Some(FilesAction::Quota { path }) => render::quota_value(backend, path),
        Some(FilesAction::Caps) => Ok(render::caps_value(backend)),
        // ── the gated writes ───────────────────────────────────────────────
        Some(FilesAction::Mkdir { path, parents }) => {
            guard_host_write(guard, &format!("files mkdir {path}"))?;
            backend
                .create_directory(path, *parents)
                .map_err(SmartError::from)?;
            Ok(render::verb_value("mkdir", path))
        }
        Some(FilesAction::Touch { path }) => {
            guard_host_write(guard, &format!("files touch {path}"))?;
            backend.touch(path).map_err(SmartError::from)?;
            Ok(render::verb_value("touch", path))
        }
        Some(FilesAction::Rm { path, recursive }) => {
            guard_host_write(guard, &format!("files rm {path}"))?;
            backend
                .remove_file(path, *recursive)
                .map_err(SmartError::from)?;
            Ok(render::verb_value("rm", path))
        }
        Some(FilesAction::Mv { src, dst }) => {
            guard_host_write(guard, &format!("files mv {src} {dst}"))?;
            backend.rename_file(src, dst).map_err(SmartError::from)?;
            Ok(render::verb_value("mv", &format!("{src} → {dst}")))
        }
        Some(FilesAction::Cp { src, dst }) => {
            guard_host_write(guard, &format!("files cp {src} {dst}"))?;
            backend.copy_file(src, dst).map_err(SmartError::from)?;
            Ok(render::verb_value("cp", &format!("{src} → {dst}")))
        }
        Some(FilesAction::Write { path, text, parents }) => {
            guard_host_write(guard, &format!("files write {path}"))?;
            backend
                .create_file(path, text.as_bytes(), *parents)
                .map_err(SmartError::from)?;
            Ok(render::verb_value("write", path))
        }
        Some(FilesAction::Link { target, link, hard }) => {
            guard_host_write(guard, &format!("files link {target} {link}"))?;
            backend
                .link_file(target, link, *hard)
                .map_err(SmartError::from)?;
            Ok(render::verb_value(
                "link",
                &format!("{target} → {link} ({})", if *hard { "hard" } else { "symbolic" }),
            ))
        }
        Some(FilesAction::Lock { path, kind, owner }) => {
            guard_host_write(guard, &format!("files lock {path}"))?;
            let request = smart_lib::domains::files::types::LockFileRequest {
                path: path.clone(),
                kind: match kind.as_str() {
                    "shared" | "read" => smart_lib::domains::files::types::LockKind::Shared,
                    _ => smart_lib::domains::files::types::LockKind::Exclusive,
                },
                owner: owner.clone(),
                timeout: smart_lib::core::values::Duration::from_nanos(0),
                metadata: Vec::new(),
            };
            let lock = backend.lock_file(&request).map_err(SmartError::from)?;
            let kind_word = match lock.kind {
                smart_lib::domains::files::types::LockKind::Shared => "shared",
                smart_lib::domains::files::types::LockKind::Exclusive => "exclusive",
                smart_lib::domains::files::types::LockKind::Custom => "custom",
                smart_lib::domains::files::types::LockKind::Unknown => "unknown",
            };
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("lock".into())),
                ("path".into(), Value::String(lock.lock_path())),
                ("lock_id".into(), Value::Number(lock.lock_id as f64)),
                ("kind".into(), Value::String(kind_word.to_string())),
                ("owner".into(), Value::String(lock.owner.clone())),
            ]))
        }
        Some(FilesAction::Unlock { lock_id }) => {
            guard_host_write(guard, &format!("files unlock {lock_id}"))?;
            backend.unlock_file(*lock_id).map_err(SmartError::from)?;
            Ok(render::verb_value("unlock", &format!("lock {lock_id}")))
        }
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(cmd: &FilesCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

pub mod render;

/// The host-write gate (law #8, the C1 shape): a mutating verb proceeds only
/// when the caller accepted the write.
fn guard_host_write(guard: WriteGuard, verb: &str) -> SmartResult<()> {
    if guard.allowed {
        return Ok(());
    }
    Err(SmartError::not_supported(format!(
        "refusing to run `{verb}`: it CHANGES host state. In the REPL confirm the prompt, or pass \
         --force once you accept the write (the live bring-back proof is founder-run)."
    )))
}

/// The files selector tree, rendered from the engine's owned types — the same
/// values the command surface renders, so a selector and a command can never
/// disagree. `@files/entries`, `@files/cwd`, `@files/caps`, `@files/du`,
/// `@files/home` resolve from here.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let cwd = backend.get_current_directory().map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("entries".into(), render::ls_path_value(backend, &cwd, false, None, SortField::Name, SortOrder::Ascending, false)?),
        ("cwd".into(), Value::String(cwd.clone())),
        ("caps".into(), render::caps_value(backend)),
        (
            "du".into(),
            render::du_value(backend, cwd.clone()).unwrap_or_else(|_| Value::Null),
        ),
        (
            "home".into(),
            render::special_value(backend, "home").unwrap_or_else(|_| Value::Null),
        ),
    ]))
}

/// A read-only listing over the engine's own filter vocabulary.
pub(crate) fn listing_filter(
    patterns: Vec<String>,
    include_hidden: bool,
    sort_by: SortField,
    sort_order: SortOrder,
    recursive: bool,
) -> DirectoryFilter {
    DirectoryFilter {
        patterns,
        include_hidden,
        sort_by,
        sort_order,
        recursive,
        page: all(),
        annotations: Vec::new(),
    }
}

/// The kind token a caller names for `--dirs`/`--files`.
pub(crate) fn kind_matches(kind: FileKind, dirs_only: bool, files_only: bool) -> bool {
    if dirs_only {
        return matches!(kind, FileKind::Directory);
    }
    if files_only {
        return matches!(kind, FileKind::Regular);
    }
    true
}

/// The tokens a `find --type` caller may name.
pub(crate) fn special_path_token(token: &str) -> SpecialPath {
    SpecialPath::from_token(token)
}

/// An `OpenRequest` for the read path: the engine's own handle surface.
pub(crate) fn read_request() -> OpenRequest {
    OpenRequest {
        mode: OpenMode::Read,
        flags: Vec::new(),
    }
}

/// Re-export for the shell and main: the clap shape.
pub use self::{FilesAction as Action, FilesCmd as Cmd};