//! The clipboard history's durable home: [`HistoryArchive`] over `database.kv`.
//!
//! # Why the archive lives here and not in the domain
//!
//! The domain owns the *seam* (`domains::clipboard::archive::HistoryArchive`);
//! the composition root owns the *choice* of store. This module is that choice,
//! written once, so the decision is visible in exactly one place: the clipboard's
//! history is kept in the **database domain**, under its own namespace, through
//! the same `DatabaseBackend` a `database.kv` client talks to. There is no
//! second store, no bespoke file format for the product, and no path by which the
//! clipboard domain learns what a database is.
//!
//! # Layout
//!
//! ```text
//!   namespace  clipboard.history
//!   keys       <20-digit id>       one item per key, sorted by id
//!              next_id             the id counter, so ids never repeat
//! ```
//!
//! Key order *is* time order, which is what makes `getHistory` a range read and
//! what makes an archive list identical to the ledger list after a reopen.
//!
//! # Encoding
//!
//! ```text
//!   magic   "SCA1"                          one item per value
//!   id       u64
//!   captured u64
//!   source   u64
//!   pinned   u8
//!   tags     u32 count, then per tag  u32 len + bytes
//!   offers   u32 count, then per offer u32 len + mime, u64 len + bytes
//! ```
//!
//! Little-endian, explicitly sized, no serde: a value here can be a 4 MiB
//! screenshot, so the format copies bytes rather than describing them as text.
//! An unreadable value is skipped by `load` — one corrupt item must not cost the
//! user their history.

extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use std::sync::Arc;

use crate::core::{Error, Result};
use crate::domains::clipboard::archive::{ArchivedItem, HistoryArchive};
use crate::domains::database::backend::DatabaseBackend;
use crate::domains::database::types::Pagination;

/// The `database.kv` namespace the clipboard's history lives in.
pub const HISTORY_NAMESPACE: &str = "clipboard.history";

/// The key holding the id counter.
const NEXT_ID_KEY: &str = "next_id";

/// The value magic: a reader that finds anything else skips the record.
const MAGIC: &[u8; 4] = b"SCA1";

/// The database-backed clipboard archive.
///
/// Cloneable and cheap: it is an `Arc` around the database backend plus the
/// namespace, so the daemon can hand the same archive to the served backend and
/// to the selection watcher.
#[derive(Clone)]
pub struct DatabaseArchive<D> {
    database: Arc<D>,
    namespace: String,
}

impl<D> DatabaseArchive<D>
where
    D: DatabaseBackend + Send + Sync + 'static,
{
    /// An archive over the `clipboard.history` namespace.
    pub fn new(database: Arc<D>) -> Self {
        Self { database, namespace: HISTORY_NAMESPACE.to_string() }
    }

    /// An archive over an explicit namespace (a test that must not collide with
    /// the real one, or a second clipboard-like surface later).
    pub fn with_namespace(database: Arc<D>, namespace: &str) -> Self {
        Self { database, namespace: namespace.to_string() }
    }

    /// The key one item is stored under: zero-padded so lexical order is id
    /// order for any id width.
    fn key(id: u64) -> String {
        format!("{id:020}")
    }

    fn page_all() -> Pagination {
        // `limit == 0` means ONE to the contract, so "everything" is spelled
        // out rather than left to a default.
        Pagination { offset: 0, limit: u32::MAX }
    }
}

impl<D> HistoryArchive for DatabaseArchive<D>
where
    // `Send + Sync` because the trait promises it: one archive is handed to the
    // served backend *and* to the selection watcher.
    D: DatabaseBackend + Send + Sync + 'static,
{
    fn load(&self) -> Result<Vec<ArchivedItem>> {
        let (keys, _total) = self.database.list_keys(&self.namespace, "", Self::page_all())?;
        let mut items = Vec::new();
        for key in keys {
            // The counter is not an item.
            if key == NEXT_ID_KEY {
                continue;
            }
            let Ok(bytes) = self.database.get(&self.namespace, &key) else {
                continue;
            };
            if let Some(item) = decode(&bytes) {
                items.push(item);
            }
        }
        // Newest first: the ledger's order, and the product's list order.
        items.sort_by(|left, right| right.id.cmp(&left.id));
        Ok(items)
    }

    fn put(&self, item: &ArchivedItem) -> Result<()> {
        let mut bytes = Vec::new();
        encode(item, &mut bytes);
        self.database.set(&self.namespace, &Self::key(item.id), &bytes)?;
        // Keep the counter ahead of every stored id, so a restart never reissues
        // an id that an item (or a caller holding it) still names.
        let next = self
            .database
            .get(&self.namespace, NEXT_ID_KEY)
            .ok()
            .and_then(|bytes| read_u64(&bytes, 0))
            .unwrap_or(0);
        if next <= item.id {
            let mut counter = Vec::new();
            counter.extend_from_slice(&(item.id + 1).to_le_bytes());
            self.database.set(&self.namespace, NEXT_ID_KEY, &counter)?;
        }
        Ok(())
    }

    fn remove(&self, id: u64) -> Result<()> {
        // A key that is not there is already the caller's intent.
        let _ = self.database.delete(&self.namespace, &Self::key(id));
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        self.database.clear(&self.namespace)
    }
}

/// Encode one item.
fn encode(item: &ArchivedItem, out: &mut Vec<u8>) {
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&item.id.to_le_bytes());
    out.extend_from_slice(&item.captured_at.to_le_bytes());
    out.extend_from_slice(&item.source_id.to_le_bytes());
    out.push(u8::from(item.pinned));
    out.extend_from_slice(&(item.tags.len() as u32).to_le_bytes());
    for tag in &item.tags {
        push_blob(out, tag.as_bytes());
    }
    out.extend_from_slice(&(item.offers.len() as u32).to_le_bytes());
    for (mime, data) in &item.offers {
        push_blob(out, mime.as_bytes());
        push_blob(out, data);
    }
}

fn push_blob(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Decode one item, or `None` when the bytes are not ours or are truncated.
fn decode(bytes: &[u8]) -> Option<ArchivedItem> {
    let mut cursor = Cursor { bytes, at: 0 };
    if cursor.take(4)? != MAGIC {
        return None;
    }
    let id = cursor.u64()?;
    let captured_at = cursor.u64()?;
    let source_id = cursor.u64()?;
    let pinned = cursor.u8()? != 0;

    let tag_count = cursor.u32()? as usize;
    // Bound the count by what the buffer can possibly hold, so a corrupt count
    // cannot make us allocate a million empty strings.
    if tag_count > bytes.len() / 4 {
        return None;
    }
    let mut tags = Vec::with_capacity(tag_count);
    for _ in 0..tag_count {
        tags.push(String::from_utf8(cursor.blob()?).ok()?);
    }

    let offer_count = cursor.u32()? as usize;
    if offer_count > bytes.len() / 4 {
        return None;
    }
    let mut offers = Vec::with_capacity(offer_count);
    for _ in 0..offer_count {
        let mime = String::from_utf8(cursor.blob()?).ok()?;
        let data = cursor.blob()?;
        offers.push((mime, data));
    }
    Some(ArchivedItem { id, captured_at, source_id, pinned, tags, offers })
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let end = at.checked_add(8)?;
    let slice = bytes.get(at..end)?;
    let mut word = [0u8; 8];
    word.copy_from_slice(slice);
    Some(u64::from_le_bytes(word))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(len)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        let slice = self.take(4)?;
        let mut word = [0u8; 4];
        word.copy_from_slice(slice);
        Some(u32::from_le_bytes(word))
    }

    fn u64(&mut self) -> Option<u64> {
        let slice = self.take(8)?;
        let mut word = [0u8; 8];
        word.copy_from_slice(slice);
        Some(u64::from_le_bytes(word))
    }

    fn blob(&mut self) -> Option<Vec<u8>> {
        let len = self.u32()? as usize;
        Some(self.take(len)?.to_vec())
    }
}

/// The "no history" reason a caller sees when the archive is not wired. Kept
/// here so the daemon and the tests describe the same absence the same way.
pub fn archive_unavailable_reason() -> String {
    "no-backend: this service was started without a history store, so its copy ledger lives \
     only in memory and does not survive a restart"
        .to_string()
}

/// A defensive constructor used by the daemon: build an archive over a database
/// backend, or say why not.
pub fn open_archive<D>(database: Arc<D>) -> Result<DatabaseArchive<D>>
where
    D: DatabaseBackend + Send + Sync + 'static,
{
    // Reading the namespace is the only way to find out the store is unusable,
    // so probe it now rather than at the first copy.
    let archive = DatabaseArchive::new(database);
    archive
        .database
        .count(HISTORY_NAMESPACE)
        .map_err(|error| Error::failed(&format!("the history namespace is unreadable: {error}")))?;
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::linux::database::LinuxDatabaseBackend;

    fn item(id: u64, mime: &str, data: &[u8]) -> ArchivedItem {
        ArchivedItem {
            id,
            captured_at: 1_700_000_000_000_000_000 + id,
            source_id: 1,
            pinned: id % 2 == 0,
            tags: vec!["tag".to_string()],
            offers: vec![(mime.to_string(), data.to_vec())],
        }
    }

    /// An item survives the round trip field for field, including binary
    /// payloads and an empty offer list.
    #[test]
    fn an_item_round_trips_through_the_codec() {
        let original = ArchivedItem {
            id: 41,
            captured_at: 12,
            source_id: 2,
            pinned: true,
            tags: vec!["a".to_string(), "b".to_string()],
            offers: vec![
                ("text/plain".to_string(), b"hello".to_vec()),
                ("image/png".to_string(), vec![0x89, 0x50, 0x00, 0xff]),
            ],
        };
        let mut bytes = Vec::new();
        encode(&original, &mut bytes);
        assert_eq!(decode(&bytes), Some(original));

        let empty = ArchivedItem {
            id: 1,
            captured_at: 0,
            source_id: 1,
            pinned: false,
            tags: Vec::new(),
            offers: Vec::new(),
        };
        let mut bytes = Vec::new();
        encode(&empty, &mut bytes);
        assert_eq!(decode(&bytes), Some(empty.clone()));
        assert_eq!(empty.size_bytes(), 0);
        assert_eq!(empty.first_text(), None);
    }

    /// A truncated or foreign value is skipped, not a panic.
    #[test]
    fn a_corrupt_value_is_skipped() {
        let mut bytes = Vec::new();
        encode(&item(1, "text/plain", b"x"), &mut bytes);
        for cut in [0, 3, 4, 12, bytes.len() - 1] {
            assert_eq!(decode(&bytes[..cut]), None, "cut at {cut}");
        }
        assert_eq!(decode(b"not ours at all"), None);
        // A nonsense tag count is refused rather than allocating for it.
        let mut hostile = Vec::new();
        hostile.extend_from_slice(MAGIC);
        hostile.extend_from_slice(&1u64.to_le_bytes());
        hostile.extend_from_slice(&0u64.to_le_bytes());
        hostile.extend_from_slice(&1u64.to_le_bytes());
        hostile.push(0);
        hostile.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&hostile), None);
    }

    /// The archive's list is the ledger's list: newest first, ids intact.
    #[test]
    fn the_archive_lists_newest_first() {
        let database = Arc::new(LinuxDatabaseBackend::new());
        let archive = DatabaseArchive::with_namespace(database, "clipboard.history.test");
        archive.put(&item(3, "text/plain", b"three")).expect("put");
        archive.put(&item(1, "text/plain", b"one")).expect("put");
        archive.put(&item(2, "image/png", b"two")).expect("put");
        let items = archive.load().expect("load");
        assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), vec![3, 2, 1]);
        assert_eq!(items[1].first_text(), Some("two"));
    }

    /// Remove and clear are the archive's own answers, and removing an absent
    /// item is not an error.
    #[test]
    fn remove_and_clear_are_idempotent() {
        let database = Arc::new(LinuxDatabaseBackend::new());
        let archive = DatabaseArchive::with_namespace(database, "clipboard.history.test");
        archive.put(&item(1, "text/plain", b"one")).expect("put");
        archive.remove(1).expect("remove");
        assert!(archive.load().expect("load").is_empty());
        archive.remove(99).expect("removing an absent item is fine");
        archive.put(&item(2, "text/plain", b"two")).expect("put");
        archive.clear().expect("clear");
        assert!(archive.load().expect("load").is_empty());
    }

    /// Ids never repeat across a reopen: the counter is stored beside the items.
    #[test]
    fn the_id_counter_survives_a_reopen() {
        let database = Arc::new(LinuxDatabaseBackend::new());
        let archive = DatabaseArchive::with_namespace(Arc::clone(&database), "clipboard.history.test");
        archive.put(&item(7, "text/plain", b"seven")).expect("put");
        let stored = database.get("clipboard.history.test", NEXT_ID_KEY).expect("counter");
        assert_eq!(read_u64(&stored, 0), Some(8));
    }
}
