use alloc::string::String;
use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{Page, Pagination, SubscriptionId, TransactionId};

pub trait DatabaseBackend {
    fn get(&self, _namespace: &str, _key: &str) -> Result<Vec<u8>> {
        Err(Error::not_supported("no-backend: database get is not served on this backend"))
    }
    fn has(&self, _namespace: &str, _key: &str) -> Result<bool> {
        Err(Error::not_supported("no-backend: database has is not served on this backend"))
    }
    fn list_keys(&self, _namespace: &str, _prefix: &str, _page: Pagination) -> Result<(Vec<String>, u64)> {
        Err(Error::not_supported("no-backend: database listKeys is not served on this backend"))
    }
    fn count(&self, _namespace: &str) -> Result<u64> {
        Err(Error::not_supported("no-backend: database count is not served on this backend"))
    }
    fn set(&self, _namespace: &str, _key: &str, _value: &[u8]) -> Result<()> {
        Err(Error::not_supported("no-backend: database set is not served on this backend"))
    }
    fn delete(&self, _namespace: &str, _key: &str) -> Result<()> {
        Err(Error::not_supported("no-backend: database delete is not served on this backend"))
    }
    fn clear(&self, _namespace: &str) -> Result<()> {
        Err(Error::not_supported("no-backend: database clear is not served on this backend"))
    }
    fn list_namespaces(&self, _page: Pagination) -> Result<(Vec<String>, u64)> {
        Err(Error::not_supported("no-backend: database listNamespaces is not served on this backend"))
    }
    fn database_caps(&self) -> Vec<&'static str> { alloc::vec!["database.kv"] }
}
