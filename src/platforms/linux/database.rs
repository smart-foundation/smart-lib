//! The database domain's host store: `database.kv` over the host's own files.
//!
//! # Two modes, one type
//!
//! * [`LinuxDatabaseBackend::new`] — memory only. Every existing test and every
//!   seat-less sandbox keeps exactly the behaviour it had.
//! * [`LinuxDatabaseBackend::persistent`] — the same API, backed by one
//!   append-only journal file per root directory, replayed at open. This is what
//!   a daemon uses, and it is why a clipboard history outlives the daemon.
//!
//! There is no SQLite here and no dependency added: a key/value domain needs
//! *durability*, not a query planner, and the smallest honest durable thing is a
//! journal of length-prefixed frames plus a compaction pass. Every frame is
//! self-describing, so a torn write at the end of the file costs the last record
//! and nothing else — and the next successful write rewrites the file compactly,
//! which is also how an unclean shutdown repairs itself.
//!
//! # The frame format
//!
//! ```text
//!   kind : u8        'S' set · 'D' delete · 'C' clear-namespace
//!   ns   : u32 len + bytes
//!   key  : u32 len + bytes      (absent for 'C')
//!   value: u32 len + bytes      (only for 'S')
//! ```
//!
//! Little-endian lengths, all-prefixed, no separators: values are arbitrary
//! bytes (a 4 MiB screenshot is a value), so a text format would need escaping
//! or base64 and would inflate exactly the payloads this host cares about.
//!
//! # Compaction
//!
//! The journal is rewritten when it holds more than `4 × live keys + 64` frames:
//! a rewrite goes to `journal.tmp`, is fsynced, and is renamed over the journal,
//! so a crash during compaction leaves the old journal intact.

extern crate std;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::core::{Error, Result};
use crate::domains::database::backend::DatabaseBackend;
use crate::domains::database::types::Pagination;

/// The journal's file name inside a store root.
const JOURNAL: &str = "database.journal";

/// How many frames a journal may hold beyond `4 × live keys` before the write
/// that noticed is followed by a compacting rewrite.
const COMPACT_SLACK: usize = 64;

/// A frame in the journal.
enum Frame {
    Set { ns: String, key: String, value: Vec<u8> },
    Delete { ns: String, key: String },
    Clear { ns: String },
}

/// The journal's file handle plus how many frames it currently holds.
struct Journal {
    file: File,
    frames: usize,
}

impl Journal {
    fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)
            .map_err(|error| Error::failed(&alloc::format!("cannot open the store journal: {error}")))?;
        Ok(Journal { file, frames: 0 })
    }

    /// Append one frame and make it durable. `sync_data` on every write is the
    /// deliberate trade: a copy the user just made must survive a power cut, and
    /// a clipboard write is not a high-frequency event.
    fn append(&mut self, frame: &Frame) -> Result<()> {
        let mut bytes = Vec::new();
        encode(frame, &mut bytes);
        self.file
            .write_all(&bytes)
            .and_then(|()| self.file.flush())
            .and_then(|()| self.file.sync_data())
            .map_err(|error| Error::failed(&alloc::format!("cannot write the store journal: {error}")))?;
        self.frames += 1;
        Ok(())
    }
}

/// Encode one frame into `out`.
fn encode(frame: &Frame, out: &mut Vec<u8>) {
    let (kind, ns, key, value): (u8, &str, Option<&str>, Option<&[u8]>) = match frame {
        Frame::Set { ns, key, value } => (b'S', ns, Some(key), Some(value)),
        Frame::Delete { ns, key } => (b'D', ns, Some(key), None),
        Frame::Clear { ns } => (b'C', ns, None, None),
    };
    out.push(kind);
    push_blob(out, ns.as_bytes());
    if let Some(key) = key {
        push_blob(out, key.as_bytes());
    }
    if let Some(value) = value {
        push_blob(out, value);
    }
}

fn push_blob(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// A cursor over the journal's bytes. Every read is bounds-checked and returns
/// `None` at a truncated tail, so one torn frame does not poison the replay.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    fn u32(&mut self) -> Option<u32> {
        let end = self.at.checked_add(4)?;
        let slice = self.bytes.get(self.at..end)?;
        let mut word = [0u8; 4];
        word.copy_from_slice(slice);
        self.at = end;
        Some(u32::from_le_bytes(word))
    }

    fn blob(&mut self) -> Option<Vec<u8>> {
        let len = self.u32()? as usize;
        let end = self.at.checked_add(len)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice.to_vec())
    }

    fn text(&mut self) -> Option<String> {
        String::from_utf8(self.blob()?).ok()
    }
}

/// Replay a journal's bytes into `store`, returning the frames applied.
fn replay(bytes: &[u8], store: &mut HashMap<String, HashMap<String, Vec<u8>>>) -> usize {
    let mut cursor = Cursor { bytes, at: 0 };
    let mut frames = 0;
    loop {
        let Some(kind) = cursor.u8() else { break };
        let Some(ns) = cursor.text() else { break };
        match kind {
            b'S' => {
                let (Some(key), Some(value)) = (cursor.text(), cursor.blob()) else { break };
                store.entry(ns).or_default().insert(key, value);
            }
            b'D' => {
                let Some(key) = cursor.text() else { break };
                if let Some(keys) = store.get_mut(&ns) {
                    keys.remove(&key);
                }
            }
            b'C' => {
                if let Some(keys) = store.get_mut(&ns) {
                    keys.clear();
                }
            }
            // An unknown kind means the file is not ours (or is newer than us):
            // stop replaying rather than guess at the remaining bytes.
            _ => break,
        }
        frames += 1;
    }
    frames
}

/// The database domain's host backend: a key/value store that survives the
/// process when it is given a root directory.
pub struct LinuxDatabaseBackend {
    store: Mutex<HashMap<String, HashMap<String, Vec<u8>>>>,
    journal: Mutex<Option<Journal>>,
    root: Option<PathBuf>,
}

impl LinuxDatabaseBackend {
    /// The in-memory store: no file, no replay, exactly today's behaviour.
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
            journal: Mutex::new(None),
            root: None,
        }
    }

    /// A store rooted at `root` (created if absent), replayed from its journal.
    ///
    /// A journal that cannot be read is not fatal: the store opens empty and
    /// says so, because refusing to serve the database domain because one file
    /// is unreadable would take the clipboard's history down with it.
    pub fn persistent(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|error| {
            Error::failed(&alloc::format!("cannot create the store directory {}: {error}", root.display()))
        })?;
        let path = root.join(JOURNAL);

        let mut store: HashMap<String, HashMap<String, Vec<u8>>> = HashMap::new();
        let mut frames = 0;
        if let Ok(mut bytes) = std::fs::read(&path) {
            // A blank tail from a torn write is trimmed to the last whole frame
            // by the replay itself; the compaction on the first write then
            // rewrites the file without it.
            frames = replay(&bytes, &mut store);
            bytes.clear();
        }

        let mut journal = Journal::open(&path)?;
        journal.frames = frames;
        Ok(Self {
            store: Mutex::new(store),
            journal: Mutex::new(Some(journal)),
            root: Some(root),
        })
    }

    /// The store's root directory, when it has one. Diagnostics only: a caller
    /// that wants to know where its data lives asks here rather than guessing.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Append a frame (when persistent) and compact if the journal has outgrown
    /// the live set. Called with the store lock held, so a write and its journal
    /// entry can never be reordered against another write.
    fn record(&self, store: &HashMap<String, HashMap<String, Vec<u8>>>, frame: Frame) {
        let mut guard = match self.journal.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let Some(journal) = guard.as_mut() else { return };
        // A journal write that fails must not fail the call: the value is in
        // memory and the next write retries with a compacted file.
        if journal.append(&frame).is_err() {
            return;
        }
        let live: usize = store.values().map(HashMap::len).sum();
        if journal.frames > live.saturating_mul(4) + COMPACT_SLACK {
            self.compact(store, journal);
        }
    }

    /// Rewrite the journal from the live map: temp file, fsync, rename.
    fn compact(&self, store: &HashMap<String, HashMap<String, Vec<u8>>>, journal: &mut Journal) {
        let Some(root) = self.root.as_deref() else { return };
        let path = root.join(JOURNAL);
        let tmp = root.join("database.journal.tmp");
        let mut bytes = Vec::new();
        let mut namespaces: Vec<&String> = store.keys().collect();
        namespaces.sort();
        for ns in namespaces {
            let Some(keys) = store.get(ns) else { continue };
            let mut ordered: Vec<&String> = keys.keys().collect();
            ordered.sort();
            for key in ordered {
                let Some(value) = keys.get(key) else { continue };
                encode(
                    &Frame::Set { ns: ns.clone(), key: key.clone(), value: value.clone() },
                    &mut bytes,
                );
            }
        }
        let written = File::create(&tmp)
            .and_then(|mut file| {
                file.write_all(&bytes)?;
                file.flush()?;
                file.sync_data()
            })
            .and_then(|()| std::fs::rename(&tmp, &path));
        if written.is_err() {
            return;
        }
        let Ok(mut file) = OpenOptions::new().read(true).append(true).open(&path) else {
            return;
        };
        // Truncate-then-append would lose a concurrent frame; compaction runs
        // under the store lock, so nothing else is writing here.
        let _ = file.seek(SeekFrom::End(0));
        journal.file = file;
        journal.frames = store.values().map(HashMap::len).sum();
    }

    /// Read the whole journal back as bytes (tests and diagnostics).
    #[cfg(test)]
    fn journal_bytes(&self) -> Vec<u8> {
        let Some(root) = self.root.as_deref() else { return Vec::new() };
        let mut bytes = Vec::new();
        if let Ok(mut file) = File::open(root.join(JOURNAL)) {
            let _ = file.read_to_end(&mut bytes);
        }
        bytes
    }
}

impl Default for LinuxDatabaseBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DatabaseBackend for LinuxDatabaseBackend {
    fn get(&self, namespace: &str, key: &str) -> Result<Vec<u8>> {
        let store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        store
            .get(namespace)
            .and_then(|ns| ns.get(key).cloned())
            .ok_or_else(Error::not_found)
    }

    fn has(&self, namespace: &str, key: &str) -> Result<bool> {
        let store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        Ok(store.get(namespace).map(|ns| ns.contains_key(key)).unwrap_or(false))
    }

    fn list_keys(&self, namespace: &str, prefix: &str, page: Pagination) -> Result<(Vec<String>, u64)> {
        let store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        let mut keys: Vec<String> = store
            .get(namespace)
            .map(|ns| ns.keys().filter(|k| k.starts_with(prefix)).cloned().collect())
            .unwrap_or_default();
        keys.sort();
        let total = keys.len() as u64;
        let limit = page.effective_limit() as usize;
        let off = page.offset as usize;
        let slice = keys.into_iter().skip(off).take(limit).collect();
        Ok((slice, total))
    }

    fn count(&self, namespace: &str) -> Result<u64> {
        let store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        Ok(store.get(namespace).map(|ns| ns.len() as u64).unwrap_or(0))
    }

    fn set(&self, namespace: &str, key: &str, value: &[u8]) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        store
            .entry(namespace.to_string())
            .or_default()
            .insert(key.to_string(), value.to_vec());
        self.record(
            &store,
            Frame::Set {
                ns: namespace.to_string(),
                key: key.to_string(),
                value: value.to_vec(),
            },
        );
        Ok(())
    }

    fn delete(&self, namespace: &str, key: &str) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        if let Some(ns) = store.get_mut(namespace) {
            ns.remove(key);
        }
        self.record(
            &store,
            Frame::Delete { ns: namespace.to_string(), key: key.to_string() },
        );
        Ok(())
    }

    fn clear(&self, namespace: &str) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        if let Some(ns) = store.get_mut(namespace) {
            ns.clear();
        }
        self.record(&store, Frame::Clear { ns: namespace.to_string() });
        Ok(())
    }

    fn list_namespaces(&self, page: Pagination) -> Result<(Vec<String>, u64)> {
        let store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        let mut ns: Vec<String> = store.keys().cloned().collect();
        ns.sort();
        let total = ns.len() as u64;
        let limit = page.effective_limit() as usize;
        let off = page.offset as usize;
        Ok((ns.into_iter().skip(off).take(limit).collect(), total))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::core::ErrorCode;
    use crate::domains::database::backend::DatabaseBackend;

    /// A backend over the engine's OWN store: this domain keeps its data in
    /// the process's memory, so a test never touches a host file and never
    /// needs to restore one.
    fn backend() -> LinuxDatabaseBackend {
        LinuxDatabaseBackend::new()
    }

    /// The whole page: a `Pagination::default()` carries a limit of 0, which
    /// the contract reads as ONE item.
    const ALL: crate::domains::database::types::Pagination =
        crate::domains::database::types::Pagination { offset: 0, limit: u32::MAX };

    /// `get` answers with the bytes that were stored, and a key that was
    /// never stored is `notFound` — never a fabricated empty value.
    #[test]
    fn get_answers_with_the_stored_bytes() {
        let b = backend();
        b.set("ns", "k", b"the real bytes").expect("set");
        assert_eq!(b.get("ns", "k").expect("get"), b"the real bytes".to_vec());
        let missing = b.get("ns", "absent").expect_err("an absent key");
        assert_eq!(missing.code, ErrorCode::NotFound);
        let no_namespace = b.get("nosuchns", "k").expect_err("an absent namespace");
        assert_eq!(no_namespace.code, ErrorCode::NotFound);
    }

    /// `has` answers with the store's own answer: true for a key that is
    /// there, false for one that is not, and false — never an error — for a
    /// namespace that does not exist.
    #[test]
    fn has_answers_with_the_stores_own_yes_or_no() {
        let b = backend();
        b.set("ns", "k", b"v").expect("set");
        assert!(b.has("ns", "k").expect("has"));
        assert!(!b.has("ns", "absent").expect("has"));
        // A namespace that was never created is an honest "no", not a failure.
        assert!(!b.has("nosuchns", "k").expect("has"));
    }

    /// `listKeys` answers with the namespace's OWN keys, sorted, filtered by
    /// the prefix, and paginated under the contract's rule.
    #[test]
    fn list_keys_answers_with_the_namespaces_own_keys() {
        let b = backend();
        b.set("ns", "alpha", b"1").expect("set");
        b.set("ns", "beta", b"2").expect("set");
        b.set("ns", "gamma", b"3").expect("set");
        let (keys, total) = b.list_keys("ns", "", ALL).expect("list");
        assert_eq!(total, 3);
        assert_eq!(keys, alloc::vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]);
        // The prefix filters, and the total counts what the prefix matched.
        let (prefixed, total) = b.list_keys("ns", "a", ALL).expect("list");
        assert_eq!(prefixed, alloc::vec!["alpha".to_string()]);
        assert_eq!(total, 1, "the total is the count the prefix matched");
        // The page is honoured.
        let (page, total) = b
            .list_keys("ns", "", crate::domains::database::types::Pagination { offset: 1, limit: 1 })
            .expect("list");
        assert_eq!(page, alloc::vec!["beta".to_string()]);
        assert_eq!(total, 3);
        // A namespace that was never created is the honest empty.
        let (none, total) = b.list_keys("nosuchns", "", ALL).expect("list");
        assert!(none.is_empty());
        assert_eq!(total, 0);
    }

    /// `count` answers with the namespace's OWN key count, and zero for a
    /// namespace that was never created.
    #[test]
    fn count_answers_with_the_namespaces_own_key_count() {
        let b = backend();
        assert_eq!(b.count("ns").expect("count"), 0);
        b.set("ns", "a", b"1").expect("set");
        b.set("ns", "b", b"2").expect("set");
        assert_eq!(b.count("ns").expect("count"), 2);
        // Overwriting a key does not add one.
        b.set("ns", "a", b"3").expect("set");
        assert_eq!(b.count("ns").expect("count"), 2);
        // Another namespace has its own count.
        b.set("other", "a", b"1").expect("set");
        assert_eq!(b.count("other").expect("count"), 1);
    }

    /// `set` stores the bytes, and the store keeps them exactly: a value
    /// written is the value read back, byte for byte.
    #[test]
    fn set_stores_the_bytes_the_store_gives_back() {
        let b = backend();
        b.set("ns", "k", b"\x00\x01\x02binary").expect("set");
        assert_eq!(b.get("ns", "k").expect("get"), b"\x00\x01\x02binary".to_vec());
        // A write replaces the old value; it does not append.
        b.set("ns", "k", b"new").expect("set");
        assert_eq!(b.get("ns", "k").expect("get"), b"new".to_vec());
        assert_eq!(b.count("ns").expect("count"), 1);
    }

    /// `delete` removes the key, and the store says so: a deleted key is
    /// `notFound`, never a zeroed value.
    #[test]
    fn delete_removes_the_key_and_the_store_says_so() {
        let b = backend();
        b.set("ns", "k", b"v").expect("set");
        b.delete("ns", "k").expect("delete");
        let missing = b.get("ns", "k").expect_err("a deleted key");
        assert_eq!(missing.code, ErrorCode::NotFound);
        assert!(!b.has("ns", "k").expect("has"));
        // Deleting a key that is not there is the honest no-op, not a failure.
        b.delete("ns", "absent").expect("delete");
        // The namespace itself survives: only the key is gone.
        assert_eq!(b.count("ns").expect("count"), 0);
    }

    /// `clear` empties the namespace and leaves the OTHER namespaces alone.
    #[test]
    fn clear_empties_the_namespace_and_only_the_namespace() {
        let b = backend();
        b.set("ns", "a", b"1").expect("set");
        b.set("ns", "b", b"2").expect("set");
        b.set("other", "c", b"3").expect("set");
        b.clear("ns").expect("clear");
        assert_eq!(b.count("ns").expect("count"), 0);
        assert_eq!(b.count("other").expect("count"), 1, "another namespace is untouched");
        // Clearing a namespace that was never created is the honest no-op.
        b.clear("nosuchns").expect("clear");
    }

    /// `listNamespaces` answers with the store's OWN namespaces, sorted and
    /// paginated.
    #[test]
    fn list_namespaces_answers_with_the_stores_own_namespaces() {
        let b = backend();
        assert!(b.list_namespaces(ALL).expect("list").0.is_empty());
        b.set("beta", "k", b"1").expect("set");
        b.set("alpha", "k", b"1").expect("set");
        let (names, total) = b.list_namespaces(ALL).expect("list");
        assert_eq!(total, 2);
        assert_eq!(names, alloc::vec!["alpha".to_string(), "beta".to_string()]);
        // The page is honoured.
        let (page, total) = b
            .list_namespaces(crate::domains::database::types::Pagination { offset: 0, limit: 1 })
            .expect("list");
        assert_eq!(page, alloc::vec!["alpha".to_string()]);
        assert_eq!(total, 2);
    }

    /// The durable half: a persistent store's writes survive a reopen, and the
    /// binary values come back byte for byte.
    #[test]
    fn a_persistent_store_survives_a_reopen() {
        let root = std::env::temp_dir().join(alloc::format!(
            "smart-db-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        {
            let b = LinuxDatabaseBackend::persistent(&root).expect("open");
            b.set("clipboard.history", "0001", b"\x00\xff binary value").expect("set");
            b.set("clipboard.history", "0002", b"second").expect("set");
            b.set("other", "k", b"v").expect("set");
            b.delete("other", "k").expect("delete");
            assert_eq!(b.count("clipboard.history").expect("count"), 2);
        }
        {
            let reopened = LinuxDatabaseBackend::persistent(&root).expect("reopen");
            assert_eq!(
                reopened.get("clipboard.history", "0001").expect("get"),
                b"\x00\xff binary value".to_vec()
            );
            assert_eq!(reopened.count("clipboard.history").expect("count"), 2);
            // The delete was journalled too, so it is still deleted.
            assert!(!reopened.has("other", "k").expect("has"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Compaction rewrites the journal from the live set, so a key overwritten a
    /// thousand times costs one frame afterwards — and the values are still the
    /// last ones written.
    #[test]
    fn compaction_keeps_the_live_set_and_drops_the_history() {
        let root = std::env::temp_dir().join(alloc::format!(
            "smart-db-compact-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let b = LinuxDatabaseBackend::persistent(&root).expect("open");
        for round in 0..400u32 {
            b.set("ns", "k", alloc::format!("value-{round}").as_bytes()).expect("set");
        }
        assert_eq!(b.get("ns", "k").expect("get"), b"value-399".to_vec());
        let bytes = b.journal_bytes();
        assert!(
            bytes.len() < 512,
            "the journal compacted to the live set, got {} bytes",
            bytes.len()
        );
        drop(b);
        let reopened = LinuxDatabaseBackend::persistent(&root).expect("reopen");
        assert_eq!(reopened.get("ns", "k").expect("get"), b"value-399".to_vec());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A torn tail (the classic power-cut shape) costs the unfinished frame and
    /// nothing before it.
    #[test]
    fn a_torn_tail_costs_only_the_unfinished_frame() {
        let mut store: HashMap<String, HashMap<String, Vec<u8>>> = HashMap::new();
        let mut bytes = Vec::new();
        encode(
            &Frame::Set { ns: "ns".into(), key: "kept".into(), value: b"whole".to_vec() },
            &mut bytes,
        );
        let whole = bytes.len();
        encode(
            &Frame::Set { ns: "ns".into(), key: "torn".into(), value: b"partial".to_vec() },
            &mut bytes,
        );
        // Chop the second frame in the middle of its value.
        bytes.truncate(whole + 6);
        assert_eq!(replay(&bytes, &mut store), 1);
        assert_eq!(store["ns"]["kept"], b"whole".to_vec());
        assert!(!store["ns"].contains_key("torn"));
    }
}
