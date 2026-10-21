extern crate std;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use std::collections::HashMap;
use std::sync::Mutex;

use crate::core::{Error, Result};
use crate::domains::database::backend::DatabaseBackend;
use crate::domains::database::types::Pagination;

pub struct LinuxDatabaseBackend {
    store: Mutex<HashMap<String, HashMap<String, Vec<u8>>>>,
}

impl LinuxDatabaseBackend {
    pub fn new() -> Self {
        Self { store: Mutex::new(HashMap::new()) }
    }
}

impl DatabaseBackend for LinuxDatabaseBackend {
    fn get(&self, namespace: &str, key: &str) -> Result<Vec<u8>> {
        let store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        store.get(namespace).and_then(|ns| ns.get(key).cloned()).ok_or_else(|| Error::not_found())
    }
    fn has(&self, namespace: &str, key: &str) -> Result<bool> {
        let store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        Ok(store.get(namespace).map(|ns| ns.contains_key(key)).unwrap_or(false))
    }
    fn list_keys(&self, namespace: &str, prefix: &str, page: Pagination) -> Result<(Vec<String>, u64)> {
        let store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        let mut keys: Vec<String> = store.get(namespace).map(|ns| ns.keys().filter(|k| k.starts_with(prefix)).cloned().collect()).unwrap_or_default();
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
        store.entry(namespace.to_string()).or_default().insert(key.to_string(), value.to_vec());
        Ok(())
    }
    fn delete(&self, namespace: &str, key: &str) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        if let Some(ns) = store.get_mut(namespace) { ns.remove(key); }
        Ok(())
    }
    fn clear(&self, namespace: &str) -> Result<()> {
        let mut store = self.store.lock().map_err(|_| Error::failed("lock failed"))?;
        if let Some(ns) = store.get_mut(namespace) { ns.clear(); }
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
}
