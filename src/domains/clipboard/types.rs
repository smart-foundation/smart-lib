//! Owned value types the clipboard domain speaks (the types a caller actually
//! touches), plus their conversions to and from the frozen contract.
//!
//! Law #7: the capnp state structs are generated; this module is the
//! *hand-written* mirror used at the domain's public/backend seam. It is
//! ergonomic (owned) for the methods this milestone implements, and a compact
//! faithful echo of the ones that are honestly `DEGRADED`/`Unavailable` on a
//! given host — so the trait has one honest, well-typed method per contract
//! method. The `metadata`/`StringMap` bookkeeping the contract carries (and the
//! `T.DataHandle` shared-buffer half) is intentionally NOT mirrored here; a
//! backend that needs the wire does it against `crate::schema::clipboard_capnp`
//! directly.
//!
//! ## The payload is inline here, on purpose
//!
//! `ClipboardContent.data` is `T.DataHandle` on the wire: `inline :Data` for
//! small payloads, `buffer :BufferId` (shared memory) for large ones. This
//! domain is the one clipboard, read and written in one process; the engine
//! carries the payload as owned bytes and a backend that marshals it onto the
//! wire chooses inline-vs-buffer at its layer. Keeping `data :Vec<u8>` here
//! means a fabricated buffer reference — pointing at shared memory that does
//! not exist — can never be produced by the shell half.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::{ClipboardId, ClipboardSourceId};
use crate::schema::{clipboard_capnp, types_capnp};

/// Pagination, mirroring `T.Pagination` (`offset`, `limit`), under the contract
/// rule (conformance §2.3): `limit == 0` means "one", never "all".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pub offset: u32,
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
    pub items: Vec<T>,
    pub total: u64,
}

/// An opaque subscription handle minted by `subscribe_changes`. The clipboard
/// backend is a pull model: subscribing mints a token, `pull_changes` reads the
/// buffered events under the caller's stated bound, and the token is retired on
/// `cancel_changes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// The one clipboard payload: an open content type (a MIME type spelled the way
/// the host spells it, e.g. `"text/plain"`, `"image/png"`) plus the owned bytes.
///
/// The byte count is the size the shell and the size limits reason about; it is
/// the measured length of `data`, not a separately asserted number (a counter
/// is measured or it is absent — the contract's own law).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardContent {
    /// A MIME type as open text ("text/plain", "image/png", or a type nobody
    /// has named yet). `/` is not required: the target atoms on X11 and the
    /// wlr-data-control mime list are both open, so the mirror stays faithful.
    pub mime_type: String,
    pub data: Vec<u8>,
}

impl ClipboardContent {
    /// The measured payload size in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.data.len() as u64
    }

    /// The bytes as UTF-8 text when they are valid UTF-8; `None` for binary
    /// payloads (an image). Never a lossy replacement: a caller that wants a
    /// text preview confirms validity first.
    pub fn as_text(&self) -> Option<&str> {
        core::str::from_utf8(&self.data).ok()
    }
}

/// One entry in the engine-side history ledger (`ClipboardItem`, the fields the
/// shell renders). Ownership is reduced to the pinned flag plus a nominal
/// source id: the full `ClipboardOwnership` lifetime bookkeeping (acquiredAt /
/// expiresAt / ttl) is a host-history concern this milestone does not serve, and
/// carrying empty lifetimes would be a fabrication.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardItem {
    pub id: ClipboardId,
    /// The contract's `T.Timestamp` unit: nanoseconds since the epoch, 0 when
    /// the clock could not be read (never fabricated).
    pub timestamp: u64,
    /// The MIME types this item was offered with.
    pub mime_types: Vec<String>,
    /// Audit-7 categorization tags; empty on this backend (the host selection
    /// carries no tags), reported as the empty set rather than guessed.
    pub tags: Vec<String>,
    /// Audit-7: pinned items are not auto-expired by the size limits.
    pub pinned: bool,
    /// A nominal source identity for the copy; the constant `1` here because a
    /// process's own copies are all this backend's source.
    pub source_id: ClipboardSourceId,
    /// Size of the stored payload in bytes.
    pub size_bytes: u64,
    /// A short preview of the payload for text offerings (empty for binary).
    pub text: String,
}

/// A typed clipboard change event (`ClipboardChange` + `ClipboardEventType`):
/// what changed, when, and under which source — the payload surface a
/// `subscribeChanges`/`pullChanges` consumer reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardChange {
    pub id: ClipboardId,
    pub timestamp: u64,
    pub source_id: ClipboardSourceId,
    pub mime_types: Vec<String>,
    pub event_type: ClipboardEventType,
    /// A bounded preview of the new content for a `copied` event; empty
    /// otherwise. Kept small so a pull never returns a whole image inline.
    pub text: String,
}

/// The contract's event vocabulary (`ClipboardEventType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardEventType {
    Copied,
    Pasted,
    Cleared,
    Expired,
    Custom,
}

impl ClipboardEventType {
    /// The contract token, so the mapping is auditable.
    pub const fn as_token(self) -> &'static str {
        match self {
            ClipboardEventType::Copied => "copied",
            ClipboardEventType::Pasted => "pasted",
            ClipboardEventType::Cleared => "cleared",
            ClipboardEventType::Expired => "expired",
            ClipboardEventType::Custom => "custom",
        }
    }

    /// Read the event type off a generated `ClipboardEventType`.
    pub fn from_generated(g: clipboard_capnp::ClipboardEventType) -> Self {
        match g {
            clipboard_capnp::ClipboardEventType::Copied => ClipboardEventType::Copied,
            clipboard_capnp::ClipboardEventType::Pasted => ClipboardEventType::Pasted,
            clipboard_capnp::ClipboardEventType::Cleared => ClipboardEventType::Cleared,
            clipboard_capnp::ClipboardEventType::Expired => ClipboardEventType::Expired,
            clipboard_capnp::ClipboardEventType::Custom => ClipboardEventType::Custom,
        }
    }
}

/// `getContent @0`'s answer, owned: either the content (with the history id it
/// now occupies, when the backend records a read as a fact — see
/// [`crate::platforms::linux::clipboard`] for why this backend does not), or the
/// contract's first-class *empty* (a distinct answer from a fault).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentOutcome {
    /// The clipboard holds this content.
    Content(ClipboardContent),
    /// The clipboard holds nothing. This is an answer, not a fault — the exact
    /// distinction `ContentResult.empty @1` exists to carry.
    Empty,
}

/// `convertContent @13`'s answer, owned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversionOutcome {
    /// The converted content.
    Content(ClipboardContent),
    /// The requested conversion is not available — a real outcome, not a
    /// failure (`ConversionResult.unsupported @1`).
    Unsupported,
}

/// `validateContent @12`'s answer, owned. "Content does not match its claim" is
/// a security-relevant finding, so it carries a reason rather than a bare bool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// The content matches its claimed MIME type.
    Valid,
    /// The content does not match its claim; the reason names the mismatch.
    Invalid(String),
    /// The backend could not classify the content (unknown, not a guess).
    Unknown,
}

/// `getPreview @11`'s answer, owned (`ClipboardPreview`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewOutcome {
    Preview(ClipboardPreview),
    /// The named item has no preview (no item, or a binary payload with no text).
    Unavailable,
}

/// A low-resolution preview of clipboard content: truncated text, its size, and
/// the preview's own MIME type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardPreview {
    /// Preview text, truncated for text content.
    pub text_preview: String,
    /// Whether the preview is available.
    pub available: bool,
    /// Original content size in bytes.
    pub original_size: u64,
    /// The preview's MIME type.
    pub preview_mime_type: String,
}

/// The configured size limits (`ClipboardSizeLimits`). Each `0` means unlimited
/// per the contract, so a zero is never a fabricated bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipboardSizeLimits {
    /// Maximum size for a single item in bytes (0 = unlimited).
    pub max_item_size: u64,
    /// Maximum total history size in bytes (0 = unlimited).
    pub max_history_size: u64,
    /// Maximum number of items in history (0 = unlimited).
    pub max_history_count: u32,
}

/// The capability surface this clipboard backend advertises (law #6): a set of
/// open tokens naming what is real, so a consumer reading the handle knows what
/// it may do without a permission string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionCapabilities {
    /// The selection transport actually spoken on this host ("x11" / "wayland"
    /// / "none"). A `none` transport is an honest Unavailable, never a fake.
    pub transport: String,
    /// Whether the live selection can be *read* right now.
    pub can_read: bool,
    /// Whether the live selection can be *written* right now (behind the gate).
    pub can_write: bool,
}

// ── Wire conversions for the state structs ──────────────────────────────────

/// Read a `Text` field into an owned string. Absent or non-UTF-8 is the empty
/// string — the same equivalence the writers emit, so the round-trip is total.
pub(crate) fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// Spell a `T.MimeType` the way this domain carries it: `"type/subtype"` (or the
/// bare type when the subtype is empty). `parameters` is not surfaced here — it
/// is a host-nameable extra this backend does not set, and an empty list keeps
/// the round-trip faithful rather than fabricating parameters.
pub(crate) fn mime_text(reader: &types_capnp::mime_type::Reader<'_>) -> String {
    let ty = text_field(reader.get_type());
    let sub = text_field(reader.get_subtype());
    if sub.is_empty() {
        ty
    } else {
        format!("{ty}/{sub}")
    }
}

/// Write a `T.MimeType` from this domain's `"type/subtype"` text. No `'/'`
/// writes subtype-empty; an unrecognised spelling is still written as the type
/// with an empty subtype rather than rejected — the vocabulary is open.
pub(crate) fn set_mime_text(builder: &mut types_capnp::mime_type::Builder<'_>, mime: &str) {
    let (ty, sub) = match mime.split_once('/') {
        Some((t, s)) => (t, s),
        None => (mime, ""),
    };
    builder.set_type(ty);
    builder.set_subtype(sub);
}

/// The payload bytes carried inline in a generated `T.DataHandle`. Only the
/// `inline :Data` arm is materialised: a `buffer :BufferId` reference names
/// shared memory this in-process backend never allocated, so it reads as the
/// empty payload rather than following a handle it cannot resolve (never a
/// fabricated buffer dereference).
pub(crate) fn data_handle_bytes(
    reader: &types_capnp::data_handle::Reader<'_>,
) -> Vec<u8> {
    use types_capnp::data_handle::WhichReader;
    match reader.which() {
        // `inline :Data` carries the bytes directly (capnp's `data` is a byte
        // slice, and the union arm holds it behind a `Result`); `buffer :BufferId`
        // names shared memory this backend never allocated, so any other arm
        // (or a failed inline read) is the empty payload — never a fabricated
        // buffer dereference.
        Ok(WhichReader::Inline(Ok(bytes))) => bytes.to_vec(),
        _ => Vec::new(),
    }
}

impl ClipboardContent {
    /// Write into a generated `ClipboardContent` builder. `mimeType` becomes a
    /// `T.MimeType` struct; the payload goes in the `DataHandle.inline :Data`
    /// arm (this domain's payloads are one clipboard, read and written
    /// in-process, so the shared-buffer half is not taken).
    pub fn write_to(&self, builder: &mut clipboard_capnp::clipboard_content::Builder<'_>) {
        set_mime_text(&mut builder.reborrow().init_mime_type(), &self.mime_type);
        builder
            .reborrow()
            .init_data()
            .set_inline(self.data.as_slice());
    }

    /// Read from a generated `ClipboardContent` reader.
    pub fn from_reader(reader: &clipboard_capnp::clipboard_content::Reader<'_>) -> Self {
        let mime_type = reader
            .get_mime_type()
            .map(|m| mime_text(&m))
            .unwrap_or_default();
        let data = match reader.get_data() {
            Ok(handle) => data_handle_bytes(&handle),
            Err(_) => Vec::new(),
        };
        ClipboardContent { mime_type, data }
    }
}

impl ClipboardSizeLimits {
    /// Write into a generated `ClipboardSizeLimits` builder.
    pub fn write_to(&self, builder: &mut clipboard_capnp::clipboard_size_limits::Builder<'_>) {
        builder.set_max_item_size(self.max_item_size);
        builder.set_max_history_size(self.max_history_size);
        builder.set_max_history_count(self.max_history_count);
    }

    /// Read from a generated `ClipboardSizeLimits` reader.
    pub fn from_reader(reader: &clipboard_capnp::clipboard_size_limits::Reader<'_>) -> Self {
        ClipboardSizeLimits {
            max_item_size: reader.get_max_item_size(),
            max_history_size: reader.get_max_history_size(),
            max_history_count: reader.get_max_history_count(),
        }
    }
}

impl ClipboardItem {
    /// Write into a generated `ClipboardItem` builder. The contract's
    /// `ClipboardId`, `Timestamp`, `mimeTypes :List(MimeType)` and `tags` lists
    /// carry; the `ownership @5 ClipboardOwnership` sub-struct is omitted
    /// because this backend has no host-history lifetime bookkeeping to write,
    /// and an empty `ClipboardOwnership` would be a fabrication.
    pub fn write_to(&self, builder: &mut clipboard_capnp::clipboard_item::Builder<'_>) {
        builder.reborrow().init_id().set_id(self.id.get());
        builder.reborrow().init_timestamp().set_nanoseconds(self.timestamp);
        let mut mimes = builder.reborrow().init_mime_types(self.mime_types.len() as u32);
        for (i, m) in self.mime_types.iter().enumerate() {
            set_mime_text(&mut mimes.reborrow().get(i as u32), m);
        }
        let mut tags = builder.reborrow().init_tags(self.tags.len() as u32);
        for (i, t) in self.tags.iter().enumerate() {
            tags.set(i as u32, t);
        }
        builder.set_pinned(self.pinned);
        builder.set_size_bytes(self.size_bytes);
    }

    /// Read from a generated `ClipboardItem` reader.
    pub fn from_reader(reader: &clipboard_capnp::clipboard_item::Reader<'_>) -> Self {
        let mut mime_types = Vec::new();
        if let Ok(list) = reader.get_mime_types() {
            for m in list.iter() {
                mime_types.push(mime_text(&m));
            }
        }
        let mut tags = Vec::new();
        if let Ok(list) = reader.get_tags() {
            for t in list.iter() {
                let s = match t {
                    Ok(r) => r.to_str().unwrap_or("").to_string(),
                    Err(_) => String::new(),
                };
                tags.push(s);
            }
        }
        ClipboardItem {
            id: ClipboardId::new(reader.get_id().map(|i| i.get_id()).unwrap_or(0)),
            timestamp: reader.get_timestamp().map(|t| t.get_nanoseconds()).unwrap_or(0),
            mime_types,
            tags,
            pinned: reader.get_pinned(),
            source_id: ClipboardSourceId::new(1),
            size_bytes: reader.get_size_bytes(),
            text: String::new(),
        }
    }
}

impl ClipboardPreview {
    /// Write into a generated `ClipboardPreview` builder.
    pub fn write_to(&self, builder: &mut clipboard_capnp::clipboard_preview::Builder<'_>) {
        builder.set_text_preview(&self.text_preview);
        builder.set_available(self.available);
        builder.set_original_size(self.original_size);
        set_mime_text(&mut builder.reborrow().init_preview_mime_type(), &self.preview_mime_type);
    }

    /// Read from a generated `ClipboardPreview` reader.
    pub fn from_reader(reader: &clipboard_capnp::clipboard_preview::Reader<'_>) -> Self {
        ClipboardPreview {
            text_preview: text_field(reader.get_text_preview()),
            available: reader.get_available(),
            original_size: reader.get_original_size(),
            preview_mime_type: reader
                .get_preview_mime_type()
                .map(|m| mime_text(&m))
                .unwrap_or_default(),
        }
    }
}
