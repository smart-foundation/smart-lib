use alloc::vec::Vec;

use crate::core::Result;

use super::backend::ContentBackend;
use super::types::{ContentFilter, ContentItem, Page, Pagination};

/// The content domain's facade: the contract's methods, over one object-safe
/// [`ContentBackend`].
pub struct Content<'a, B: ContentBackend + ?Sized>(&'a B);

impl<'a, B: ContentBackend + ?Sized> Content<'a, B> {
    /// Wrap a backend.
    pub fn new(backend: &'a B) -> Self {
        Content(backend)
    }

    /// What the backend can do.
    pub fn caps(&self) -> Vec<&'static str> {
        self.0.content_caps()
    }

    /// `publish` — store an item.
    pub fn publish(&self, item: &ContentItem) -> Result<u64> {
        self.0.publish(item)
    }

    /// `publishMany` — store several items.
    pub fn publish_many(&self, items: &[ContentItem]) -> Result<Vec<u64>> {
        self.0.publish_many(items)
    }

    /// `update` — write an item, checked against its version.
    pub fn update(&self, content_id: u64, item: &ContentItem, expected_version: u64) -> Result<u64> {
        self.0.update(content_id, item, expected_version)
    }

    /// `unpublish` — take an item out of publication.
    pub fn unpublish(&self, content_id: u64) -> Result<()> {
        self.0.unpublish(content_id)
    }

    /// `delete` — remove an item.
    pub fn delete(&self, content_id: u64) -> Result<()> {
        self.0.delete(content_id)
    }

    /// `trash` — move an item to the trash.
    pub fn trash(&self, content_id: u64) -> Result<()> {
        self.0.trash(content_id)
    }

    /// `restore` — bring an item back from the trash.
    pub fn restore(&self, content_id: u64) -> Result<()> {
        self.0.restore(content_id)
    }

    /// `emptyTrash` — empty the trash.
    pub fn empty_trash(&self) -> Result<u32> {
        self.0.empty_trash()
    }

    /// `query` — the items matching a filter.
    pub fn query(&self, filter: &ContentFilter, page: Pagination) -> Result<Page<ContentItem>> {
        self.0.query(filter, page)
    }

    /// `get` — one item by its id.
    pub fn get(&self, content_id: u64) -> Result<ContentItem> {
        self.0.get(content_id)
    }
}
