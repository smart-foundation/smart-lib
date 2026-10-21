//! The Linux **content** cell: the engine's own content store.
//!
//! Native mechanism: an in-process store — the contract defines content as
//! the engine's own corpus, and this host publishes no content corpus of its
//! own, so the store *is* the mechanism. There is no shellout (law #9) and
//! no host-mutating act (law #8): a published item lives in this process's
//! memory, and the store says so where it matters.
//!
//! What is real: the store's own verbs — publish, publishMany, update
//! (with the contract's optimistic version check), unpublish, delete, trash,
//! restore, emptyTrash, query and get — over real items with real ids,
//! versions, timestamps and payloads.
//!
//! What is not served, and why: everything the store does not model —
//! relationships, reactions, provenance, encryption, branches, composites,
//! lineage, translations, the rate limiter, previews and the change feed.
//! Each refuses with the reason, and none of them pretends.

extern crate std;

use alloc::vec::Vec;

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::core::{Error, Result};
use crate::domains::content::backend::ContentBackend;
use crate::domains::content::types::{ContentFilter, ContentItem, Page, Pagination};

/// The engine's own content store.
pub struct LinuxContentBackend {
    store: Mutex<Store>,
}

/// The store: the items by id, the next id, and the clock.
#[derive(Default)]
struct Store {
    items: BTreeMap<u64, ContentItem>,
    next_id: u64,
    clock: u64,
}

impl Store {
    fn new() -> Self {
        Store {
            items: BTreeMap::new(),
            next_id: 1,
            clock: 0,
        }
    }

    /// The next monotonic stamp, in nanoseconds.
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

impl LinuxContentBackend {
    /// A backend with an empty store.
    pub fn new() -> Self {
        LinuxContentBackend {
            store: Mutex::new(Store::new()),
        }
    }

    /// The items the store holds, in id order.
    pub fn items(&self) -> Result<Vec<ContentItem>> {
        let store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        Ok(store.items.values().cloned().collect())
    }
}

impl Default for LinuxContentBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether an item matches a filter.
fn matches(item: &ContentItem, filter: &ContentFilter) -> bool {
    if item.trashed && !filter.include_trashed {
        return false;
    }
    if !filter.mime_type_prefix.is_empty()
        && !item.mime_type.starts_with(&filter.mime_type_prefix)
    {
        return false;
    }
    if !filter.tags.is_empty() {
        let wanted = &filter.tags;
        if !item
            .tags
            .iter()
            .any(|tag| wanted.iter().any(|w| w.eq_ignore_ascii_case(tag)))
        {
            return false;
        }
    }
    if !filter.query.is_empty() {
        let needle = filter.query.to_lowercase();
        let payload = core::str::from_utf8(&item.body)
            .unwrap_or("")
            .to_lowercase();
        let hit = item.title.to_lowercase().contains(&needle)
            || item.summary.to_lowercase().contains(&needle)
            || payload.contains(&needle);
        if !hit {
            return false;
        }
    }
    true
}

/// Take a page of rows.
fn paginate<T: Clone>(rows: Vec<T>, page: Pagination) -> Page<T> {
    let total = rows.len() as u64;
    let limit = page.effective_limit() as usize;
    let offset = page.offset as usize;
    let items = rows.into_iter().skip(offset).take(limit).collect();
    Page { items, total }
}

impl ContentBackend for LinuxContentBackend {
    fn publish(&self, item: &ContentItem) -> Result<u64> {
        let mut store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        let id = store.next_id;
        store.next_id += 1;
        let now = store.tick();
        let mut published = item.clone();
        published.id = id;
        published.created_at = now;
        published.modified_at = now;
        published.version = 1;
        published.published = true;
        published.trashed = false;
        store.items.insert(id, published);
        Ok(id)
    }

    fn publish_many(&self, items: &[ContentItem]) -> Result<Vec<u64>> {
        let mut ids = Vec::with_capacity(items.len());
        for item in items {
            ids.push(self.publish(item)?);
        }
        Ok(ids)
    }

    fn update(&self, content_id: u64, item: &ContentItem, expected_version: u64) -> Result<u64> {
        let mut store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        let current = store
            .items
            .get(&content_id)
            .cloned()
            .ok_or_else(|| Error::not_found())?;
        if current.version != expected_version {
            // The contract's optimistic-concurrency check: a stale write is a
            // conflict, never a silent overwrite.
            return Err(Error::with_message(
                crate::core::ErrorCode::Conflict,
                alloc::format!(
                    "content {content_id} is at version {} but the write presented {expected_version}",
                    current.version
                ),
            ));
        }
        let now = store.tick();
        let mut updated = item.clone();
        updated.id = content_id;
        updated.created_at = current.created_at;
        updated.modified_at = now;
        updated.version = current.version + 1;
        updated.published = current.published;
        updated.trashed = current.trashed;
        let version = updated.version;
        store.items.insert(content_id, updated);
        Ok(version)
    }

    fn unpublish(&self, content_id: u64) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        let mut item = store
            .items
            .get(&content_id)
            .cloned()
            .ok_or_else(|| Error::not_found())?;
        item.published = false;
        item.modified_at = store.tick();
        item.version += 1;
        store.items.insert(content_id, item);
        Ok(())
    }

    fn delete(&self, content_id: u64) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        store
            .items
            .remove(&content_id)
            .map(|_| ())
            .ok_or_else(|| Error::not_found())
    }

    fn trash(&self, content_id: u64) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        let mut item = store
            .items
            .get(&content_id)
            .cloned()
            .ok_or_else(|| Error::not_found())?;
        item.trashed = true;
        item.modified_at = store.tick();
        item.version += 1;
        store.items.insert(content_id, item);
        Ok(())
    }

    fn restore(&self, content_id: u64) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        let mut item = store
            .items
            .get(&content_id)
            .cloned()
            .ok_or_else(|| Error::not_found())?;
        item.trashed = false;
        item.modified_at = store.tick();
        item.version += 1;
        store.items.insert(content_id, item);
        Ok(())
    }

    fn empty_trash(&self) -> Result<u32> {
        let mut store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        let trashed: Vec<u64> = store
            .items
            .values()
            .filter(|item| item.trashed)
            .map(|item| item.id)
            .collect();
        for id in &trashed {
            store.items.remove(id);
        }
        Ok(trashed.len() as u32)
    }

    fn query(&self, filter: &ContentFilter, page: Pagination) -> Result<Page<ContentItem>> {
        let store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        let rows: Vec<ContentItem> = store
            .items
            .values()
            .filter(|item| matches(item, filter))
            .cloned()
            .collect();
        Ok(paginate(rows, page))
    }

    fn get(&self, content_id: u64) -> Result<ContentItem> {
        let store = self.store.lock().map_err(|_| Error::failed("content store lock failed"))?;
        store
            .items
            .get(&content_id)
            .cloned()
            .ok_or_else(|| Error::not_found())
    }

    fn content_caps(&self) -> Vec<&'static str> {
        alloc::vec!["content.inproc", "content.store"]
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;

    fn item(title: &str) -> ContentItem {
        ContentItem {
            title: title.to_string(),
            summary: format!("the summary of {title}"),
            mime_type: "text/plain".to_string(),
            body: alloc::format!("the body of {title}").into_bytes(),
            tags: vec!["note".to_string()],
            ..ContentItem::default()
        }
    }

    fn page() -> Pagination {
        Pagination {
            offset: 0,
            limit: 10,
        }
    }

    /// `publish` stores the item and hands back its own id: the store holds
    /// what was published, under the id it minted.
    #[test]
    fn a_published_item_is_really_stored() {
        let backend = LinuxContentBackend::new();
        let id = backend.publish(&item("first")).expect("publish");
        // The item is in the store, under the id the publish handed back.
        let stored = backend.items().expect("items");
        assert_eq!(stored.len(), 1, "the published item is in the store");
        assert_eq!(stored[0].id, id);
        assert!(stored[0].published, "a published item says so");
        // A second item gets its own id.
        let second = backend.publish(&item("second")).expect("publish");
        assert_eq!(second, 2);
        assert_eq!(backend.items().expect("items").len(), 2);
    }

    /// `get` reads the stored item back: the fields it was published with,
    /// byte for byte.
    #[test]
    fn get_reads_the_stored_item_back() {
        let backend = LinuxContentBackend::new();
        let id = backend.publish(&item("first")).expect("publish");
        let got = backend.get(id).expect("get");
        assert_eq!(got.title, "first");
        assert_eq!(got.mime_type, "text/plain");
        assert_eq!(got.body, b"the body of first".to_vec());
        assert_eq!(got.tags, vec!["note".to_string()]);
        assert_eq!(got.version, 1);
        assert!(got.created_at > 0);
        assert!(got.modified_at >= got.created_at);
        assert!(got.published);
    }

    /// `publishMany` answers with EVERY id it minted — one per item, in the
    /// order the items were given.
    #[test]
    fn a_batch_publish_answers_with_every_id() {
        let backend = LinuxContentBackend::new();
        let ids = backend
            .publish_many(&[item("first"), item("second"), item("third")])
            .expect("publish many");
        assert_eq!(ids.len(), 3, "one id per item");
        assert_eq!(ids, alloc::vec![1, 2, 3], "the ids are the engine's own");
        // And every id names an item that is really in the store.
        for id in &ids {
            assert!(backend.get(*id).is_ok(), "id {id} names a stored item");
        }
        // An empty batch is the honest empty, not an error.
        assert!(backend.publish_many(&[]).expect("publish many").is_empty());
    }

    /// `unpublish` takes the item out of the published set: the item is
    /// still in the store, but it no longer says it is published, and the
    /// write moved the item's own version.
    #[test]
    fn an_unpublished_item_is_no_longer_published() {
        let backend = LinuxContentBackend::new();
        let id = backend.publish(&item("first")).expect("publish");
        let before = backend.get(id).expect("the item is in the store");
        assert!(before.published);
        backend.unpublish(id).expect("unpublish");
        let got = backend.get(id).expect("the item is still in the store");
        assert!(!got.published, "the item no longer says it is published");
        assert!(
            got.version > before.version,
            "the write moved the item's own version"
        );
    }

    /// `delete` takes the item out of the store: the id names nothing
    /// afterwards, and the store says so.
    #[test]
    fn a_deleted_item_is_gone() {
        let backend = LinuxContentBackend::new();
        let id = backend.publish(&item("first")).expect("publish");
        backend.delete(id).expect("delete");
        let error = backend.get(id).expect_err("the item is gone");
        assert_eq!(error.code, crate::core::ErrorCode::NotFound);
        assert!(backend.items().expect("items").is_empty(), "the store is empty");
    }

    #[test]
    fn an_update_is_checked_against_the_version() {
        let backend = LinuxContentBackend::new();
        let id = backend.publish(&item("first")).expect("publish");
        let mut changed = item("first, edited");
        changed.body = b"edited".to_vec();
        let version = backend.update(id, &changed, 1).expect("update");
        assert_eq!(version, 2);
        assert_eq!(backend.get(id).expect("get").title, "first, edited");
        // A stale write is a conflict, never a silent overwrite.
        let error = backend
            .update(id, &changed, 1)
            .expect_err("a stale version conflicts");
        assert_eq!(error.code, crate::core::ErrorCode::Conflict);
        assert!(error.message.contains("version"));
    }

    #[test]
    fn a_query_answers_from_the_store() {
        let backend = LinuxContentBackend::new();
        backend.publish(&item("alpha")).expect("publish");
        backend.publish(&item("beta")).expect("publish");
        let hits = backend
            .query(&ContentFilter::default(), page())
            .expect("query");
        assert_eq!(hits.total, 2);

        let hits = backend
            .query(
                &ContentFilter {
                    query: "alpha".to_string(),
                    ..ContentFilter::default()
                },
                page(),
            )
            .expect("query");
        assert_eq!(hits.total, 1);
        assert_eq!(hits.items[0].title, "alpha");

        // A query that matches nothing answers nothing, without an error.
        let hits = backend
            .query(
                &ContentFilter {
                    query: "nothing matches this".to_string(),
                    ..ContentFilter::default()
                },
                page(),
            )
            .expect("query");
        assert_eq!(hits.total, 0);
    }

    /// `trash` moves the item into the trash: it is hidden from the default
    /// query and the trash query shows it, marked as trashed.
    #[test]
    fn trash_moves_the_item_into_the_trash() {
        let backend = LinuxContentBackend::new();
        let id = backend.publish(&item("first")).expect("publish");
        backend.trash(id).expect("trash");
        let hits = backend
            .query(&ContentFilter::default(), page())
            .expect("query");
        assert_eq!(hits.total, 0, "a trashed item is hidden by default");
        let hits = backend
            .query(
                &ContentFilter {
                    include_trashed: true,
                    ..ContentFilter::default()
                },
                page(),
            )
            .expect("query");
        assert_eq!(hits.total, 1);
        assert!(hits.items[0].trashed, "the item says it is trashed");
    }

    /// `restore` takes the item out of the trash: it is published again.
    #[test]
    fn restore_takes_the_item_out_of_the_trash() {
        let backend = LinuxContentBackend::new();
        let id = backend.publish(&item("first")).expect("publish");
        backend.trash(id).expect("trash");
        backend.restore(id).expect("restore");
        let hits = backend
            .query(&ContentFilter::default(), page())
            .expect("query");
        assert_eq!(hits.total, 1, "the restored item is published again");
        assert!(!hits.items[0].trashed);
    }

    /// `emptyTrash` empties the trash: it says how many it removed, and the
    /// removed items are gone from the store.
    #[test]
    fn empty_trash_empties_the_trash_and_says_how_many() {
        let backend = LinuxContentBackend::new();
        let first = backend.publish(&item("first")).expect("publish");
        let second = backend.publish(&item("second")).expect("publish");
        backend.trash(first).expect("trash");
        backend.trash(second).expect("trash");
        assert_eq!(backend.empty_trash().expect("empty"), 2, "two were trashed");
        assert!(backend.get(first).is_err(), "the trashed item is gone");
        assert!(backend.get(second).is_err(), "the trashed item is gone");
        // Emptying an empty trash is the honest zero, not an error.
        assert_eq!(backend.empty_trash().expect("empty"), 0);
    }

    #[test]
    fn a_missing_id_is_an_honest_absence() {
        let backend = LinuxContentBackend::new();
        let error = backend.get(42).expect_err("no such content");
        assert_eq!(error.code, crate::core::ErrorCode::NotFound);
        let error = backend.delete(42).expect_err("no such content");
        assert_eq!(error.code, crate::core::ErrorCode::NotFound);
    }
}
