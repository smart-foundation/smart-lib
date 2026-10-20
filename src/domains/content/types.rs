use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::schema::{content_capnp, types_capnp};

/// One piece of content — the contract's `ContentItem`.
///
/// The store this domain keeps is the engine's own memory (the contract
/// defines content as the engine's own corpus, and this host publishes no
/// content corpus of its own), so every field here is a field the store
/// really holds rather than a placeholder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentItem {
    /// The content's identifier, assigned by the store.
    pub id: u64,
    /// The title.
    pub title: String,
    /// The summary.
    pub summary: String,
    /// The URI, when the item is a reference rather than a payload.
    pub uri: String,
    /// The MIME type (`"text/plain"`).
    pub mime_type: String,
    /// The inline payload.
    pub body: Vec<u8>,
    /// The tags.
    pub tags: Vec<String>,
    /// The publishing app.
    pub publisher: u64,
    /// The creation stamp, in nanoseconds since the epoch.
    pub created_at: u64,
    /// The last modification stamp.
    pub modified_at: u64,
    /// The optimistic-concurrency version: it moves on every write, and a
    /// write that presents a stale version is refused with a conflict.
    pub version: u64,
    /// Whether the item is in the trash.
    pub trashed: bool,
    /// Whether the item is published.
    pub published: bool,
}

impl ContentItem {
    /// The payload's size in bytes.
    pub fn size(&self) -> u64 {
        self.body.len() as u64
    }

    /// Write into a generated `ContentItem` builder.
    pub fn write_to(&self, builder: &mut content_capnp::content_item::Builder<'_>) {
        builder.reborrow().init_id().set_id(self.id);
        builder.set_title(&self.title);
        builder.set_summary(&self.summary);
        builder.set_uri(&self.uri);
        builder.set_size(self.size());
        builder.reborrow().init_created_at().set_nanoseconds(self.created_at);
        builder
            .reborrow()
            .init_modified_at()
            .set_nanoseconds(self.modified_at);
        builder.reborrow().init_publisher().set_id(self.publisher);
        builder.set_trashed(self.trashed);
        builder.set_version(self.version);
        write_mime_type(&mut builder.reborrow().init_mime_type(), &self.mime_type);
        if !self.body.is_empty() {
            builder
                .reborrow()
                .init_mime_data()
                .set_inline(self.body.as_slice());
        }
        let mut tags = builder.reborrow().init_tags(self.tags.len() as u32);
        for (index, tag) in self.tags.iter().enumerate() {
            // The contract's `ContentTag` carries the tag's text, a
            // confidence and a source. A tag the caller set has a confidence
            // of 1 and no source of its own — the store does not invent a
            // provenance it was never given.
            let mut entry = tags.reborrow().get(index as u32);
            entry.set_text(tag);
            entry.set_confidence(1.0);
        }
    }

    /// Read from a generated `ContentItem` reader.
    pub fn from_reader(reader: &content_capnp::content_item::Reader<'_>) -> Self {
        let mut body = Vec::new();
        if let Ok(handle) = reader.get_mime_data() {
            // The `DataHandle` union: only the `inline :Data` arm is
            // materialised — a `buffer :BufferId` names shared memory this
            // in-process store never allocated.
            use types_capnp::data_handle::WhichReader;
            if let Ok(WhichReader::Inline(Ok(data))) = handle.which() {
                body = data.to_vec();
            }
        }
        let mut tags = Vec::new();
        if let Ok(list) = reader.get_tags() {
            for tag in list.iter() {
                let text = crate::domains::content::types::text_of(tag.get_text());
                if !text.is_empty() {
                    tags.push(text);
                }
            }
        }
        ContentItem {
            id: reader
                .get_id()
                .map(|id| id.get_id())
                .unwrap_or_default(),
            title: crate::domains::content::types::text_of(reader.get_title()),
            summary: crate::domains::content::types::text_of(reader.get_summary()),
            uri: crate::domains::content::types::text_of(reader.get_uri()),
            mime_type: reader
                .get_mime_type()
                .map(|mime| read_mime_type(&mime))
                .unwrap_or_default(),
            body,
            tags,
            publisher: reader
                .get_publisher()
                .map(|publisher| publisher.get_id())
                .unwrap_or_default(),
            created_at: reader
                .get_created_at()
                .map(|stamp| stamp.get_nanoseconds())
                .unwrap_or_default(),
            modified_at: reader
                .get_modified_at()
                .map(|stamp| stamp.get_nanoseconds())
                .unwrap_or_default(),
            version: reader.get_version(),
            trashed: reader.get_trashed(),
            published: true,
        }
    }
}

/// The query filter — the contract's `ContentFilter`, narrowed to the fields
/// this store can answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentFilter {
    /// A substring match over the title, the summary and the payload.
    pub query: String,
    /// The MIME type prefix the item must carry.
    pub mime_type_prefix: String,
    /// The tags the item must carry (any of them).
    pub tags: Vec<String>,
    /// Whether trashed items are included.
    pub include_trashed: bool,
}

/// The pagination window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    /// The first row to return.
    pub offset: u32,
    /// How many rows to return (0 means one — never "everything").
    pub limit: u32,
}

impl Pagination {
    /// The effective limit: a zero limit means one row, never an unbounded
    /// read.
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A page of rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    /// How many rows the store holds in total.
    pub total: u64,
}

/// A subscription handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// Read a capnp text field into an owned string.
pub(crate) fn text_of(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// Spell a `T.MimeType` as `"type/subtype"`.
fn write_mime_type(builder: &mut types_capnp::mime_type::Builder<'_>, mime: &str) {
    let (kind, subtype) = match mime.split_once('/') {
        Some((kind, subtype)) => (kind, subtype),
        None => (mime, ""),
    };
    builder.set_type(kind);
    builder.set_subtype(subtype);
}

/// Read a `T.MimeType` back as `"type/subtype"`.
fn read_mime_type(reader: &types_capnp::mime_type::Reader<'_>) -> String {
    let kind = text_of(reader.get_type());
    let subtype = text_of(reader.get_subtype());
    if subtype.is_empty() {
        kind
    } else {
        alloc::format!("{kind}/{subtype}")
    }
}
