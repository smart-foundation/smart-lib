//! The `database` fixture: a scripted backend over the engine's OWNED types.
//!
//! The host is never touched (charter law #8): every read is served from the
//! script, and every mutating verb is RECORDED so a test can prove the shell
//! issued the right engine call — and that a refused write reaches no engine
//! at all.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use smart_lib::core::{Error, Result};

use smart_lib::domains::database::types::Pagination as DatabasePagination;
use smart_lib::domains::database::DatabaseBackend;

/// A scripted key/value store: namespaces of keys to byte payloads, with the
/// engine's own paging.
#[derive(Default)]
pub struct MockDatabase {
    store: RefCell<HashMap<String, HashMap<String, Vec<u8>>>>,
    /// The engine calls a mutating verb made, in order.
    pub writes: RefCell<Vec<String>>,
}

impl MockDatabase {
    /// A store with one namespace and two keys.
    pub fn seeded() -> Self {
        let mut store = HashMap::new();
        let mut keys = HashMap::new();
        keys.insert("greeting".to_string(), b"hello".to_vec());
        keys.insert("colour".to_string(), b"green".to_vec());
        store.insert("demo".to_string(), keys);
        MockDatabase {
            store: RefCell::new(store),
            writes: RefCell::new(Vec::new()),
        }
    }

    /// An empty store.
    pub fn empty() -> Self {
        MockDatabase::default()
    }
}

impl DatabaseBackend for MockDatabase {
    fn get(&self, namespace: &str, key: &str) -> Result<Vec<u8>> {
        self.store
            .borrow()
            .get(namespace)
            .and_then(|keys| keys.get(key))
            .cloned()
            .ok_or_else(Error::not_found)
    }

    fn has(&self, namespace: &str, key: &str) -> Result<bool> {
        Ok(self
            .store
            .borrow()
            .get(namespace)
            .map(|keys| keys.contains_key(key))
            .unwrap_or(false))
    }

    fn list_keys(
        &self,
        namespace: &str,
        prefix: &str,
        page: DatabasePagination,
    ) -> Result<(Vec<String>, u64)> {
        let store = self.store.borrow();
        let mut keys: Vec<String> = store
            .get(namespace)
            .map(|keys| {
                keys.keys()
                    .filter(|key| key.starts_with(prefix))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        keys.sort();
        let total = keys.len() as u64;
        let items: Vec<String> = keys
            .into_iter()
            .skip(page.offset as usize)
            .take(page.effective_limit() as usize)
            .collect();
        Ok((items, total))
    }

    fn count(&self, namespace: &str) -> Result<u64> {
        Ok(self
            .store
            .borrow()
            .get(namespace)
            .map(|keys| keys.len() as u64)
            .unwrap_or(0))
    }

    fn set(&self, namespace: &str, key: &str, value: &[u8]) -> Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("set {namespace}/{key}"));
        self.store
            .borrow_mut()
            .entry(namespace.to_string())
            .or_default()
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn delete(&self, namespace: &str, key: &str) -> Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("delete {namespace}/{key}"));
        let mut store = self.store.borrow_mut();
        let keys = store.get_mut(namespace).ok_or_else(Error::not_found)?;
        keys.remove(key)
            .map(|_| ())
            .ok_or_else(Error::not_found)
    }

    fn clear(&self, namespace: &str) -> Result<()> {
        self.writes.borrow_mut().push(format!("clear {namespace}"));
        let mut store = self.store.borrow_mut();
        store
            .remove(namespace)
            .map(|_| ())
            .ok_or_else(Error::not_found)
    }

    fn list_namespaces(&self, page: DatabasePagination) -> Result<(Vec<String>, u64)> {
        let store = self.store.borrow();
        let mut names: Vec<String> = store.keys().cloned().collect();
        names.sort();
        let total = names.len() as u64;
        let items: Vec<String> = names
            .into_iter()
            .skip(page.offset as usize)
            .take(page.effective_limit() as usize)
            .collect();
        Ok((items, total))
    }
}
