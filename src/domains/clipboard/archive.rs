//! The clipboard domain's **durable history** seam.
//!
//! # Why this exists
//!
//! The platform ledger is a bounded, in-memory list: it dies with the process,
//! which makes `getHistory` a session log rather than a history. A clipboard
//! product is only useful if the thing you copied yesterday is still there, so
//! the ledger needs somewhere to survive — and the charter is explicit that the
//! *backend* owns the store (law #9: the daemon holds the policy and the
//! storage; the consumer holds a capability).
//!
//! This module is that seam, and nothing more: a trait the served backend writes
//! **through**, so the domain never names a file, a database, or a schema. The
//! composition root (the daemon) decides what is behind it — today a
//! `database.kv`-backed archive on the host's own state directory, tomorrow a
//! remote database domain or a different engine build. The domain keeps one
//! rule: the *in-memory* ledger is the read path, the archive is the shadow of
//! it, and an archive failure never fails a clipboard call. Lose the archive and
//! you lose durability, not the clipboard.
//!
//! # What is archived, and what is not
//!
//! One record per item: its id, when it was copied, whether it is pinned, and
//! **every representation it was copied with** — which is what makes
//! `getPreview`, `convertContent` and "copy as" work after a restart, instead of
//! answering `notFound` about a list the caller can see. Nothing else is
//! written: no window geometry, no query text, no timing of what the user
//! looked at.

use alloc::string::String;
use alloc::vec::Vec;

use crate::core::Result;

/// One archived history item: the item's own fields plus the payloads it was
/// copied with.
///
/// The order of `offers` is the order the copy advertised it, because that order
/// is the producer's preference and `convertContent`'s exact-match arm depends
/// on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchivedItem {
    /// The item's id, stable across restarts for an archived item.
    pub id: u64,
    /// Nanoseconds since the epoch as the backend measured it (0 = unreadable
    /// clock, which the contract says is a real answer).
    pub captured_at: u64,
    /// The backend-side source id (1 = this process's own copy, 2 = observed
    /// from another client's selection owner).
    pub source_id: u64,
    /// Pinned items are exempt from the backend's eviction policy.
    pub pinned: bool,
    /// Audit tags the backend attached. Empty is a real answer.
    pub tags: Vec<String>,
    /// The representations, in the producer's order: `(mime_type, bytes)`.
    pub offers: Vec<(String, Vec<u8>)>,
}

impl ArchivedItem {
    /// The item's measured size: the sum of its representations.
    pub fn size_bytes(&self) -> u64 {
        self.offers.iter().map(|(_, data)| data.len() as u64).sum()
    }

    /// The item's MIME list, in offer order.
    pub fn mime_types(&self) -> Vec<String> {
        self.offers.iter().map(|(mime, _)| mime.clone()).collect()
    }

    /// The first text representation as UTF-8, for the ledger's snippet. `None`
    /// when no representation is text — never a lossy guess.
    pub fn first_text(&self) -> Option<&str> {
        self.offers
            .iter()
            .find(|(mime, _)| mime.starts_with("text/") || mime == "UTF8_STRING")
            .and_then(|(_, data)| core::str::from_utf8(data).ok())
    }
}

/// Where a served clipboard backend persists its history.
///
/// Every method is fallible, and every caller in the backend ignores the error
/// on the *serving* path: a store that cannot write must never turn
/// `setContent` into a failure, because the host did accept the copy. The
/// backend records the failure for the availability surface instead (see
/// `archive_last_error` in `platforms::linux::clipboard`).
///
/// The trait is `Send + Sync` because one backend is shared by every connection
/// of the daemon and by the selection watcher.
pub trait HistoryArchive: Send + Sync {
    /// Every archived item, newest first. A corrupt record is skipped, not a
    /// reason to fail the load: one bad line must not cost the user the rest of
    /// their history.
    fn load(&self) -> Result<Vec<ArchivedItem>> {
        Ok(Vec::new())
    }

    /// Insert or replace one item.
    fn put(&self, _item: &ArchivedItem) -> Result<()> {
        Ok(())
    }

    /// Remove one item by id. Removing an item that is not there is not an
    /// error (the caller's intent — the item is gone — already holds).
    fn remove(&self, _id: u64) -> Result<()> {
        Ok(())
    }

    /// Remove the whole archive.
    fn clear(&self) -> Result<()> {
        Ok(())
    }
}

/// The archive of a backend that has none: durable history is simply off, and
/// every method is an honest no-op.
///
/// This is the default, so a build that never wires a store (a unit test, an
/// embedded host, a seat-less sandbox) keeps exactly today's behaviour instead
/// of failing at run time.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoArchive;

impl HistoryArchive for NoArchive {}
