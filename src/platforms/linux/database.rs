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
    use crate::domains::database::backend::DatabaseBackend;

    #[test]
    fn core_works() {
        let b = LinuxDatabaseBackend::new();
        let _ = b.set("ns", "k", b"v");
        let _ = b.get("ns", "k");
        let _ = b.has("ns", "k");
        let _ = b.count("ns");
        let _ = b.list_keys("ns", "", crate::domains::database::types::Pagination::default());
        let _ = b.list_namespaces(crate::domains::database::types::Pagination::default());
    }
}
