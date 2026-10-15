//! Owned value types the **files** domain speaks (the types a caller actually
//! touches), plus their conversions to and from the frozen `files.capnp`.
//!
//! Law #7: the capnp state structs are generated; this module is the
//! *hand-written* mirror used at the domain's public/backend seam.
//!
//! ## StringMap, and why it is a `Vec<(String, String)>` here
//!
//! The contract's `T.StringMap` is a recursive tree of entries, so a value that
//! carries `StringMap` carries a *shape*, not a pair. At the seam that shape is
//! an ordered pair list in the contract's own order: reading a map preserves
//! the order the peer wrote and writing one emits that order back, so a
//! round-trip is exact and a consumer that sorts gets to do so deliberately.
//!
//! ## The result unions are total
//!
//! Every `*Result` union in `files.capnp` has an `error` arm and an `unknown`
//! arm beside its payload arm. Mirroring them as Rust enums with those three
//! (or four) variants means a caller cannot forget the failure case: there is
//! no `Option<FileInfo>` whose `None` means "the file does not exist, or the
//! read failed, or the peer sent something new" all at once.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::{AppId, EventId, FileHandleId, FilesystemId, KeyId, MountId, SnapshotId};
use crate::core::values::Timestamp;
use crate::schema::{files_capnp, types_capnp};
use files_capnp as capnp;

/// The contract's `T.StringMap` as an ordered pair list, in the peer's order.
pub type Metadata = Vec<(String, String)>;

/// Pagination, mirroring `T.Pagination` (`offset`, `limit`).
///
/// The contract's rule (conformance §2.3): `limit == 0` means "one", never
/// "all"; `offset + limit` may never exceed `total`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    /// Where the page starts (0-based).
    pub offset: u32,
    /// How many entries the caller will take.
    pub limit: u32,
}

impl Pagination {
    /// Effective limit under the contract rule: 0 means "one".
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A one-page window over an unbounded list, with the stable `total` the
/// contract promises across two identical calls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    /// The entries this page carries.
    pub items: Vec<T>,
    /// The total the caller may page through (stable across identical calls).
    pub total: u64,
}

/// The window of a list that `page` selects, with the total beside it.
pub(crate) fn page_window<T: Clone>(items: &[T], page: Pagination) -> Page<T> {
    let total = items.len() as u64;
    let start = page.offset as usize;
    let end = start
        .saturating_add(page.effective_limit() as usize)
        .min(items.len());
    let items = if start >= items.len() {
        Vec::new()
    } else {
        items[start..end].to_vec()
    };
    Page { items, total }
}

/// Read a `Text` field into an owned string (absent/invalid UTF-8 is empty —
/// the same equivalence the writers emit, so a round-trip stays total).
pub(crate) fn text_field(reader: ::capnp::Result<::capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// Read a `T.StringMap` field into an ordered pair list.
pub(crate) fn metadata_field(reader: Option<types_capnp::string_map::Reader<'_>>) -> Metadata {
    let Some(reader) = reader else {
        return Vec::new();
    };
    reader
        .get_entries()
        .map(|entries| {
            entries
                .iter()
                .map(|e| (text_field(e.get_key()), text_field(e.get_value())))
                .collect()
        })
        .unwrap_or_default()
}

/// Write an ordered pair list into a `T.StringMap` builder.
pub(crate) fn write_metadata<'a>(
    pairs: &Metadata,
    mut builder: types_capnp::string_map::Builder<'a>,
) {
    let mut list = builder.reborrow().init_entries(pairs.len() as u32);
    for (i, (key, value)) in pairs.iter().enumerate() {
        let mut entry = list.reborrow().get(i as u32);
        entry.set_key(key);
        entry.set_value(value);
    }
}

// ═══ Enums (closed vocabularies with the contract's own `unknown` arm) ═════

/// How a handle was opened (`OpenMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpenMode {
    Unknown,
    Read,
    Write,
    ReadWrite,
    Append,
    Truncate,
    Create,
    CreateNew,
    Custom,
}

/// The zero value is the contract's own `unknown` arm — an unset field reads as the enumerant at
/// ordinal 0 (or the contract's catch-all), never as a fabricated value.
impl Default for OpenMode {
    fn default() -> Self {
        OpenMode::Unknown
    }
}

impl OpenMode {
    /// The open token a caller names.
    pub const fn as_token(self) -> &'static str {
        match self {
            OpenMode::Unknown => "unknown",
            OpenMode::Read => "read",
            OpenMode::Write => "write",
            OpenMode::ReadWrite => "readWrite",
            OpenMode::Append => "append",
            OpenMode::Truncate => "truncate",
            OpenMode::Create => "create",
            OpenMode::CreateNew => "createNew",
            OpenMode::Custom => "custom",
        }
    }

    /// Parse a token, unknown-tolerant.
    pub fn from_token(token: &str) -> Self {
        match token {
            "read" => OpenMode::Read,
            "write" => OpenMode::Write,
            "readWrite" => OpenMode::ReadWrite,
            "append" => OpenMode::Append,
            "truncate" => OpenMode::Truncate,
            "create" => OpenMode::Create,
            "createNew" => OpenMode::CreateNew,
            _ => OpenMode::Custom,
        }
    }

    pub(crate) fn to_generated(self) -> capnp::OpenMode {
        use capnp::OpenMode as G;
        match self {
            OpenMode::Unknown => G::Unknown,
            OpenMode::Read => G::Read,
            OpenMode::Write => G::Write,
            OpenMode::ReadWrite => G::ReadWrite,
            OpenMode::Append => G::Append,
            OpenMode::Truncate => G::Truncate,
            OpenMode::Create => G::Create,
            OpenMode::CreateNew => G::CreateNew,
            OpenMode::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(mode: capnp::OpenMode) -> Self {
        use capnp::OpenMode as G;
        match mode {
            G::Unknown => OpenMode::Unknown,
            G::Read => OpenMode::Read,
            G::Write => OpenMode::Write,
            G::ReadWrite => OpenMode::ReadWrite,
            G::Append => OpenMode::Append,
            G::Truncate => OpenMode::Truncate,
            G::Create => OpenMode::Create,
            G::CreateNew => OpenMode::CreateNew,
            G::Custom => OpenMode::Custom,
        }
    }
}

/// One `OpenFlag` of the contract's open flags (`OpenFlag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpenFlag {
    Unknown,
    NonBlocking,
    Append,
    Exclusive,
    Synchronous,
    Hint,
    Custom,
}

/// The zero value is the contract's own `unknown` arm — an unset field reads as the enumerant at
/// ordinal 0 (or the contract's catch-all), never as a fabricated value.
impl Default for OpenFlag {
    fn default() -> Self {
        OpenFlag::Unknown
    }
}

impl OpenFlag {
    pub(crate) fn to_generated(self) -> capnp::OpenFlag {
        use capnp::OpenFlag as G;
        match self {
            OpenFlag::Unknown => G::Unknown,
            OpenFlag::NonBlocking => G::NonBlocking,
            OpenFlag::Append => G::Append,
            OpenFlag::Exclusive => G::Exclusive,
            OpenFlag::Synchronous => G::Synchronous,
            OpenFlag::Hint => G::Hint,
            OpenFlag::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(flag: capnp::OpenFlag) -> Self {
        use capnp::OpenFlag as G;
        match flag {
            G::Unknown => OpenFlag::Unknown,
            G::NonBlocking => OpenFlag::NonBlocking,
            G::Append => OpenFlag::Append,
            G::Exclusive => OpenFlag::Exclusive,
            G::Synchronous => OpenFlag::Synchronous,
            G::Hint => OpenFlag::Hint,
            G::Custom => OpenFlag::Custom,
        }
    }
}

/// What a directory entry or a path is (`FileType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Unknown,
    Regular,
    Directory,
    Symlink,
    Other,
    Custom,
}

/// The zero value is the contract's own `unknown` arm — an unset field reads as the enumerant at
/// ordinal 0 (or the contract's catch-all), never as a fabricated value.
impl Default for FileKind {
    fn default() -> Self {
        FileKind::Unknown
    }
}

impl FileKind {
    /// The open token a caller names.
    pub const fn as_token(self) -> &'static str {
        match self {
            FileKind::Unknown => "unknown",
            FileKind::Regular => "regular",
            FileKind::Directory => "directory",
            FileKind::Symlink => "symlink",
            FileKind::Other => "other",
            FileKind::Custom => "custom",
        }
    }

    /// Parse a token, unknown-tolerant.
    pub fn from_token(token: &str) -> Self {
        match token {
            "unknown" => FileKind::Unknown,
            "regular" => FileKind::Regular,
            "directory" => FileKind::Directory,
            "symlink" => FileKind::Symlink,
            "other" => FileKind::Other,
            _ => FileKind::Custom,
        }
    }

    pub(crate) fn to_generated(self) -> capnp::FileType {
        use capnp::FileType as G;
        match self {
            FileKind::Unknown => G::Unknown,
            FileKind::Regular => G::Regular,
            FileKind::Directory => G::Directory,
            FileKind::Symlink => G::Symlink,
            FileKind::Other => G::Other,
            FileKind::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(kind: capnp::FileType) -> Self {
        use capnp::FileType as G;
        match kind {
            G::Unknown => FileKind::Unknown,
            G::Regular => FileKind::Regular,
            G::Directory => FileKind::Directory,
            G::Symlink => FileKind::Symlink,
            G::Other => FileKind::Other,
            G::Custom => FileKind::Custom,
        }
    }
}

/// How a listing is sorted (`SortField`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortField {
    #[default]
    Unknown,
    Name,
    Size,
    Modified,
    Created,
    Accessed,
    Custom,
}

impl SortField {
    /// Parse a token, unknown-tolerant.
    pub fn from_token(token: &str) -> Self {
        match token {
            "unknown" => SortField::Unknown,
            "name" => SortField::Name,
            "size" => SortField::Size,
            "modified" => SortField::Modified,
            "created" => SortField::Created,
            "accessed" => SortField::Accessed,
            _ => SortField::Custom,
        }
    }

    /// The open token a caller names.
    pub const fn as_token(self) -> &'static str {
        match self {
            SortField::Unknown => "unknown",
            SortField::Name => "name",
            SortField::Size => "size",
            SortField::Modified => "modified",
            SortField::Created => "created",
            SortField::Accessed => "accessed",
            SortField::Custom => "custom",
        }
    }

    pub(crate) fn to_generated(self) -> capnp::SortField {
        use capnp::SortField as G;
        match self {
            SortField::Unknown => G::Unknown,
            SortField::Name => G::Name,
            SortField::Size => G::Size,
            SortField::Modified => G::Modified,
            SortField::Created => G::Created,
            SortField::Accessed => G::Accessed,
            SortField::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(field: capnp::SortField) -> Self {
        use capnp::SortField as G;
        match field {
            G::Unknown => SortField::Unknown,
            G::Name => SortField::Name,
            G::Size => SortField::Size,
            G::Modified => SortField::Modified,
            G::Created => SortField::Created,
            G::Accessed => SortField::Accessed,
            G::Custom => SortField::Custom,
        }
    }
}

/// Which direction a sort runs (`SortOrder`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortOrder {
    #[default]
    Unknown,
    Ascending,
    Descending,
    Custom,
}

impl SortOrder {
    pub(crate) fn to_generated(self) -> capnp::SortOrder {
        use capnp::SortOrder as G;
        match self {
            SortOrder::Unknown => G::Unknown,
            SortOrder::Ascending => G::Ascending,
            SortOrder::Descending => G::Descending,
            SortOrder::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(order: capnp::SortOrder) -> Self {
        use capnp::SortOrder as G;
        match order {
            G::Unknown => SortOrder::Unknown,
            G::Ascending => SortOrder::Ascending,
            G::Descending => SortOrder::Descending,
            G::Custom => SortOrder::Custom,
        }
    }
}

/// What a watch reported (`FileEventType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileEventKind {
    Unknown,
    Create,
    Delete,
    Modify,
    Rename,
    Move,
    Custom,
}

/// The zero value is the contract's own `unknown` arm — an unset field reads as the enumerant at
/// ordinal 0 (or the contract's catch-all), never as a fabricated value.
impl Default for FileEventKind {
    fn default() -> Self {
        FileEventKind::Unknown
    }
}

impl FileEventKind {
    /// The open token a caller names.
    pub const fn as_token(self) -> &'static str {
        match self {
            FileEventKind::Unknown => "unknown",
            FileEventKind::Create => "create",
            FileEventKind::Delete => "delete",
            FileEventKind::Modify => "modify",
            FileEventKind::Rename => "rename",
            FileEventKind::Move => "move",
            FileEventKind::Custom => "custom",
        }
    }

    pub(crate) fn to_generated(self) -> capnp::FileEventType {
        use capnp::FileEventType as G;
        match self {
            FileEventKind::Unknown => G::Unknown,
            FileEventKind::Create => G::Create,
            FileEventKind::Delete => G::Delete,
            FileEventKind::Modify => G::Modify,
            FileEventKind::Rename => G::Rename,
            FileEventKind::Move => G::Move,
            FileEventKind::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(kind: capnp::FileEventType) -> Self {
        use capnp::FileEventType as G;
        match kind {
            G::Unknown => FileEventKind::Unknown,
            G::Create => FileEventKind::Create,
            G::Delete => FileEventKind::Delete,
            G::Modify => FileEventKind::Modify,
            G::Rename => FileEventKind::Rename,
            G::Move => FileEventKind::Move,
            G::Custom => FileEventKind::Custom,
        }
    }
}

/// Which kind of lock a caller asked for (`FileLockType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LockKind {
    Unknown,
    Exclusive,
    Shared,
    Custom,
}

/// The zero value is the contract's own `unknown` arm — an unset field reads as the enumerant at
/// ordinal 0 (or the contract's catch-all), never as a fabricated value.
impl Default for LockKind {
    fn default() -> Self {
        LockKind::Unknown
    }
}

impl LockKind {
    pub(crate) fn to_generated(self) -> capnp::FileLockType {
        use capnp::FileLockType as G;
        match self {
            LockKind::Unknown => G::Unknown,
            LockKind::Exclusive => G::Exclusive,
            LockKind::Shared => G::Shared,
            LockKind::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(kind: capnp::FileLockType) -> Self {
        use capnp::FileLockType as G;
        match kind {
            G::Unknown => LockKind::Unknown,
            G::Exclusive => LockKind::Exclusive,
            G::Shared => LockKind::Shared,
            G::Custom => LockKind::Custom,
        }
    }
}

/// What a lifecycle policy does with an old file (`RetentionAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RetentionAction {
    Unknown,
    Retain,
    Archive,
    Delete,
    Compress,
    Notify,
    Custom,
}

/// The zero value is the contract's own `unknown` arm — an unset field reads as the enumerant at
/// ordinal 0 (or the contract's catch-all), never as a fabricated value.
impl Default for RetentionAction {
    fn default() -> Self {
        RetentionAction::Unknown
    }
}

impl RetentionAction {
    pub(crate) fn to_generated(self) -> capnp::RetentionAction {
        use capnp::RetentionAction as G;
        match self {
            RetentionAction::Unknown => G::Unknown,
            RetentionAction::Retain => G::Retain,
            RetentionAction::Archive => G::Archive,
            RetentionAction::Delete => G::Delete,
            RetentionAction::Compress => G::Compress,
            RetentionAction::Notify => G::Notify,
            RetentionAction::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(action: capnp::RetentionAction) -> Self {
        use capnp::RetentionAction as G;
        match action {
            G::Unknown => RetentionAction::Unknown,
            G::Retain => RetentionAction::Retain,
            G::Archive => RetentionAction::Archive,
            G::Delete => RetentionAction::Delete,
            G::Compress => RetentionAction::Compress,
            G::Notify => RetentionAction::Notify,
            G::Custom => RetentionAction::Custom,
        }
    }
}

/// What the audit trail recorded (`AuditAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuditAction {
    Unknown,
    Read,
    Write,
    Delete,
    Rename,
    PermissionChange,
    Create,
    Custom,
}

/// The zero value is the contract's own `unknown` arm — an unset field reads as the enumerant at
/// ordinal 0 (or the contract's catch-all), never as a fabricated value.
impl Default for AuditAction {
    fn default() -> Self {
        AuditAction::Unknown
    }
}

impl AuditAction {
    pub(crate) fn to_generated(self) -> capnp::AuditAction {
        use capnp::AuditAction as G;
        match self {
            AuditAction::Unknown => G::Unknown,
            AuditAction::Read => G::Read,
            AuditAction::Write => G::Write,
            AuditAction::Delete => G::Delete,
            AuditAction::Rename => G::Rename,
            AuditAction::PermissionChange => G::PermissionChange,
            AuditAction::Create => G::Create,
            AuditAction::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(action: capnp::AuditAction) -> Self {
        use capnp::AuditAction as G;
        match action {
            G::Unknown => AuditAction::Unknown,
            G::Read => AuditAction::Read,
            G::Write => AuditAction::Write,
            G::Delete => AuditAction::Delete,
            G::Rename => AuditAction::Rename,
            G::PermissionChange => AuditAction::PermissionChange,
            G::Create => AuditAction::Create,
            G::Custom => AuditAction::Custom,
        }
    }
}

/// The XDG-named path a caller asks for (`SpecialPathType`, absorbed from
/// `filesystem.capnp` — the ordinals are the absorbed file's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpecialPath {
    Home,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
    Desktop,
    Templates,
    Public,
    Trash,
    Cache,
    Config,
    Data,
    Runtime,
    Other,
}

/// The zero value is the contract's own catch-all arm — an unset field reads as the enumerant at
/// ordinal 0 (or the contract's catch-all), never as a fabricated value.
impl Default for SpecialPath {
    fn default() -> Self {
        SpecialPath::Other
    }
}

impl SpecialPath {
    /// The open token a caller names.
    pub const fn as_token(self) -> &'static str {
        match self {
            SpecialPath::Home => "home",
            SpecialPath::Documents => "documents",
            SpecialPath::Downloads => "downloads",
            SpecialPath::Music => "music",
            SpecialPath::Pictures => "pictures",
            SpecialPath::Videos => "videos",
            SpecialPath::Desktop => "desktop",
            SpecialPath::Templates => "templates",
            SpecialPath::Public => "public",
            SpecialPath::Trash => "trash",
            SpecialPath::Cache => "cache",
            SpecialPath::Config => "config",
            SpecialPath::Data => "data",
            SpecialPath::Runtime => "runtime",
            SpecialPath::Other => "other",
        }
    }

    /// Parse a token, unknown-tolerant.
    pub fn from_token(token: &str) -> Self {
        match token {
            "home" => SpecialPath::Home,
            "documents" => SpecialPath::Documents,
            "downloads" => SpecialPath::Downloads,
            "music" => SpecialPath::Music,
            "pictures" => SpecialPath::Pictures,
            "videos" => SpecialPath::Videos,
            "desktop" => SpecialPath::Desktop,
            "templates" => SpecialPath::Templates,
            "public" => SpecialPath::Public,
            "trash" => SpecialPath::Trash,
            "cache" => SpecialPath::Cache,
            "config" => SpecialPath::Config,
            "data" => SpecialPath::Data,
            "runtime" => SpecialPath::Runtime,
            _ => SpecialPath::Other,
        }
    }

    pub(crate) fn to_generated(self) -> capnp::SpecialPathType {
        use capnp::SpecialPathType as G;
        match self {
            SpecialPath::Home => G::Home,
            SpecialPath::Documents => G::Documents,
            SpecialPath::Downloads => G::Downloads,
            SpecialPath::Music => G::Music,
            SpecialPath::Pictures => G::Pictures,
            SpecialPath::Videos => G::Videos,
            SpecialPath::Desktop => G::Desktop,
            SpecialPath::Templates => G::Templates,
            SpecialPath::Public => G::Public,
            SpecialPath::Trash => G::Trash,
            SpecialPath::Cache => G::Cache,
            SpecialPath::Config => G::Config,
            SpecialPath::Data => G::Data,
            SpecialPath::Runtime => G::Runtime,
            SpecialPath::Other => G::Other,
        }
    }

    pub(crate) fn from_generated(path: capnp::SpecialPathType) -> Self {
        use capnp::SpecialPathType as G;
        match path {
            G::Home => SpecialPath::Home,
            G::Documents => SpecialPath::Documents,
            G::Downloads => SpecialPath::Downloads,
            G::Music => SpecialPath::Music,
            G::Pictures => SpecialPath::Pictures,
            G::Videos => SpecialPath::Videos,
            G::Desktop => SpecialPath::Desktop,
            G::Templates => SpecialPath::Templates,
            G::Public => SpecialPath::Public,
            G::Trash => SpecialPath::Trash,
            G::Cache => SpecialPath::Cache,
            G::Config => SpecialPath::Config,
            G::Data => SpecialPath::Data,
            G::Runtime => SpecialPath::Runtime,
            G::Other => SpecialPath::Other,
        }
    }
}

// ═══ Structs ═════════════════════════════════════════════════════════════

/// What `openFile` asks for (`OpenRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenRequest {
    /// The access mode.
    pub mode: OpenMode,
    /// The flags to open with.
    pub flags: Vec<OpenFlag>,
}

/// A borrowed file handle: a pure reference (`FileHandle`). The path and the
/// open mode are reported by `FileInfo` — a handle that carried them would
/// force a backend to fabricate values a consumer then depends on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileHandle {
    /// The backend's own token for the open file.
    pub id: FileHandleId,
    /// Opaque backend extras.
    pub metadata: Metadata,
}

/// One entry of a directory listing (`DirectoryEntry`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    pub modified: Timestamp,
    pub metadata: Metadata,
}

/// How a listing is filtered and ordered (`DirectoryFilter`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectoryFilter {
    /// Empty = every name matches.
    pub patterns: Vec<String>,
    /// Whether dot-files are listed.
    pub include_hidden: bool,
    pub sort_by: SortField,
    pub sort_order: SortOrder,
    /// Descend into subdirectories.
    pub recursive: bool,
    /// The page window the caller will take.
    pub page: Pagination,
    pub annotations: Metadata,
}

/// What a watch reported (`FileEvent`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEvent {
    pub id: EventId,
    pub timestamp: Timestamp,
    pub kind: FileEventKind,
    pub path: String,
    /// The old path of a rename/move, empty otherwise.
    pub old_path: String,
    pub metadata: Metadata,
}

/// A name search (`FileSearchQuery`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileSearchQuery {
    pub path: String,
    /// A plain-name pattern (glob), not a query language.
    pub pattern: String,
    pub recursive: bool,
    pub page: Pagination,
    pub annotations: Metadata,
}

/// One search hit (`FileSearchResult`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileSearchResult {
    pub path: String,
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    pub modified: Timestamp,
    /// The backend's own relevance score in 0.0..=1.0; 0 when unranked.
    pub score: f32,
    pub metadata: Metadata,
}

/// Space on a filesystem (`DiskUsage`), as `statvfs` reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskUsage {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub percent_used: f32,
    pub mount_point: String,
    pub mount_id: MountId,
    pub filesystem_id: FilesystemId,
    pub metadata: Metadata,
}

/// Everything `statx` knows about a path (`ExtendedMetadata`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtendedMetadata {
    pub path: String,
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    pub created: Timestamp,
    pub modified: Timestamp,
    pub accessed: Timestamp,
    /// The owner label the host reports (`statx`'s uid resolved against the
    /// host's own passwd database). It carries no authority.
    pub owner: String,
    /// The mime type the host's own `mime.types` table maps the extension to,
    /// or an empty type when the host does not know it.
    pub mime_type: String,
    /// A symlink's own target, empty otherwise.
    pub target: String,
    pub is_hidden: bool,
    pub is_read_only: bool,
    pub is_executable: bool,
    /// The content hash, when the backend can report one (never fabricated).
    pub content_hash: Vec<u8>,
    pub compressed_size: u64,
    pub encryption_key: KeyId,
    pub metadata: Metadata,
}

/// The result of a comparison (`ComparisonResult`): the `diff` arm carries the
/// structured difference as `T.DataHandle` bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComparisonResult {
    pub identical: bool,
    /// The diff payload (unified diff text), empty when identical.
    pub diff: Vec<u8>,
    pub metadata: Metadata,
}

/// A lock this backend holds (`FileLock`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileLock {
    pub lock_id: u64,
    pub handle: FileHandle,
    pub kind: LockKind,
    /// The owner label the caller named.
    pub owner: String,
    pub acquired_at: Timestamp,
    pub expires_at: Timestamp,
    pub metadata: Metadata,
}

impl FileLock {
    /// The host path this lock is held on, read out of the handle's own
    /// metadata (the contract's one place a backend may report its
    /// internals). Empty when the backend recorded no path.
    pub fn lock_path(&self) -> String {
        self.handle
            .metadata
            .iter()
            .find(|(key, _)| key == "path")
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }
}

/// What `lockFile` asks for (`LockFileRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockFileRequest {
    pub path: String,
    pub kind: LockKind,
    pub owner: String,
    pub timeout: crate::core::values::Duration,
    pub metadata: Metadata,
}

/// One stored version of a file (`FileVersion`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileVersion {
    pub version_id: u64,
    pub content_hash: Vec<u8>,
    pub created_at: Timestamp,
    pub author: AppId,
    pub message: String,
    pub size: u64,
    pub metadata: Metadata,
}

/// `VersionResult` — the union is total: a version, an error, or an unknown.
#[derive(Clone, Debug, PartialEq)]
pub enum VersionResult {
    Version(FileVersion),
    Error(crate::core::Error),
    Unknown,
}

/// One branch of a file's history (`FileBranch`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileBranch {
    pub branch_id: String,
    pub branch_name: String,
    pub path: String,
    pub base_version: u64,
    pub created_at: Timestamp,
    pub author: String,
    pub metadata: Metadata,
}

/// `BranchResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum BranchResult {
    Branch(FileBranch),
    Error(crate::core::Error),
    Unknown,
}

/// A content-addressed reference (`ContentAddressedRef`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentAddressedRef {
    /// The digest bytes of the hashed algorithm.
    pub hash: Vec<u8>,
    /// The algorithm token that produced `hash`.
    pub algorithm: String,
    pub size_bytes: u64,
    pub metadata: Metadata,
}

/// `ContentRefResult` — the union is total: a ref, a handle, an error, or an
/// unknown.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentRefResult {
    Ref(ContentAddressedRef),
    Handle(FileHandle),
    Error(crate::core::Error),
    Unknown,
}

/// What `getByHash` asks for (`GetByHashRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GetByHashRequest {
    pub hash: Vec<u8>,
    pub algorithm: String,
    pub metadata: Metadata,
}

/// What `createEphemeral` asks for (`EphemeralFileConfig`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EphemeralFileConfig {
    pub delete_after_read: bool,
    pub max_age: crate::core::values::Duration,
    pub max_reads: u32,
}

/// What `createEncrypted` asks for (`CreateEncryptedRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreateEncryptedRequest {
    pub path: String,
    /// The plaintext to seal.
    pub content: Vec<u8>,
    pub key_id: KeyId,
    /// The open algorithm token.
    pub algorithm: String,
    pub parents: bool,
    pub metadata: Metadata,
}

/// What an encrypted file reports (`EncryptedFileInfo`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EncryptedFileInfo {
    pub key_id: KeyId,
    pub algorithm: String,
    pub encrypted_size: u64,
    pub metadata: Metadata,
}

/// `EncryptionInfoResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum EncryptionInfoResult {
    Info(EncryptedFileInfo),
    Error(crate::core::Error),
    Unknown,
}

/// A compression policy (`FileCompressionConfig`): the algorithm is an open
/// token, so a 2046 codec is a string with unknown-tolerance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileCompressionConfig {
    pub algorithm: String,
    pub level: u32,
    pub metadata: Metadata,
}

/// What deduplication has saved (`FileDeduplicationInfo`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDeduplicationInfo {
    pub content_hash: Vec<u8>,
    pub reference_count: u32,
    pub saved_bytes: u64,
    pub metadata: Metadata,
}

/// `FileInfoResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum FileInfoResult {
    Info(FileInfo),
    Error(crate::core::Error),
    Unknown,
}

/// The `FileInfo` a `statFile`/`FileService.stat` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileInfo {
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    pub created: Timestamp,
    pub modified: Timestamp,
    pub accessed: Timestamp,
    pub mime_type: String,
    /// Whether the backend can report this path's content hash.
    pub content_hash: Vec<u8>,
    /// The backend's own extras (inode, device, link count) — reported data.
    pub metadata: Metadata,
}

/// `TextResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum TextResult {
    Value(String),
    Error(crate::core::Error),
    Unknown,
}

/// `FileHashResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum FileHashResult {
    Hash(Vec<u8>),
    Error(crate::core::Error),
    Unknown,
}

/// What a checksum read reports (`ChecksumResult`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumResult {
    pub valid: bool,
    pub stored: Vec<u8>,
    pub computed: Vec<u8>,
    pub metadata: Metadata,
}

/// `LifecyclePolicyResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum LifecyclePolicyResult {
    Policy(LifecyclePolicy),
    Error(crate::core::Error),
    Unknown,
}

/// What one batch operation reports (`BatchResult`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchResult {
    pub completed: u32,
    pub failed: u32,
    /// One error per failed item, in the request's order.
    pub errors: Vec<crate::core::Error>,
    pub metadata: Metadata,
}

/// A batch copy (`BatchCopyRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchCopyRequest {
    pub sources: Vec<String>,
    pub destination: String,
    pub metadata: Metadata,
}

/// A batch move (`BatchMoveRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchMoveRequest {
    pub sources: Vec<String>,
    pub destination: String,
    pub metadata: Metadata,
}

/// A batch delete (`BatchDeleteRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchDeleteRequest {
    pub paths: Vec<String>,
    pub recursive: bool,
    pub metadata: Metadata,
}

/// What a directory walk totals (`DirectoryStats`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DirectoryStats {
    pub total_files: u64,
    pub total_directories: u64,
    pub total_size: u64,
    pub by_type: Vec<FileTypeCount>,
    pub largest_file: String,
    pub oldest_file: String,
    pub newest_file: String,
    pub metadata: Metadata,
}

/// One `byType` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileTypeCount {
    pub kind: FileKind,
    pub count: u64,
    pub total_size: u64,
}

/// What happened to one file (`FileActivity`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileActivity {
    pub timestamp: Timestamp,
    pub actor: String,
    /// An open token: the backend names the action it took.
    pub action: String,
    pub path: String,
    pub metadata: Metadata,
}

/// A retention policy (`LifecyclePolicy`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LifecyclePolicy {
    pub path: String,
    pub action: RetentionAction,
    pub retention_period: crate::core::values::Duration,
    pub min_age: crate::core::values::Duration,
    pub enabled: bool,
    pub description: String,
    pub metadata: Metadata,
}

/// One audit record (`FileAuditEntry`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileAuditEntry {
    pub timestamp: Timestamp,
    pub actor: String,
    pub action: AuditAction,
    pub path: String,
    /// The outcome token the backend recorded.
    pub result: String,
    pub details: String,
    pub metadata: Metadata,
}

/// What an audit subscription filters on (`FileAuditFilter`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileAuditFilter {
    pub path_pattern: String,
    pub actor: String,
    /// Empty = every action.
    pub actions: Vec<AuditAction>,
    pub since: Timestamp,
    pub metadata: Metadata,
}

/// What `analyzeFileContent` asks for (`FileAnalysisRequest`): the analysis
/// kinds are open tokens, so an unknown kind is ignored, never rejected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileAnalysisRequest {
    pub path: String,
    pub analysis_types: Vec<String>,
    pub language: String,
    pub metadata: Metadata,
}

/// One analysis result (`FileAnalysisResult`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileAnalysisResult {
    pub path: String,
    pub analysis_type: String,
    /// The analysis payload as `T.DataHandle` bytes.
    pub result: Vec<u8>,
    pub confidence: f32,
    pub timestamp: Timestamp,
    pub metadata: Metadata,
}

/// A content search (`ContentSearchQuery`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentSearchQuery {
    pub path: String,
    /// A plain substring, not a query language.
    pub query: String,
    pub recursive: bool,
    /// Empty = every mime type.
    pub mime_types: Vec<String>,
    pub page: Pagination,
    pub annotations: Metadata,
}

/// One content hit (`ContentSearchResult`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentSearchResult {
    pub path: String,
    /// The matching line, with the hit left in place.
    pub snippet: String,
    pub score: f32,
    pub line_number: u32,
    pub metadata: Metadata,
}

/// One trash entry (`TrashEntry`), as the host's own trash directory holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrashEntry {
    pub entry_id: String,
    pub original_path: String,
    pub deleted_at: Timestamp,
    pub size: u64,
    pub metadata: Metadata,
}

/// One recently-used file (`RecentFile`), as the host's own record holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentFile {
    pub path: String,
    pub mime_type: String,
    pub timestamp: Timestamp,
    pub metadata: Metadata,
}

/// The application a mime type opens with (`DefaultAppInfo`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefaultAppInfo {
    pub app_id: AppId,
    pub name: String,
    /// The app's icon as `T.DataHandle` bytes, empty when the host carries none.
    pub icon: Vec<u8>,
    pub metadata: Metadata,
}

/// `DefaultAppResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum DefaultAppResult {
    App(DefaultAppInfo),
    Error(crate::core::Error),
    Unknown,
}

/// A filesystem's health (`FilesystemHealth`): `status` is an open token
/// ("healthy", "degraded", "checking"), not a closed enum.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilesystemHealth {
    pub status: String,
    pub last_check: Timestamp,
    pub next_scheduled: Timestamp,
    pub errors: u64,
    pub warnings: u64,
    pub details: String,
    pub metadata: Metadata,
}

/// `FilesystemHealthResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum FilesystemHealthResult {
    Health(FilesystemHealth),
    Error(crate::core::Error),
    Unknown,
}

/// A quota (`FilesystemQuota`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilesystemQuota {
    pub max_size_bytes: u64,
    pub max_files: u64,
    pub current_size_bytes: u64,
    pub current_files: u64,
    pub soft_limit_percent: u8,
}

/// `SnapshotRefResult` — the union is total.
#[derive(Clone, Debug, PartialEq)]
pub enum SnapshotRefResult {
    Snapshot(SnapshotId),
    Error(crate::core::Error),
    Unknown,
}

/// A `FileService` bound to one path: holding it IS the authority to operate on
/// that path and nothing else. There is no permission string and no ACL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundPath {
    /// The host path this service is bound to.
    pub path: String,
    /// The authority token that bound it (opaque to the backend).
    pub authority: u64,
}

impl FileHandle {
    /// A handle over the host's own file descriptor number.
    pub fn new(id: FileHandleId) -> Self {
        FileHandle {
            id,
            metadata: Vec::new(),
        }
    }
}

// ═══ Wire conversions for the structs the tests round-trip ═══════════════

impl OpenRequest {
    /// Read from the generated reader.
    pub fn from_reader(reader: &capnp::open_request::Reader<'_>) -> Self {
        OpenRequest {
            mode: OpenMode::from_generated(reader.get_mode().unwrap_or(capnp::OpenMode::Unknown)),
            flags: reader
                .get_flags()
                .map(|list| list.iter().map(|f| OpenFlag::from_generated(f.expect("an open flag ordinal the schema carries"))).collect())
                .unwrap_or_default(),
        }
    }

    /// Write into the generated builder.
    pub fn write_to<'a>(&self, mut builder: capnp::open_request::Builder<'a>) {
        builder.set_mode(self.mode.to_generated());
        let mut list = builder.reborrow().init_flags(self.flags.len() as u32);
        for (i, flag) in self.flags.iter().enumerate() {
            list.set(i as u32, flag.to_generated());
        }
    }
}

impl DirectoryFilter {
    /// Read from the generated reader.
    pub fn from_reader(reader: &capnp::directory_filter::Reader<'_>) -> Self {
        DirectoryFilter {
            patterns: reader
                .get_patterns()
                .map(|list| list.iter().map(text_field).collect())
                .unwrap_or_default(),
            include_hidden: reader.get_include_hidden(),
            sort_by: SortField::from_generated(reader.get_sort_by().unwrap_or(capnp::SortField::Unknown)),
            sort_order: SortOrder::from_generated(
                reader.get_sort_order().unwrap_or(capnp::SortOrder::Unknown),
            ),
            recursive: reader.get_recursive(),
            page: Pagination {
                offset: reader.get_page().map(|p| p.get_offset()).unwrap_or(0),
                limit: reader.get_page().map(|p| p.get_limit()).unwrap_or(0),
            },
            annotations: metadata_field(reader.get_annotations().ok()),
        }
    }
}

impl DirectoryEntry {
    /// Write into the generated builder.
    pub fn write_to<'a>(&self, mut builder: capnp::directory_entry::Builder<'a>) {
        builder.set_name(&self.name);
        builder.set_path(&self.path);
        builder.set_type(self.kind.to_generated());
        builder.set_size(self.size);
        self.modified.write_to(&mut builder.reborrow().init_modified());
        write_metadata(&self.metadata, builder.reborrow().init_metadata());
    }

    /// Read from the generated reader.
    pub fn from_reader(reader: &capnp::directory_entry::Reader<'_>) -> Self {
        DirectoryEntry {
            name: text_field(reader.get_name()),
            path: text_field(reader.get_path()),
            kind: FileKind::from_generated(reader.get_type().unwrap_or(capnp::FileType::Unknown)),
            size: reader.get_size(),
            modified: reader
                .get_modified()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            metadata: metadata_field(reader.get_metadata().ok()),
        }
    }
}

/// The seam's `type/subtype` spelling of a `T.MimeType` reader.
fn mime_from_reader(reader: &types_capnp::mime_type::Reader<'_>) -> String {
    let ty = text_field(reader.get_type());
    let sub = text_field(reader.get_subtype());
    if sub.is_empty() {
        ty
    } else {
        alloc::format!("{ty}/{sub}")
    }
}

impl FileInfo {
    /// Write into the generated builder.
    pub fn write_to<'a>(&self, mut builder: capnp::file_info::Builder<'a>) {
        builder.set_path(&self.path);
        builder.set_type(self.kind.to_generated());
        builder.set_size(self.size);
        self.created.write_to(&mut builder.reborrow().init_created());
        self.modified.write_to(&mut builder.reborrow().init_modified());
        self.accessed.write_to(&mut builder.reborrow().init_accessed());
        // `T.MimeType` is a struct, so the seam's `type/subtype` string is
        // split back into its two fields on the way out — the same spelling the
        // reader produces, so a round-trip is exact.
        let (ty, sub) = match self.mime_type.split_once('/') {
            Some((ty, sub)) => (ty, sub),
            None => (self.mime_type.as_str(), ""),
        };
        {
            let mut mime = builder.reborrow().init_mime_type();
            mime.set_type(ty);
            mime.set_subtype(sub);
        }
        {
            let mut hash = builder.reborrow().init_content_hash();
            hash.set_bytes(&self.content_hash);
        }
        write_metadata(&self.metadata, builder.reborrow().init_metadata());
    }

    /// Read from the generated reader.
    pub fn from_reader(reader: &capnp::file_info::Reader<'_>) -> Self {
        FileInfo {
            path: text_field(reader.get_path()),
            kind: FileKind::from_generated(reader.get_type().unwrap_or(capnp::FileType::Unknown)),
            size: reader.get_size(),
            created: reader
                .get_created()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            modified: reader
                .get_modified()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            accessed: reader
                .get_accessed()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            mime_type: reader
                .get_mime_type()
                .map(|m| mime_from_reader(&m))
                .unwrap_or_default(),
            // `T.HashOutput` is the contract's {algorithm, bytes} pair: the
            // algorithm is reported as data, the bytes are the digest.
            content_hash: reader
                .get_content_hash()
                .map(|h| h.get_bytes().unwrap_or_default().to_vec())
                .unwrap_or_default(),
            metadata: metadata_field(reader.get_metadata().ok()),
        }
    }
}
