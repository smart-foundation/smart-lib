//! The `content` fixture: a scripted backend over the engine's OWNED types.
//!
//! The host is never touched (charter law #8): every read is served from the
//! script, and every mutating verb is RECORDED so a test can prove the shell
//! issued the right engine call — and that a refused write reaches no engine
//! at all.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use smart_lib::core::{Error, Result};

use smart_lib::domains::content::types::{
    ContentFilter, ContentItem, Page as ContentPage, Pagination as ContentPagination,
};
use smart_lib::domains::content::ContentBackend;

/// One scripted content item.
pub fn content_item(id: u64, title: &str, version: u64) -> ContentItem {
    ContentItem {
        id,
        title: title.to_string(),
        summary: format!("the {title} summary"),
        uri: String::new(),
        mime_type: "text/plain".to_string(),
        body: format!("the {title} body").into_bytes(),
        tags: vec!["demo".to_string()],
        version,
        created_at: 1,
        modified_at: 2,
        published: true,
        trashed: false,
        publisher: 0,
    }
}

/// A scripted content store, with the engine's own optimistic concurrency.
#[derive(Default)]
pub struct MockContent {
    items: RefCell<HashMap<u64, ContentItem>>,
    next_id: RefCell<u64>,
    /// The engine calls a mutating verb made, in order.
    pub writes: RefCell<Vec<String>>,
}

impl MockContent {
    /// A store holding one published item.
    pub fn seeded() -> Self {
        let mut items = HashMap::new();
        items.insert(1, content_item(1, "First Note", 1));
        MockContent {
            items: RefCell::new(items),
            next_id: RefCell::new(2),
            writes: RefCell::new(Vec::new()),
        }
    }

    /// An empty store.
    pub fn empty() -> Self {
        MockContent {
            items: RefCell::new(HashMap::new()),
            next_id: RefCell::new(1),
            writes: RefCell::new(Vec::new()),
        }
    }
}

impl ContentBackend for MockContent {
    fn publish(&self, item: &ContentItem) -> Result<u64> {
        self.writes.borrow_mut().push("publish".to_string());
        let mut next = self.next_id.borrow_mut();
        let id = *next;
        *next += 1;
        let mut published = item.clone();
        published.id = id;
        published.version = 1;
        self.items.borrow_mut().insert(id, published);
        Ok(id)
    }

    fn publish_many(&self, items: &[ContentItem]) -> Result<Vec<u64>> {
        self.writes.borrow_mut().push("publish_many".to_string());
        let mut ids = Vec::with_capacity(items.len());
        for item in items {
            let mut next = self.next_id.borrow_mut();
            let id = *next;
            *next += 1;
            let mut published = item.clone();
            published.id = id;
            published.version = 1;
            self.items.borrow_mut().insert(id, published);
            ids.push(id);
        }
        Ok(ids)
    }

    fn update(&self, content_id: u64, item: &ContentItem, expected_version: u64) -> Result<u64> {
        self.writes
            .borrow_mut()
            .push(format!("update {content_id}"));
        let mut items = self.items.borrow_mut();
        let current = items.get(&content_id).cloned().ok_or_else(Error::not_found)?;
        if current.version != expected_version {
            return Err(Error::with_message(
                smart_lib::core::ErrorCode::Conflict,
                format!(
                    "content {content_id} is at version {} but the write presented {expected_version}",
                    current.version
                ),
            ));
        }
        let mut updated = item.clone();
        updated.id = content_id;
        updated.version = current.version + 1;
        updated.created_at = current.created_at;
        let version = updated.version;
        items.insert(content_id, updated);
        Ok(version)
    }

    fn unpublish(&self, content_id: u64) -> Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("unpublish {content_id}"));
        let mut items = self.items.borrow_mut();
        let mut item = items.get(&content_id).cloned().ok_or_else(Error::not_found)?;
        item.published = false;
        item.version += 1;
        items.insert(content_id, item);
        Ok(())
    }

    fn delete(&self, content_id: u64) -> Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("delete {content_id}"));
        self.items
            .borrow_mut()
            .remove(&content_id)
            .map(|_| ())
            .ok_or_else(Error::not_found)
    }

    fn trash(&self, content_id: u64) -> Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("trash {content_id}"));
        let mut items = self.items.borrow_mut();
        let mut item = items.get(&content_id).cloned().ok_or_else(Error::not_found)?;
        item.trashed = true;
        item.version += 1;
        items.insert(content_id, item);
        Ok(())
    }

    fn restore(&self, content_id: u64) -> Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("restore {content_id}"));
        let mut items = self.items.borrow_mut();
        let mut item = items.get(&content_id).cloned().ok_or_else(Error::not_found)?;
        item.trashed = false;
        item.version += 1;
        items.insert(content_id, item);
        Ok(())
    }

    fn empty_trash(&self) -> Result<u32> {
        self.writes.borrow_mut().push("empty_trash".to_string());
        let mut items = self.items.borrow_mut();
        let trashed: Vec<u64> = items
            .values()
            .filter(|item| item.trashed)
            .map(|item| item.id)
            .collect();
        for id in &trashed {
            items.remove(id);
        }
        Ok(trashed.len() as u32)
    }

    fn query(
        &self,
        filter: &ContentFilter,
        page: ContentPagination,
    ) -> Result<ContentPage<ContentItem>> {
        let items = self.items.borrow();
        let rows: Vec<ContentItem> = items
            .values()
            .filter(|item| {
                if item.trashed && !filter.include_trashed {
                    return false;
                }
                if !filter.query.is_empty() {
                    let needle = filter.query.to_lowercase();
                    let hit = item.title.to_lowercase().contains(&needle)
                        || item.summary.to_lowercase().contains(&needle)
                        || String::from_utf8_lossy(&item.body)
                            .to_lowercase()
                            .contains(&needle);
                    if !hit {
                        return false;
                    }
                }
                if !filter.mime_type_prefix.is_empty()
                    && !item.mime_type.starts_with(&filter.mime_type_prefix)
                {
                    return false;
                }
                if !filter.tags.is_empty()
                    && !filter
                        .tags
                        .iter()
                        .all(|tag| item.tags.iter().any(|owned| owned == tag))
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        let total = rows.len() as u64;
        let items = rows
            .into_iter()
            .skip(page.offset as usize)
            .take(page.effective_limit() as usize)
            .collect();
        Ok(ContentPage { items, total })
    }

    fn get(&self, content_id: u64) -> Result<ContentItem> {
        self.items
            .borrow()
            .get(&content_id)
            .cloned()
            .ok_or_else(Error::not_found)
    }
}
